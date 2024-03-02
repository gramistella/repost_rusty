mod callbacks;
mod commands;
mod helpers;
mod messages;
mod state;

use chrono::DateTime;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use teloxide::types::{InputFile, MessageId};
use teloxide::{
    dispatching::dialogue::InMemStorage,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
    RequestError,
};

use tokio::sync::mpsc::Receiver;
use tokio::time::sleep;

use crate::database::{Database, DatabaseTransaction, PostedContent, QueuedContent, UserSettings, VideoInfo};
use crate::telegram_bot::state::{schema, State};
use indexmap::IndexMap;

use crate::utils::now_in_my_timezone;
use tokio::sync::Mutex;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Serialize, Deserialize, Clone)]
struct UIDefinitions {
    buttons: HashMap<String, String>,
    labels: HashMap<String, String>,
}

type BotDialogue = Dialogue<State, InMemStorage<State>>;

const CHAT_ID: ChatId = ChatId(34957918);
const REFRESH_RATE: Duration = Duration::from_secs(30);

pub(crate) async fn run_bot(rx: Receiver<(String, String, String, String)>, database: Database, credentials: HashMap<String, String>) {
    let api_token = credentials.get("telegram_api_token").unwrap();
    let bot = Bot::new(api_token);

    let storage = InMemStorage::new();
    let dialogue = BotDialogue::new(storage.clone(), CHAT_ID);

    let ui_definitions_yaml_data = include_str!("../config/ui_definitions.yaml");
    let config: UIDefinitions = serde_yaml::from_str(&ui_definitions_yaml_data).expect("Error parsing config file");

    let execution_mutex = Arc::new(Mutex::new(()));

    tokio::select! {
        _ = start_dispatcher(bot.clone(), dialogue.clone(), execution_mutex.clone(), database.clone(), config.clone(), storage.clone()) => {},
        _ = receive_videos(rx, bot, dialogue, execution_mutex, database, config) => {},
    }
}

async fn start_dispatcher(bot: Bot, dialogue: BotDialogue, execution_mutex: Arc<Mutex<()>>, database: Database, config: UIDefinitions, storage: Arc<InMemStorage<State>>) {
    let _ = execution_mutex.lock().await;
    let mut dispatcher_builder = Dispatcher::builder(bot.clone(), schema())
        .dependencies(dptree::deps![dialogue.clone(), storage.clone(), execution_mutex.clone(), database.clone(), config.clone()])
        .enable_ctrlc_handler()
        .build();
    let dispatcher_future = dispatcher_builder.dispatch();
    dispatcher_future.await;
}
async fn receive_videos(mut rx: Receiver<(String, String, String, String)>, bot: Bot, dialogue: BotDialogue, execution_mutex: Arc<Mutex<()>>, database: Database, config: UIDefinitions) {
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
                let mut hashtags = hashtags.join(" ");

                if hashtags.is_empty() {
                    hashtags = "#meme #cringe".to_string();
                }

                let caption = re.replace_all(&received_caption.clone(), "").to_string();
                let user_settings = tx.load_user_settings().unwrap();

                let video = VideoInfo {
                    url: received_url.clone(),
                    status: "waiting".to_string(),
                    caption,
                    hashtags,
                    original_author: original_author.clone(),
                    original_shortcode: original_shortcode.clone(),
                    last_updated_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
                    encountered_errors: 0,
                };

                let message_id = tx.get_temp_message_id(user_settings);
                let content_mapping: IndexMap<MessageId, VideoInfo> = IndexMap::from([(MessageId(message_id), video.clone())]);

                tx.save_content_mapping(content_mapping).unwrap();
            };

            //println!("Received URL: \n\"{}\"\ncaption: \n\"{}\"\n", received_url, received_caption);
        }
        let _success = match send_videos(bot.clone(), dialogue.clone(), execution_mutex.clone(), database.clone(), config.clone()).await {
            Ok(..) => {
                //println!("Video sent successfully")
            }
            Err(e) => {
                println!("Error sending video in loop: {}", e)
            }
        };
    }
}

async fn send_videos(bot: Bot, dialogue: BotDialogue, execution_mutex: Arc<Mutex<()>>, database: Database, ui_definitions: UIDefinitions) -> HandlerResult {
    let _mutex_guard = execution_mutex.lock().await;

    let dialogue_state = dialogue.get().await.unwrap().unwrap_or_else(|| State::ScrapeView);

    if dialogue_state != State::ScrapeView {
        return Ok(());
    }

    let mut tx = database.begin_transaction().unwrap();
    if let Ok(video_mapping) = tx.load_content_mapping() {
        for (message_id, mut video_info) in video_mapping {
            let input_file = InputFile::url(video_info.url.parse().unwrap());
            let full_video_caption = format!("{}\n{}\n(from @{})", video_info.caption, video_info.hashtags, video_info.original_author);

            if video_info.encountered_errors > 0 {
                continue;
            }
            let result = match video_info.status.as_str() {
                "waiting" => process_waiting(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, input_file, full_video_caption).await,
                "pending_hidden" => process_pending_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, input_file, full_video_caption).await,
                "pending_shown" => process_pending_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, input_file, full_video_caption).await,
                "posted_hidden" => process_posted_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, &input_file, full_video_caption).await,
                "posted_shown" => process_posted_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "accepted_hidden" => process_accepted_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, input_file, full_video_caption).await,
                "accepted_shown" => process_accepted_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "queued_hidden" => process_queued_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, &input_file, full_video_caption).await,
                "queued_shown" => process_queued_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "rejected_hidden" => process_rejected_hidden(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "rejected_shown" => process_rejected_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info).await,
                "failed_hidden" => process_failed_hidden(&bot, &ui_definitions, &mut video_info, input_file, full_video_caption).await,
                "failed_shown" => process_failed_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, full_video_caption).await,
                "removed_from_view" => Ok((message_id, video_info)),
                _ => {
                    println!("Unknown status: {}", video_info.status);
                    println!("Video info: {:?}", video_info);
                    Ok((message_id, video_info))
                }
            };

            match result {
                Ok((message_id, video_info)) => {
                    let content_mapping: IndexMap<MessageId, VideoInfo> = IndexMap::from([(message_id, video_info)]);
                    tx.save_content_mapping(content_mapping).unwrap();
                }
                Err(e) => {
                    println!("Error: {}", e);
                }
            }
        }
    }
    Ok(())
}

async fn process_failed_shown(bot: &Bot, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    let failed_content = tx.load_failed_content().unwrap();
    for mut content in failed_content {
        if content.original_shortcode == video_info.original_shortcode {
            // Parse content failed at into a DateTime
            let datetime = DateTime::parse_from_rfc3339(&content.last_updated_at)?;
            let now = now_in_my_timezone(tx.load_user_settings()?);
            // Check
            if now > datetime {
                let video_actions = get_action_buttons(ui_definitions, &["remove_from_view"], message_id);
                video_info.status = "failed_shown".to_string();
                let full_video_caption = format!("{}\n\n{}", full_video_caption, ui_definitions.labels.get("failed_caption").unwrap());
                edit_message_caption_and_markup(&bot, CHAT_ID, message_id, full_video_caption, video_actions).await?;

                content.last_updated_at = (now + Duration::from_secs(60)).to_rfc3339();
                tx.update_failed_content(content.clone())?;

                return Ok((message_id, video_info.clone()));
            }
        }
    }

    Ok((message_id, video_info.clone()))
}

async fn process_failed_hidden(bot: &Bot, ui_definitions: &UIDefinitions, mut video_info: &mut VideoInfo, input_file: InputFile, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    let sent_message_id = send_video_and_get_id(&bot, input_file).await?;
    let video_actions = get_action_buttons(ui_definitions, &["remove_from_view"], sent_message_id);
    video_info.status = "failed_shown".to_string();
    let full_video_caption = format!("{}\n\n{}", full_video_caption, ui_definitions.labels.get("failed_caption").unwrap());
    edit_message_caption_and_markup(&bot, CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
    Ok((sent_message_id, video_info.clone()))
}

async fn process_pending_shown(bot: &Bot, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo, input_file: InputFile, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    return Ok((message_id, video_info.clone()));
}

async fn process_pending_hidden(bot: &Bot, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo, input_file: InputFile, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    let sent_message_id = send_video_and_get_id(&bot, input_file).await?;
    let video_actions = get_action_buttons(ui_definitions, &["accept", "reject", "edit"], sent_message_id);
    video_info.status = "pending_shown".to_string();
    edit_message_caption_and_markup(&bot, CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
    Ok((sent_message_id, video_info.clone()))
}

async fn process_waiting(bot: &Bot, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo, input_file: InputFile, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    let sent_message_id = send_video_and_get_id(&bot, input_file).await?;
    let video_actions = get_action_buttons(ui_definitions, &["accept", "reject", "edit"], sent_message_id);
    video_info.status = "pending_shown".to_string();
    edit_message_caption_and_markup(&bot, CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
    Ok((sent_message_id, video_info.clone()))
}

async fn process_accepted_hidden(bot: &Bot, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo, input_file: InputFile, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    // Use the helper function to send the video and get the message ID.
    let sent_message_id = send_video_and_get_id(bot, input_file).await?;

    //println!("process_accepted_hidden - Sent message ID: {}", sent_message_id);

    // Use the helper function to get the action buttons.
    let undo_action = get_action_buttons(ui_definitions, &["undo"], sent_message_id);

    // Update the video info status.
    video_info.status = "accepted_shown".to_string();

    // Update the caption.
    let accepted_caption_text = ui_definitions.labels.get("accepted_caption").unwrap();

    full_video_caption = format!("{}\n\n{}", full_video_caption, accepted_caption_text);

    // Use the helper function to edit the message caption and markup.
    edit_message_caption_and_markup(bot, CHAT_ID, sent_message_id, full_video_caption, undo_action).await?;

    Ok((sent_message_id, video_info.clone()))
}

async fn process_accepted_shown(bot: &Bot, ui_definitions: &UIDefinitions, mut tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    //println!("process_accepted_shown - Message ID: {}", message_id);
    /*
    let mut content_list = tx.load_video_mapping().unwrap();
    for (_message_id, mut content) in &mut content_list {
        if content.original_shortcode == video_info.original_shortcode {
            let accepted_caption = ui_definitions.labels.get("accepted_caption").unwrap();
            let full_video_caption = format!("{}\n{}\n(from @{})\n\n{}", content.caption, content.hashtags, content.original_author, accepted_caption,);

            let _ = match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                Ok(_) => {
                    let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                    let undo_action = [InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", message_id))];

                    let _msg = bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                }
                Err(e) => {
                    println!("Error: {}", e);
                    let new_message = bot.send_video(CHAT_ID, InputFile::url(content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

                    let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                    let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
                    let undo_action = [
                        InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", new_message.id)),
                        InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id)),
                    ];

                    tx.save_video_info(IndexMap::from([(new_message.id, video_info.clone())]))?;

                    let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                }
            };
        }
    }
    */

    // tx.save_video_info(IndexMap::from([(message_id, video_info.clone())]))?;
    Ok((message_id, video_info.clone()))
}

async fn process_rejected_hidden(bot: &Bot, ui_definitions: &UIDefinitions, mut tx: &mut DatabaseTransaction, mut message_id: MessageId, mut video_info: &mut VideoInfo) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    video_info.status = "rejected_shown".to_string();

    let rejected_content = tx.load_rejected_content()?;
    let mut did_expire = false;

    let user_settings = tx.load_user_settings()?;
    let expiry_duration = Duration::from_secs((user_settings.rejected_content_lifespan * 60) as u64);
    let now = now_in_my_timezone(user_settings);

    for mut content in rejected_content {
        let datetime = DateTime::parse_from_rfc3339(&content.rejected_at)?;

        if content.url == video_info.url && !content.expired {
            if now > datetime + expiry_duration {
                video_info.status = "removed_from_view".to_string();
                bot.delete_message(CHAT_ID, message_id).await?;
                did_expire = true;
                content.expired = true;
                tx.save_rejected_content(content.clone())?;
                tx.remove_content_info_with_shortcode(content.original_shortcode.clone())?;
                //println!("Permanently removed post from video_info: {}", content.url);
            } else {
                video_info.status = "rejected_shown".to_string();
                let expiry_date = datetime + expiry_duration;

                let duration_until_expiration = expiry_date.signed_duration_since(now);
                let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
                let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
                let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

                let rejected_caption = ui_definitions.labels.get("rejected_caption").unwrap();
                let full_video_caption = format!(
                    "{}\n{}\n(from @{})\n\n{}\n\nWill expire in {} hours, {}, minutes and {} seconds",
                    content.caption, content.hashtags, content.original_author, rejected_caption, hours_until_expiration, minutes_until_expiration, seconds_until_expiration
                );

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
        }
    }

    Ok((message_id, video_info.clone()))
}

async fn process_queued_hidden(bot: &Bot, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo, input_file: &InputFile, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    video_info.status = "queued_shown".to_string();

    // Extract the queued post information
    let queued_post = match tx.load_content_queue().unwrap().into_iter().find(|post| post.original_shortcode == video_info.original_shortcode) {
        Some(post) => Some(post),
        None => None,
    };

    if queued_post == None {
        let posted_content = tx.load_content_queue().unwrap().into_iter().find(|post| post.original_shortcode == video_info.original_shortcode).expect("No queued or uploaded post found");

        let (message_id, video_info) = process_posted_hidden(bot, ui_definitions, tx, message_id, video_info, input_file, full_video_caption).await?;
        return Ok((message_id, video_info));
    }

    let queued_post = queued_post.unwrap();

    let will_post_at = DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap();
    // calculate difference between now and will_post_at
    let user_settings = tx.load_user_settings().unwrap();
    let now = now_in_my_timezone(user_settings);
    let duration_until_expiration = will_post_at.signed_duration_since(now);

    let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
    let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
    let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

    let formatted_datetime = will_post_at.format("%H:%M %m/%d").to_string();

    let queued_video_caption = ui_definitions.labels.get("queued_caption").unwrap();
    full_video_caption = format!(
        "{}\n\n{}\n\nWill post at {}\n({} hours, {} minutes and {} seconds from now)",
        full_video_caption, queued_video_caption, formatted_datetime, hours_until_expiration, minutes_until_expiration, seconds_until_expiration
    );

    // Attempt to edit the existing message caption
    match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
        Ok(_) => {
            let remove_from_queue_action = get_action_buttons(ui_definitions, &["remove_from_queue"], message_id);
            edit_message_caption_and_markup(bot, CHAT_ID, message_id, full_video_caption, remove_from_queue_action).await?;

            let content_mapping: IndexMap<MessageId, VideoInfo> = IndexMap::from([(message_id, video_info.clone())]);
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

async fn process_queued_shown(bot: &Bot, ui_definitions: &UIDefinitions, tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    //println!("process_queued_shown - Message ID: {}", message_id);
    let mut queued_content_list = tx.load_content_queue().unwrap();
    for mut queued_content in &mut queued_content_list {
        if queued_content.original_shortcode == video_info.original_shortcode {
            let will_post_at = DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap();
            //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();
            let user_settings = tx.load_user_settings().unwrap();
            let now = now_in_my_timezone(user_settings);
            let duration_until_expiration = will_post_at.signed_duration_since(now);
            let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
            let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
            let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

            let last_updated_at = DateTime::parse_from_rfc3339(&queued_content.last_updated_at).unwrap();
            let posted_caption = ui_definitions.labels.get("queued_caption").unwrap();
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
                queued_content.caption, queued_content.hashtags, queued_content.original_author, posted_caption, formatted_datetime, countdown_caption
            );

            if last_updated_at < now - REFRESH_RATE {
                let remove_from_queue_action_text = ui_definitions.buttons.get("remove_from_queue").unwrap();
                let _ = match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                    Ok(_) => {
                        let remove_action = [InlineKeyboardButton::callback(remove_from_queue_action_text, format!("remove_from_queue_{}", message_id))];

                        let _msg = bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_action])).await?;
                    }
                    Err(_) => {
                        let new_message = bot.send_video(CHAT_ID, InputFile::url(queued_content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

                        let undo_action = [InlineKeyboardButton::callback(remove_from_queue_action_text, format!("remove_from_queue_{}", new_message.id))];

                        tx.save_content_mapping(IndexMap::from([(new_message.id, video_info.clone())]))?;

                        let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
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
        }
    }
    Ok((message_id, video_info.clone()))
}

async fn process_posted_hidden(bot: &Bot, ui_definitions: &UIDefinitions, mut tx: &mut DatabaseTransaction, mut message_id: MessageId, mut video_info: &mut VideoInfo, input_file: &InputFile, mut full_video_caption: String) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    //println!("process_posted_hidden - Message ID: {}", message_id);
    video_info.status = "posted_shown".to_string();
    let mut posted_content_list = tx.load_posted_content().unwrap();
    for mut posted_content in &mut posted_content_list {
        if posted_content.original_shortcode == video_info.original_shortcode {
            let datetime = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
            let will_expire_at = datetime.checked_add_signed(chrono::Duration::seconds(tx.load_user_settings().unwrap().posted_content_lifespan * 60)).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            let now = now_in_my_timezone(user_settings);
            let duration_until_expiration = will_expire_at.signed_duration_since(now);
            let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
            let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
            let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

            let posted_caption = ui_definitions.labels.get("posted_caption").unwrap();
            let formatted_datetime = datetime.format("%H:%M %m/%d").to_string();
            full_video_caption = format!(
                "{}\n{}\n(from @{})\n\n{} at {}\n\nWill expire in {} hours, {} minutes and {} seconds",
                posted_content.caption, posted_content.hashtags, posted_content.original_author, posted_caption, formatted_datetime, hours_until_expiration, minutes_until_expiration, seconds_until_expiration
            );

            match bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                Ok(_) => {
                    let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
                    let remove_from_view_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", message_id))];

                    bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_from_view_action])).await?;

                    let content_mapping: IndexMap<MessageId, VideoInfo> = IndexMap::from([(message_id, video_info.clone())]);
                    tx.save_content_mapping(content_mapping).unwrap();
                }
                Err(_) => {
                    let new_message = bot.send_video(CHAT_ID, input_file.to_owned()).caption(full_video_caption.clone()).await.unwrap();

                    let remove_from_view_action_text = ui_definitions.buttons.get("remove_from_view").unwrap();
                    let undo_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id))];

                    message_id = new_message.id;
                    //tx.save_video_info(IndexMap::from([(new_message.id, video_info.clone())]))?;

                    let _msg = bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                }
            };
            posted_content.last_updated_at = now.to_rfc3339();
            tx.save_posted_content(posted_content.clone())?;
        }
    }
    Ok((message_id, video_info.clone()))
}

async fn update_reply_markup(bot: &Bot, message_id: MessageId, ui_definitions: &UIDefinitions, button_key: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let action_text = ui_definitions.buttons.get(button_key).unwrap();
    let action = [InlineKeyboardButton::callback(action_text, format!("{}_{}", button_key, message_id))];
    bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([action])).await?;
    Ok(())
}

enum ContentType {
    QueuedContent(QueuedContent),
    PostedContent(PostedContent),
}

fn get_content_details(content: ContentType, user_settings: &UserSettings, ui_definitions: &UIDefinitions) -> Option<(String, String)> {
    match content {
        ContentType::QueuedContent(queued_post) => {
            let datetime = DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap();
            let formatted_datetime = datetime.format("%H:%M %m-%d").to_string();
            let caption_label = ui_definitions.labels.get("queued_caption").unwrap();
            Some((caption_label.to_string(), formatted_datetime))
        }
        ContentType::PostedContent(posted_content) => {
            let datetime = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
            let formatted_datetime = datetime.format("%H:%M %m-%d").to_string();
            let will_expire_at = datetime.checked_add_signed(chrono::Duration::seconds(user_settings.posted_content_lifespan * 60)).unwrap().format("%H:%M %m-%d").to_string();
            let caption_label = ui_definitions.labels.get("posted_caption").unwrap();
            Some((format!("{} at {}\n  Will expire at {}", caption_label, formatted_datetime, will_expire_at), formatted_datetime))
        }
    }
}

async fn process_rejected_shown(bot: &Bot, ui_definitions: &UIDefinitions, mut tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    let mut rejected_content_list = tx.load_rejected_content().unwrap();
    for mut rejected_content in &mut rejected_content_list {
        if rejected_content.original_shortcode == video_info.original_shortcode {
            let datetime = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap();
            //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

            let will_expire_at = datetime.checked_add_signed(chrono::Duration::seconds(tx.load_user_settings().unwrap().rejected_content_lifespan * 60)).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            if will_expire_at < now_in_my_timezone(user_settings.clone()) {
                let mut did_expire = false;

                let user_settings = tx.load_user_settings()?;
                let expiry_duration = Duration::from_secs((user_settings.rejected_content_lifespan * 60) as u64);
                let now = now_in_my_timezone(user_settings);

                video_info.status = "removed_from_view".to_string();
                bot.delete_message(CHAT_ID, message_id).await?;
                did_expire = true;
                rejected_content.expired = true;
            } else {
                let now = now_in_my_timezone(user_settings);
                let duration_until_expiration = will_expire_at.signed_duration_since(now);
                let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
                let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
                let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

                let last_updated_at = DateTime::parse_from_rfc3339(&rejected_content.last_updated_at).unwrap();
                if last_updated_at < now - REFRESH_RATE {
                    let rejected_caption = ui_definitions.labels.get("rejected_caption").unwrap();
                    let full_video_caption = format!(
                        "{}\n{}\n(from @{})\n\n{}\n\nWill expire in {} hours, {} minutes and {} seconds",
                        rejected_content.caption, rejected_content.hashtags, rejected_content.original_author, rejected_caption, hours_until_expiration, minutes_until_expiration, seconds_until_expiration
                    );

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
                    tx.save_rejected_content(rejected_content.clone())?;
                } else {
                    //println!("No need to update the message");
                }
            }
        }
    }

    Ok((message_id, video_info.clone()))
}

async fn process_posted_shown(bot: &Bot, ui_definitions: &UIDefinitions, mut tx: &mut DatabaseTransaction, message_id: MessageId, mut video_info: &mut VideoInfo) -> Result<(MessageId, VideoInfo), Box<dyn Error + Send + Sync>> {
    //println!("process_posted_shown - Message ID: {}", message_id);
    let mut posted_content_list = tx.load_posted_content().unwrap();
    for mut posted_content in &mut posted_content_list {
        if posted_content.original_shortcode == video_info.original_shortcode {
            let datetime = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
            //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

            let will_expire_at = datetime.checked_add_signed(chrono::Duration::seconds(tx.load_user_settings().unwrap().posted_content_lifespan * 60)).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            if will_expire_at < now_in_my_timezone(user_settings.clone()) {
                let mut did_expire = false;

                let user_settings = tx.load_user_settings()?;
                let expiry_duration = Duration::from_secs((user_settings.posted_content_lifespan * 60) as u64);
                let now = now_in_my_timezone(user_settings);

                video_info.status = "removed_from_view".to_string();
                bot.delete_message(CHAT_ID, message_id).await?;
                did_expire = true;
                posted_content.expired = true;
            } else {
                let now = now_in_my_timezone(user_settings);
                let duration_until_expiration = will_expire_at.signed_duration_since(now);
                let hours_until_expiration = format!("{:01}", duration_until_expiration.num_hours());
                let minutes_until_expiration = format!("{:01}", duration_until_expiration.num_minutes() % 60);
                let seconds_until_expiration = format!("{:01}", duration_until_expiration.num_seconds() % 60);

                let last_updated_at = DateTime::parse_from_rfc3339(&posted_content.last_updated_at).unwrap();
                if last_updated_at < now - REFRESH_RATE {
                    let posted_caption = ui_definitions.labels.get("posted_caption").unwrap();
                    let full_video_caption = format!(
                        "{}\n{}\n(from @{})\n\n{}\n\nWill expire in {} hours, {} minutes and {} seconds",
                        posted_content.caption, posted_content.hashtags, posted_content.original_author, posted_caption, hours_until_expiration, minutes_until_expiration, seconds_until_expiration
                    );

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
        }
    }
    Ok((message_id, video_info.clone()))
}

async fn send_video_and_get_id(bot: &Bot, input_file: InputFile) -> Result<MessageId, Box<dyn Error + Send + Sync>> {
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

async fn edit_message_caption_and_markup(bot: &Bot, chat_id: ChatId, message_id: MessageId, caption: String, markup_buttons: Vec<InlineKeyboardButton>) -> Result<(), Box<dyn Error + Send + Sync>> {
    match bot.edit_message_caption(chat_id, message_id).caption(caption).await {
        Ok(_) => {}
        Err(e) => {
            if e.to_string().contains("message is not modified") {
            } else {
                return Err(Box::new(e));
            }
        }
    }

    match bot.edit_message_reply_markup(chat_id, message_id).reply_markup(InlineKeyboardMarkup::new([markup_buttons])).await {
        Ok(_) => {}
        Err(e) => {
            if e.to_string().contains("message is not modified") {
            } else {
                return Err(Box::new(e));
            }
        }
    }

    Ok(())
}
