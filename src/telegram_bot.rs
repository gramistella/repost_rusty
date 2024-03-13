use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use teloxide::adaptors::throttle::{Limits, Throttle};
use teloxide::types::{InputFile, MessageId};
use teloxide::{
    dispatching::dialogue::InMemStorage,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};
use tokio::sync::mpsc::Receiver;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::database::{ContentInfo, Database, DatabaseTransaction, FailedContent, DEFAULT_FAILURE_EXPIRATION};
use crate::telegram_bot::errors::handle_message_is_not_modified_error;
use crate::telegram_bot::helpers::send_or_replace_navigation_bar;
use crate::telegram_bot::state::{schema, State};
use crate::utils::now_in_my_timezone;
use crate::{CHAT_ID, REFRESH_RATE};

mod callbacks;
mod commands;
mod errors;
mod helpers;
mod messages;
mod state;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct UIDefinitions {
    buttons: HashMap<String, String>,
    labels: HashMap<String, String>,
}

struct NavigationBar {
    message_id: MessageId,
    current_total_pages: i32,
    last_caption: String,
    last_updated_at: DateTime<Utc>,
}
type BotDialogue = Dialogue<State, InMemStorage<State>>;

pub(crate) async fn run_bot(rx: Receiver<(String, String, String, String)>, database: Database, credentials: HashMap<String, String>) {
    let api_token = credentials.get("telegram_api_token").unwrap();
    let bot = Bot::new(api_token).throttle(Limits::default());

    let storage = InMemStorage::new();
    let dialogue = BotDialogue::new(storage.clone(), CHAT_ID);

    let ui_definitions_yaml_data = include_str!("../config/ui_definitions.yaml");
    let config: UIDefinitions = serde_yaml::from_str(&ui_definitions_yaml_data).expect("Error parsing config file");

    let execution_mutex = Arc::new(Mutex::new(()));

    let nav_bar = NavigationBar {
        message_id: MessageId(0),
        current_total_pages: 0,
        last_caption: "".to_string(),
        last_updated_at: Utc::now(),
    };

    let nav_bar_mutex = Arc::new(Mutex::new(nav_bar));

    tokio::select! {
        _ = start_dispatcher(bot.clone(), dialogue.clone(), execution_mutex.clone(), database.clone(), config.clone(), storage.clone(), nav_bar_mutex.clone()) => {},
        _ = receive_videos(rx, bot, dialogue, execution_mutex, database, config, nav_bar_mutex.clone()) => {},
    }
}

async fn start_dispatcher(bot: Throttle<Bot>, dialogue: BotDialogue, execution_mutex: Arc<Mutex<()>>, database: Database, config: UIDefinitions, storage: Arc<InMemStorage<State>>, nav_bar_mutex: Arc<Mutex<NavigationBar>>) {
    let _ = execution_mutex.lock().await;
    let mut dispatcher_builder = Dispatcher::builder(bot.clone(), schema())
        .dependencies(dptree::deps![dialogue.clone(), storage.clone(), execution_mutex.clone(), database.clone(), config.clone(), nav_bar_mutex])
        .enable_ctrlc_handler()
        .build();
    let dispatcher_future = dispatcher_builder.dispatch();
    dispatcher_future.await;
}

async fn receive_videos(mut rx: Receiver<(String, String, String, String)>, bot: Throttle<Bot>, dialogue: BotDialogue, execution_mutex: Arc<Mutex<()>>, database: Database, config: UIDefinitions, nav_bar_mutex: Arc<Mutex<NavigationBar>>) {
    // Give a head start to the dispatcher
    sleep(Duration::from_secs(1)).await;

    loop {
        let mut tx = database.begin_transaction().unwrap();
        if let Some((received_url, received_caption, original_author, original_shortcode)) = rx.recv().await {
            // This if statement is not actually needed in production since the scraper will not send the same video twice
            if tx.does_content_exist_with_shortcode(original_shortcode.clone()) == false {
                let re = regex::Regex::new(r"#\w+").unwrap();
                let cloned_caption = received_caption.clone();
                let hashtags: Vec<&str> = re.find_iter(&cloned_caption).map(|mat| mat.as_str()).collect();
                let hashtags = hashtags.join(" ");

                let caption = re.replace_all(&received_caption.clone(), "").to_string();
                let user_settings = tx.load_user_settings().unwrap();

                let video = ContentInfo {
                    url: received_url.clone(),
                    status: "waiting".to_string(),
                    caption,
                    hashtags,
                    original_author: original_author.clone(),
                    original_shortcode: original_shortcode.clone(),
                    last_updated_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
                    url_last_updated_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
                    page_num: 1,
                    encountered_errors: 0,
                };

                let message_id = tx.get_temp_message_id(user_settings);
                let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(MessageId(message_id), video.clone())]);

                tx.save_content_mapping(content_mapping).unwrap();
            };

            //println!("Received URL: \n\"{}\"\ncaption: \n\"{}\"\n", received_url, received_caption);
        }
        let _success = match update_view(bot.clone(), dialogue.clone(), execution_mutex.clone(), database.clone(), config.clone(), nav_bar_mutex.clone()).await {
            Ok(..) => {
                //println!("Video sent successfully")
            }
            Err(e) => {
                println!("Error sending video in loop: {}", e)
            }
        };
    }
}

async fn update_view(bot: Throttle<Bot>, dialogue: BotDialogue, execution_mutex: Arc<Mutex<()>>, database: Database, ui_definitions: UIDefinitions, nav_bar_mutex: Arc<Mutex<NavigationBar>>) -> HandlerResult {
    let _mutex_guard = execution_mutex.lock().await;

    let dialogue_state = dialogue.get().await.unwrap().unwrap_or_else(|| State::PageView);

    if dialogue_state != State::PageView {
        return Ok(());
    }

    let mut tx = database.begin_transaction().unwrap();
    if let Ok(video_mapping) = tx.load_page() {
        for (message_id, mut video_info) in video_mapping {
            let input_file = InputFile::url(video_info.url.parse().unwrap());

            if video_info.encountered_errors > 0 {
                continue;
            }
            let result = match video_info.status.as_str() {
                "waiting" => process_waiting(&bot, &mut tx, &ui_definitions, &mut video_info, input_file).await,
                "pending_hidden" => process_pending_hidden(&bot, &ui_definitions, &mut tx, &mut video_info, input_file).await,
                "pending_shown" => process_pending_shown(message_id, &mut video_info).await,
                "posted_hidden" => process_posted_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, &input_file).await,
                "posted_shown" => process_posted_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "accepted_hidden" => process_accepted_hidden(&bot, &ui_definitions, &mut tx, &mut video_info, input_file).await,
                "accepted_shown" => process_accepted_shown(message_id, &mut video_info).await,
                "queued_hidden" => process_queued_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, &input_file).await,
                "queued_shown" => process_queued_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "rejected_hidden" => process_rejected_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "rejected_shown" => process_rejected_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "failed_hidden" => process_failed_hidden(&bot, &ui_definitions, &mut tx, &mut video_info, input_file).await,
                "failed_shown" => process_failed_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "removed_from_view" => Ok((message_id, video_info.clone())),
                _ => {
                    println!("Unknown status: {}", video_info.status);
                    println!("Video info: {:?}", video_info);
                    Ok((message_id, video_info.clone()))
                }
            };

            match result {
                Ok((message_id, video_info)) => {
                    if video_info.status == "removed_from_view" {
                        // println!("Removing video from view: {}", video_info.original_shortcode);
                        tx.remove_content_info_with_shortcode(video_info.original_shortcode).unwrap();
                    } else {
                        let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info)]);
                        tx.save_content_mapping(content_mapping).unwrap();
                    }
                }
                Err(e) => {
                    println!("Error processing video {}, with status {}", video_info.original_shortcode, video_info.status);
                    println!("With error: {}", e);
                }
            }
        }
    }

    send_or_replace_navigation_bar(bot, database, nav_bar_mutex).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    Ok(())
}
async fn process_failed_shown(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, video_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    let mut content = tx.get_failed_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
    // Parse last_updated_at at into a DateTime
    let last_updated_at = DateTime::parse_from_rfc3339(&content.last_updated_at).unwrap();
    let will_expire_at = DateTime::parse_from_rfc3339(&content.failed_at).unwrap().checked_add_signed(chrono::Duration::from_std(DEFAULT_FAILURE_EXPIRATION).unwrap()).unwrap();
    let now = now_in_my_timezone(tx.load_user_settings()?);
    // Check
    if now > will_expire_at {
        video_info.status = "removed_from_view".to_string();
        content.expired = true;
        bot.delete_message(CHAT_ID, message_id).await?;
    }
    if now > last_updated_at + REFRESH_RATE {
        video_info.status = "failed_shown".to_string();

        let full_caption = generate_full_video_caption("failed", ui_definitions, tx, video_info);
        let video_actions = get_action_buttons(ui_definitions, &["remove_from_view"], message_id);
        edit_message_caption_and_markup(&bot, CHAT_ID, message_id, full_caption, video_actions).await?;

        content.last_updated_at = now.to_rfc3339();
    }
    tx.update_failed_content(content.clone())?;
    Ok((message_id, video_info.clone()))
}

pub fn generate_full_video_caption(caption_type: &str, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, video_info: &ContentInfo) -> String {
    let caption_body = format!("{}\n{}\n(from @{})", video_info.caption, video_info.hashtags, video_info.original_author);

    match caption_type {
        "accepted" => {
            let full_video_caption = format!("{}\n\n{}\n\n", caption_body, ui_definitions.labels.get("accepted_caption").unwrap());
            full_video_caption
        }
        "rejected" => {
            let full_video_caption = format!("{}\n\n{}\n\n", caption_body, ui_definitions.labels.get("rejected_caption").unwrap());
            full_video_caption
        }
        "queued" => {
            let queued_content = tx.get_queued_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();

            let will_post_at = DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            let now = now_in_my_timezone(user_settings.clone());
            let duration_until_expiration = will_post_at.signed_duration_since(now);
            let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
            let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
            let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

            let queued_caption = ui_definitions.labels.get("queued_caption").unwrap();
            let formatted_datetime = will_post_at.format("%H:%M %m/%d").to_string();

            let mut countdown_caption = format!("({} hours, {} minutes and {} seconds from now)", hours_until_expiration, minutes_until_expiration, seconds_until_expiration);

            let hours = hours_until_expiration.parse::<i32>().unwrap_or(0);
            let minutes = minutes_until_expiration.parse::<i32>().unwrap_or(0);
            let seconds = seconds_until_expiration.parse::<i32>().unwrap_or(0);

            if hours <= 0 && minutes <= 0 && seconds <= 0 {
                countdown_caption = "(Posting now...)".to_string();
            }

            let full_video_caption = format!(
                "{}\n{}\n(from @{})\n\n{}\n\nWill post at {}\n{}",
                queued_content.caption, queued_content.hashtags, queued_content.original_author, queued_caption, formatted_datetime, countdown_caption
            );
            full_video_caption
        }
        "failed" => {
            let failed_content = tx.get_failed_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();

            let failed_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap();
            let will_expire_at = failed_at.checked_add_signed(chrono::Duration::from_std(DEFAULT_FAILURE_EXPIRATION).unwrap()).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            let now = now_in_my_timezone(user_settings.clone());
            let duration_until_expiration = will_expire_at.signed_duration_since(now);
            let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
            let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
            let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

            let posted_caption = ui_definitions.labels.get("failed_caption").unwrap();
            let formatted_datetime = failed_at.format("%H:%M %m/%d").to_string();
            let full_video_caption = format!(
                "{}\n\n{} @ {}\n\nWill expire in {} hours, {} minutes and {} seconds",
                caption_body, posted_caption, formatted_datetime, hours_until_expiration, minutes_until_expiration, seconds_until_expiration
            );
            full_video_caption
        }
        "pending" => caption_body.to_string(),
        "waiting" => caption_body.to_string(),
        "posted" => {
            let posted_content = tx.get_posted_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();

            let datetime = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
            let will_expire_at = datetime.checked_add_signed(chrono::Duration::try_seconds(tx.load_user_settings().unwrap().posted_content_lifespan * 60).unwrap()).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            let now = now_in_my_timezone(user_settings.clone());
            let duration_until_expiration = will_expire_at.signed_duration_since(now);
            let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
            let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
            let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

            let posted_caption = ui_definitions.labels.get("posted_caption").unwrap();
            let formatted_datetime = datetime.format("%H:%M %m/%d").to_string();
            let full_video_caption = format!(
                "{}\n\n{} @ {}\n\nWill expire in {} hours, {} minutes and {} seconds",
                caption_body, posted_caption, formatted_datetime, hours_until_expiration, minutes_until_expiration, seconds_until_expiration
            );
            full_video_caption
        }
        _ => {
            panic!("Unknown caption type: {}", caption_type);
        }
    }
}

async fn process_failed_hidden(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, video_info: &mut ContentInfo, input_file: InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    let sent_message_id = send_video_and_get_id(&bot, input_file).await?;
    let video_actions = get_action_buttons(ui_definitions, &["remove_from_view"], sent_message_id);
    video_info.status = "failed_shown".to_string();
    //let full_video_caption = format!("{}\n\n{}\n\n", full_video_caption, ui_definitions.labels.get("failed_caption").unwrap());

    let full_video_caption = generate_full_video_caption("failed", ui_definitions, tx, video_info);
    edit_message_caption_and_markup(&bot, CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
    Ok((sent_message_id, video_info.clone()))
}

async fn process_pending_shown(message_id: MessageId, video_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    return Ok((message_id, video_info.clone()));
}

async fn process_pending_hidden(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, video_info: &mut ContentInfo, input_file: InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    let sent_message_id = send_video_and_get_id(&bot, input_file).await?;
    let video_actions = get_action_buttons(ui_definitions, &["accept", "reject", "edit"], sent_message_id);
    video_info.status = "pending_shown".to_string();
    let full_video_caption = generate_full_video_caption("pending", ui_definitions, tx, video_info);
    edit_message_caption_and_markup(&bot, CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
    Ok((sent_message_id, video_info.clone()))
}

async fn process_waiting(bot: &Throttle<Bot>, tx: &mut DatabaseTransaction, ui_definitions: &UIDefinitions, video_info: &mut ContentInfo, input_file: InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    let sent_message_id = match send_video_and_get_id(&bot, input_file).await {
        Ok(id) => id,
        Err(e) => {
            if e.to_string().contains("wrong file identifier/HTTP URL specified") || e.to_string().contains("failed to get HTTP URL content") {
                let now = now_in_my_timezone(tx.load_user_settings()?);
                let failed_content = FailedContent {
                    url: video_info.url.clone(),
                    caption: video_info.caption.clone(),
                    hashtags: video_info.hashtags.clone(),
                    original_author: video_info.original_author.clone(),
                    original_shortcode: video_info.original_shortcode.clone(),
                    last_updated_at: (now - REFRESH_RATE).to_rfc3339(),
                    failed_at: now.to_rfc3339(),
                    expired: false,
                };
                tx.save_failed_content(failed_content)?;
                video_info.status = "removed_from_view".to_string();
                return Ok((MessageId(0), video_info.clone()));
            } else {
                panic!("Error sending video in process_waiting: {}", e);
            }
        }
    };
    let video_actions = get_action_buttons(ui_definitions, &["accept", "reject", "edit"], sent_message_id);
    video_info.status = "pending_shown".to_string();
    let full_video_caption = generate_full_video_caption("pending", ui_definitions, tx, video_info);
    edit_message_caption_and_markup(&bot, CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
    Ok((sent_message_id, video_info.clone()))
}

async fn process_accepted_hidden(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, video_info: &mut ContentInfo, input_file: InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    let sent_message_id = send_video_and_get_id(bot, input_file).await?;
    let undo_action = get_action_buttons(ui_definitions, &["undo"], sent_message_id);
    video_info.status = "accepted_shown".to_string();

    let full_video_caption = generate_full_video_caption("accepted", ui_definitions, tx, video_info);
    edit_message_caption_and_markup(bot, CHAT_ID, sent_message_id, full_video_caption, undo_action).await?;
    Ok((sent_message_id, video_info.clone()))
}

async fn process_accepted_shown(message_id: MessageId, video_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    Ok((message_id, video_info.clone()))
}

async fn process_rejected_hidden(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, mut message_id: MessageId, video_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    video_info.status = "rejected_shown".to_string();

    let user_settings = tx.load_user_settings()?;
    let expiry_duration = Duration::from_secs((user_settings.rejected_content_lifespan * 60) as u64);
    let now = now_in_my_timezone(user_settings.clone());

    let mut content = tx.get_rejected_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
    let rejected_at = DateTime::parse_from_rfc3339(&content.rejected_at)?;

    if content.expired {
        return Ok((message_id, video_info.clone()));
    }

    if now > rejected_at + expiry_duration {
        video_info.status = "removed_from_view".to_string();
        match bot.delete_message(CHAT_ID, message_id).await {
            Ok(_) => {
                // println!("Permanently removed post from video_info: {}", content.url);
            }
            Err(_e) => {
                // This will happen when the message is rejected_hidden and is stored in another page,
                // as it gets loaded into the current page, the message to delete cannot be found and the whole thing gets stuck
                // println!(" process_rejected_hidden - Error deleting rejected message with ID: {}: {}", message_id, e);
            }
        }
        content.expired = true;
        tx.save_rejected_content(content.clone())?;
        //println!("Permanently removed post from video_info: {}", content.url);
    } else {
        video_info.status = "rejected_shown".to_string();

        let full_video_caption = generate_full_video_caption("rejected", ui_definitions, tx, video_info);
        let new_message = bot.send_video(CHAT_ID, InputFile::url(content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

        let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
        let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
        let undo_action = [
            InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", new_message.id)),
            InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id)),
        ];

        message_id = new_message.id;

        let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
    }

    Ok((message_id, video_info.clone()))
}

async fn process_queued_hidden(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, video_info: &mut ContentInfo, input_file: &InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    video_info.status = "queued_shown".to_string();

    let full_video_caption = generate_full_video_caption("queued", ui_definitions, tx, video_info);
    // Attempt to edit the existing message caption
    match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
        Ok(_) => {
            let remove_from_queue_action = get_action_buttons(ui_definitions, &["remove_from_queue"], message_id);
            edit_message_caption_and_markup(bot, CHAT_ID, message_id, full_video_caption, remove_from_queue_action).await?;

            let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info.clone())]);
            tx.save_content_mapping(content_mapping).unwrap();
            Ok((message_id, video_info.clone()))
        }
        Err(e) => {
            if e.to_string().contains("message is not modified") {
                let remove_from_queue_action = get_action_buttons(ui_definitions, &["remove_from_queue"], message_id);
                edit_message_caption_and_markup(bot, CHAT_ID, message_id, full_video_caption, remove_from_queue_action).await?;
                Ok((message_id, video_info.clone()))
            } else {
                let sent_message_id = send_video_and_get_id(bot, input_file.clone()).await?;
                let remove_from_queue_action = get_action_buttons(ui_definitions, &["remove_from_queue"], sent_message_id);
                edit_message_caption_and_markup(bot, CHAT_ID, sent_message_id, full_video_caption, remove_from_queue_action).await?;
                Ok((sent_message_id, video_info.clone()))
            }
        }
    }
}

async fn process_queued_shown(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, video_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    let mut queued_content = tx.get_queued_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();

    let user_settings = tx.load_user_settings().unwrap();
    let now = now_in_my_timezone(user_settings.clone());

    let last_updated_at = DateTime::parse_from_rfc3339(&queued_content.last_updated_at).unwrap();

    let full_video_caption = generate_full_video_caption("queued", ui_definitions, tx, video_info);

    if last_updated_at < now - REFRESH_RATE {
        let remove_from_queue_action_text = ui_definitions.buttons.get("remove_from_queue").unwrap();
        let _ = match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
            Ok(_) => {
                let remove_action = [InlineKeyboardButton::callback(remove_from_queue_action_text, format!("remove_from_queue_{}", message_id))];

                if full_video_caption.contains("(Posting now...)") {
                    // We don't want to show the remove button if the video is being posted
                } else {
                    let _msg = bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_action])).await?;
                }
            }
            Err(_) => {
                let new_message = bot.send_video(CHAT_ID, InputFile::url(queued_content.url.clone().parse().unwrap())).caption(full_video_caption.clone()).await?;

                let undo_action = [InlineKeyboardButton::callback(remove_from_queue_action_text, format!("remove_from_queue_{}", new_message.id))];

                tx.save_content_mapping(IndexMap::from([(new_message.id, video_info.clone())]))?;

                if full_video_caption.contains("(Posting now...)") {
                    // We don't want to show the remove button if the video is being posted
                } else {
                    let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                }
            }
        };

        if !tx.load_posted_content().unwrap().iter().any(|content| content.original_shortcode == video_info.original_shortcode) {
            queued_content.last_updated_at = now.to_rfc3339();
            tx.save_content_queue(queued_content.clone())?;
        } else {
            video_info.status = "posted_hidden".to_string();
        }
    } else {
        //println!("No need to update the message");
    }
    Ok((message_id, video_info.clone()))
}

async fn process_posted_hidden(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, mut message_id: MessageId, video_info: &mut ContentInfo, input_file: &InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    //println!("process_posted_hidden - Message ID: {}", message_id);
    video_info.status = "posted_shown".to_string();

    let mut posted_content = tx.get_posted_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();

    let user_settings = tx.load_user_settings().unwrap();
    let now = now_in_my_timezone(user_settings.clone());

    let full_video_caption = generate_full_video_caption("posted", ui_definitions, tx, video_info);
    match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
        Ok(_) => {
            let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
            let remove_from_view_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", message_id))];

            bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_from_view_action])).await?;

            let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info.clone())]);
            tx.save_content_mapping(content_mapping).unwrap();
        }
        Err(_) => {
            let new_message = bot.send_video(CHAT_ID, input_file.to_owned()).caption(full_video_caption.clone()).await.unwrap();

            let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
            let undo_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id))];

            message_id = new_message.id;

            let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
        }
    };
    posted_content.last_updated_at = now.to_rfc3339();
    tx.save_posted_content(posted_content.clone())?;

    Ok((message_id, video_info.clone()))
}

async fn process_rejected_shown(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, video_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    let mut rejected_content = tx.get_rejected_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
    let datetime = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap();
    //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

    let will_expire_at = datetime.checked_add_signed(chrono::Duration::try_seconds(tx.load_user_settings().unwrap().rejected_content_lifespan * 60).unwrap()).unwrap();

    let user_settings = tx.load_user_settings().unwrap();
    if will_expire_at < now_in_my_timezone(user_settings.clone()) {
        video_info.status = "removed_from_view".to_string();
        bot.delete_message(CHAT_ID, message_id).await?;
        //tx.remove_content_info_with_shortcode(rejected_content.original_shortcode.clone())?;
        rejected_content.expired = true;
    } else {
        let now = now_in_my_timezone(user_settings.clone());

        let last_updated_at = DateTime::parse_from_rfc3339(&rejected_content.last_updated_at).unwrap();
        if last_updated_at < now - REFRESH_RATE {
            let full_video_caption = generate_full_video_caption("rejected", ui_definitions, tx, video_info);

            let _ = match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                Ok(_) => {
                    let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                    let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
                    let undo_action = [
                        InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", message_id)),
                        InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", message_id)),
                    ];

                    let _msg = bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                }
                Err(e) => {
                    println!("Error: {}", e);
                    let new_message = bot.send_video(CHAT_ID, InputFile::url(rejected_content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

                    let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                    let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
                    let undo_action = [
                        InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", new_message.id)),
                        InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id)),
                    ];

                    tx.save_content_mapping(IndexMap::from([(new_message.id, video_info.clone())]))?;

                    let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                }
            };
            rejected_content.last_updated_at = now.to_rfc3339();
        }
        tx.save_rejected_content(rejected_content.clone())?;
    }

    Ok((message_id, video_info.clone()))
}

async fn process_posted_shown(bot: &Throttle<Bot>, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, video_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
    //println!("process_posted_shown - Message ID: {}", message_id);
    let mut posted_content = tx.get_posted_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
    let posted_at = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
    //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

    let will_expire_at = posted_at.checked_add_signed(chrono::Duration::try_seconds(tx.load_user_settings().unwrap().posted_content_lifespan * 60).unwrap()).unwrap();

    let user_settings = tx.load_user_settings().unwrap();
    if will_expire_at < now_in_my_timezone(user_settings.clone()) {
        video_info.status = "removed_from_view".to_string();
        bot.delete_message(CHAT_ID, message_id).await?;
        posted_content.expired = true;
        // println!("Posted content has expired");
    } else {
        let now = now_in_my_timezone(user_settings.clone());

        let last_updated_at = DateTime::parse_from_rfc3339(&posted_content.last_updated_at).unwrap();
        if last_updated_at < now - REFRESH_RATE {
            let full_video_caption = generate_full_video_caption("posted", ui_definitions, tx, video_info);
            let _ = match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                Ok(_) => {
                    let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
                    let remove_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", message_id))];

                    let _msg = bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_action])).await?;
                }
                Err(_) => {
                    let new_message = bot.send_video(CHAT_ID, InputFile::url(posted_content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

                    let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
                    let undo_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id))];

                    tx.save_content_mapping(IndexMap::from([(new_message.id, video_info.clone())]))?;

                    let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                }
            };
            posted_content.last_updated_at = now.to_rfc3339();
            tx.save_posted_content(posted_content.clone())?;
        } else {
            //println!("No need to update the message");
        }
    }

    Ok((message_id, video_info.clone()))
}

async fn send_video_and_get_id(bot: &Throttle<Bot>, input_file: InputFile) -> Result<MessageId, Box<dyn Error + Send + Sync>> {
    let video_message = bot.send_video(CHAT_ID, input_file).await?;
    Ok(video_message.id)
}

fn get_action_buttons(ui_definitions: &UIDefinitions, action_keys: &[&str], sent_message_id: MessageId) -> Vec<InlineKeyboardButton> {
    action_keys
        .iter()
        .map(|&key| {
            let action_text = ui_definitions.buttons.get(key).unwrap();
            InlineKeyboardButton::callback(action_text, format!("{}_{}", key, sent_message_id))
        })
        .collect()
}

async fn edit_message_caption_and_markup(bot: &Throttle<Bot>, chat_id: ChatId, message_id: MessageId, caption: String, markup_buttons: Vec<InlineKeyboardButton>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let caption_edit_result = bot.edit_message_caption(chat_id, message_id).caption(caption).await;
    handle_message_is_not_modified_error(caption_edit_result).await?;

    let markup_edit_result = bot.edit_message_reply_markup(chat_id, message_id).reply_markup(InlineKeyboardMarkup::new([markup_buttons])).await;
    handle_message_is_not_modified_error(markup_edit_result).await?;

    Ok(())
}
