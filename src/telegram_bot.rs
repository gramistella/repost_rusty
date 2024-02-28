mod callbacks;
mod commands;
mod helpers;
mod messages;
mod state;

use chrono::DateTime;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use teloxide::types::{InputFile, MessageId};
use teloxide::{
    dispatching::dialogue::InMemStorage,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

use tokio::sync::mpsc::Receiver;
use tokio::time::sleep;

use crate::telegram_bot::helpers::{expire_posted_content, expire_rejected_content};
use crate::telegram_bot::state::{schema, State};
use crate::database::{Database, PostedContent, VideoInfo};
use indexmap::IndexMap;

use tokio::sync::Mutex;
use crate::utils::now_in_my_timezone;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Serialize, Deserialize, Clone)]
struct UIDefinitions {
    buttons: HashMap<String, String>,
    labels: HashMap<String, String>,
}

type BotDialogue = Dialogue<State, InMemStorage<State>>;

const CHAT_ID: ChatId = ChatId(34957918);

pub(crate) async fn run_bot(
    rx: Receiver<(String, String, String, String)>,
    database: Database,
    credentials: HashMap<String, String>,
) {
    let api_token = credentials.get("telegram_api_token").unwrap();
    let bot = Bot::new(api_token);

    let storage = InMemStorage::new();
    let dialogue = BotDialogue::new(storage.clone(), CHAT_ID);

    let ui_definitions_yaml_data = include_str!("../config/ui_definitions.yaml");
    let config: UIDefinitions =
        serde_yaml::from_str(&ui_definitions_yaml_data).expect("Error parsing config file");

    let execution_mutex = Arc::new(Mutex::new(()));

    tokio::select! {
        _ = start_dispatcher(bot.clone(), dialogue.clone(), execution_mutex.clone(), database.clone(), config.clone(), storage.clone()) => {},
        _ = receive_videos(rx, bot, dialogue, execution_mutex, database, config) => {},
    }
}

async fn start_dispatcher(
    bot: Bot,
    dialogue: BotDialogue,
    execution_mutex: Arc<Mutex<()>>,
    database: Database,
    config: UIDefinitions,
    storage: Arc<InMemStorage<State>>,
) {
    let _ = execution_mutex.lock().await;
    let mut dispatcher_builder = Dispatcher::builder(bot.clone(), schema())
        .dependencies(dptree::deps![
            dialogue.clone(),
            storage.clone(),
            execution_mutex.clone(),
            database.clone(),
            config.clone()
        ])
        .enable_ctrlc_handler()
        .build();
    let dispatcher_future = dispatcher_builder.dispatch();
    dispatcher_future.await;
}
async fn receive_videos(
    mut rx: Receiver<(String, String, String, String)>,
    bot: Bot,
    dialogue: BotDialogue,
    execution_mutex: Arc<Mutex<()>>,
    database: Database,
    config: UIDefinitions,
) {
    // Give a head start to the dispatcher
    sleep(Duration::from_secs(1)).await;

    loop {
        let mut tx = database.begin_transaction().unwrap();
        let _ = execution_mutex.lock().await;

        if let Some((received_url, received_caption, original_author, original_shortcode)) =
            rx.recv().await
        {
            // This if statement is not actually needed in production since the scraper will not send the same video twice
            if tx.does_content_exist_with_shortcode(original_shortcode.clone()) == false {
                let re = regex::Regex::new(r"#\w+").unwrap();
                let cloned_caption = received_caption.clone();
                let hashtags: Vec<&str> = re
                    .find_iter(&cloned_caption)
                    .map(|mat| mat.as_str())
                    .collect();
                let mut hashtags = hashtags.join(" ");

                if hashtags.is_empty(){
                    hashtags = "#meme #cringe".to_string();
                }

                let caption = re.replace_all(&received_caption.clone(), "").to_string();

                let video = VideoInfo {
                    url: received_url.clone(),
                    status: "waiting".to_string(),
                    caption,
                    hashtags,
                    original_author: original_author.clone(),
                    original_shortcode: original_shortcode.clone(),
                    encountered_errors: 0,
                };

                let mut tx = database.begin_transaction().unwrap();
                let user_settings = tx.load_user_settings().unwrap();
                let message_id = tx.get_temp_message_id(user_settings);
                let video_mapping: IndexMap<MessageId, VideoInfo> =
                    IndexMap::from([(MessageId(message_id), video.clone())]);

                tx.save_video_info(video_mapping).unwrap();
            };

            //println!("Received URL: \n\"{}\"\ncaption: \n\"{}\"\n", received_url, received_caption);
            let _success = match send_videos(
                bot.clone(),
                dialogue.clone(),
                execution_mutex.clone(),
                database.clone(),
                config.clone(),
            )
                .await
            {
                Ok(..) => {
                    //println!("Video sent successfully")
                }
                Err(e) => {
                    println!("Error sending video in loop: {}", e)
                }
            };
        }
    }
}

async fn send_videos(
    bot: Bot,
    dialogue: BotDialogue,
    execution_mutex: Arc<Mutex<()>>,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    let _mutex_guard = execution_mutex.lock().await;

    let dialogue_state = dialogue
        .get()
        .await
        .unwrap()
        .unwrap_or_else(|| State::ScrapeView);

    if dialogue_state != State::ScrapeView {
        return Ok(());
    }

    let mut tx = database.begin_transaction().unwrap();
    if let Ok(video_mapping) = tx.load_video_mapping() {
        for (message_id, video_info) in video_mapping {
            let mut video = VideoInfo {
                url: video_info.url,
                status: video_info.status,
                caption: video_info.caption,
                hashtags: video_info.hashtags,
                original_author: video_info.original_author,
                original_shortcode: video_info.original_shortcode,
                encountered_errors: video_info.encountered_errors,
            };

            let input_file = InputFile::url(video.url.parse().unwrap());
            let mut full_video_caption = format!(
                "{}\n{}\n(from @{})",
                video.caption, video.hashtags, video.original_author
            );

            if video.encountered_errors > 0 {
                continue;
            }

            if video.status == "posted_shown" {
                let posted_content_list = tx.load_posted_content().unwrap();
                for posted_content in posted_content_list {
                    if posted_content.url == video.url {
                        let datetime =
                            DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
                        //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

                        let will_expire_at = datetime
                            .checked_add_signed(chrono::Duration::seconds(
                                tx.load_user_settings().unwrap().posted_content_lifespan * 60,
                            ))
                            .unwrap();

                        let user_settings = tx.load_user_settings().unwrap();
                        if will_expire_at < now_in_my_timezone(user_settings.clone()) {
                            expire_posted_content(
                                &bot,
                                ui_definitions.clone(),
                                &mut tx,
                                message_id,
                                &mut video,
                            )
                            .await?;
                            continue;
                        }
                    }
                }
            }

            if video.status == "rejected_shown" {
                let posted_content_list = tx.load_rejected_content().unwrap();
                for posted_content in posted_content_list {
                    if posted_content.url == video.url {
                        let datetime =
                            DateTime::parse_from_rfc3339(&posted_content.rejected_at).unwrap();
                        //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

                        let will_expire_at = datetime
                            .checked_add_signed(chrono::Duration::seconds(
                                tx.load_user_settings().unwrap().rejected_content_lifespan * 60,
                            ))
                            .unwrap();

                        let user_settings = tx.load_user_settings().unwrap();
                        if will_expire_at < now_in_my_timezone(user_settings.clone()) {
                            expire_rejected_content(
                                &bot,
                                ui_definitions.clone(),
                                &mut tx,
                                message_id,
                                &mut video,
                            )
                            .await?;
                            continue;
                        }
                    }
                }
            }

            if !(video.status == "waiting"
                || video.status == "pending_hidden"
                || video.status == "posted_hidden"
                || video.status == "accepted_hidden"
                || video.status == "posted_confirmed"
                || video.status == "queued_hidden"
                || video.status == "rejected_hidden")
            {
                continue;
            }

            if video.status == "pending_hidden" {
                video.status = "pending_shown".to_string();
            }

            if video.status == "posted_hidden" {
                video.status = "posted_shown".to_string();
                let queue = tx.load_post_queue().unwrap();

                let mut queue_element = None;
                let mut posted_content_element = None;
                for post in queue {
                    if post.url == video.url {
                        queue_element = Some(post.clone());
                        break;
                    }
                }
                if queue_element == None {
                    let posted_content_list = tx.load_posted_content().unwrap();
                    for posted_content in posted_content_list {
                        if posted_content.url == video.url {
                            posted_content_element = Some(posted_content.clone());
                            break;
                        }
                    }
                    let datetime =
                        DateTime::parse_from_rfc3339(&posted_content_element.unwrap().posted_at)
                            .unwrap();
                    let formatted_datetime = datetime.format("%H:%M %m-%d").to_string();

                    let will_expire_at = datetime
                        .checked_add_signed(chrono::Duration::seconds(
                            tx.load_user_settings().unwrap().posted_content_lifespan * 60,
                        ))
                        .unwrap()
                        .format("%H:%M %m-%d")
                        .to_string();

                    let posted_video_caption =
                        ui_definitions.labels.get("posted_video_caption").unwrap();
                    full_video_caption = format!(
                        "{}\n\n{} at {}\n  Will expire at {}",
                        full_video_caption,
                        posted_video_caption,
                        formatted_datetime,
                        will_expire_at
                    );
                } else {
                    let user_settings = tx.load_user_settings().unwrap();
                    let posted_content = PostedContent {
                        url: queue_element.clone().unwrap().url.clone(),
                        caption: queue_element.clone().unwrap().caption.clone(),
                        hashtags: queue_element.clone().unwrap().caption.clone(),
                        original_author: queue_element.clone().unwrap().original_author.clone(),
                        original_shortcode: queue_element
                            .clone()
                            .unwrap()
                            .original_shortcode
                            .clone(),
                        posted_at: now_in_my_timezone(user_settings).to_rfc3339(),
                        expired: false,
                    };

                    // The matching QueuedPost will be cleared by save_posted_content
                    tx.save_posted_content(posted_content.clone()).unwrap();

                    let datetime = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
                    let formatted_datetime = datetime.format("%H:%M %m-%d").to_string();

                    let will_expire_at = datetime
                        .checked_add_signed(chrono::Duration::seconds(
                            tx.load_user_settings().unwrap().posted_content_lifespan * 60,
                        ))
                        .unwrap()
                        .format("%H:%M %m-%d")
                        .to_string();

                    let posted_video_caption =
                        ui_definitions.labels.get("posted_video_caption").unwrap();
                    full_video_caption = format!(
                        "{}\n\n{} at {}\n  Will expire at {}",
                        full_video_caption,
                        posted_video_caption,
                        formatted_datetime,
                        will_expire_at
                    );
                }

                match bot
                    .edit_message_caption(CHAT_ID, message_id)
                    .caption(full_video_caption.clone())
                    .await
                {
                    Ok(_) => {
                        let remove_from_view_action_text =
                            ui_definitions.buttons.get("remove_from_view").unwrap();
                        let remove_from_view_action = [InlineKeyboardButton::callback(
                            remove_from_view_action_text,
                            format!("remove_from_view_{}", message_id),
                        )];

                        bot.edit_message_reply_markup(CHAT_ID, message_id)
                            .reply_markup(InlineKeyboardMarkup::new([remove_from_view_action]))
                            .await?;

                        let video_mapping: IndexMap<MessageId, VideoInfo> =
                            IndexMap::from([(message_id, video.clone())]);
                        tx.save_video_info(video_mapping).unwrap();
                    }
                    Err(_) => {
                        let video_message = bot
                            .send_video(CHAT_ID, input_file)
                            .caption(full_video_caption.clone())
                            .await
                            .unwrap();

                        let remove_from_view_action_text =
                            ui_definitions.buttons.get("remove_from_view").unwrap();
                        let remove_from_view_action = [InlineKeyboardButton::callback(
                            remove_from_view_action_text,
                            format!("remove_from_view_{}", video_message.id),
                        )];

                        bot.edit_message_reply_markup(CHAT_ID, video_message.id)
                            .reply_markup(InlineKeyboardMarkup::new([remove_from_view_action]))
                            .await?;

                        let video_mapping: IndexMap<MessageId, VideoInfo> =
                            IndexMap::from([(video_message.id, video.clone())]);

                        tx.save_video_info(video_mapping).unwrap();
                    }
                }
                continue;
            }

            if video.status == "queued_hidden" {
                video.status = "queued_shown".to_string();

                let queue = tx.load_post_queue().unwrap();

                let mut queued_post = None;
                for post in queue {
                    if post.url == video.url {
                        queued_post = Some(post.clone());
                        break;
                    }
                }
                let datetime =
                    DateTime::parse_from_rfc3339(&queued_post.unwrap().will_post_at).unwrap();
                let formatted_datetime = datetime.format("%H:%M %m-%d").to_string();

                let queued_video_caption =
                    ui_definitions.labels.get("queued_video_caption").unwrap();
                full_video_caption = format!(
                    "{}\n\n{}, will post at {}",
                    full_video_caption, queued_video_caption, formatted_datetime
                );
                match bot
                    .edit_message_caption(CHAT_ID, message_id)
                    .caption(full_video_caption.clone())
                    .await
                {
                    Ok(_) => {
                        let remove_from_queue_action_text =
                            ui_definitions.buttons.get("remove_from_queue").unwrap();
                        let remove_from_queue_action = [InlineKeyboardButton::callback(
                            remove_from_queue_action_text,
                            format!("remove_from_queue_{}", message_id),
                        )];

                        bot.edit_message_reply_markup(CHAT_ID, message_id)
                            .reply_markup(InlineKeyboardMarkup::new([remove_from_queue_action]))
                            .await?;

                        let video_mapping: IndexMap<MessageId, VideoInfo> =
                            IndexMap::from([(message_id, video.clone())]);
                        tx.save_video_info(video_mapping).unwrap();
                    }
                    Err(_) => {
                        let video_message = bot
                            .send_video(CHAT_ID, input_file)
                            .caption(full_video_caption.clone())
                            .await
                            .unwrap();

                        let remove_from_queue_action_text =
                            ui_definitions.buttons.get("remove_from_queue").unwrap();
                        let remove_from_queue_action = [InlineKeyboardButton::callback(
                            remove_from_queue_action_text,
                            format!("remove_from_queue_{}", video_message.id),
                        )];

                        bot.edit_message_reply_markup(CHAT_ID, video_message.id)
                            .reply_markup(InlineKeyboardMarkup::new([remove_from_queue_action]))
                            .await?;

                        let video_mapping: IndexMap<MessageId, VideoInfo> =
                            IndexMap::from([(video_message.id, video.clone())]);

                        tx.save_video_info(video_mapping).unwrap();
                    }
                }

                continue;
            }

            if video.status == "rejected_hidden" {
                video.status = "rejected_shown".to_string();

                let _did_expire = expire_rejected_content(
                    &bot,
                    ui_definitions.clone(),
                    &mut tx,
                    message_id,
                    &mut video,
                )
                .await?;

                continue;
            }

            if video.status == "posted_hidden" {
                video.status = "posted_shown".to_string();

                let _did_expire = expire_posted_content(
                    &bot,
                    ui_definitions.clone(),
                    &mut tx,
                    message_id,
                    &mut video,
                )
                .await?;

                continue;
            }

            //println!("video.status: {}", video.status);

            let video_message = match bot
                .send_video(CHAT_ID, input_file)
                //.caption("Loading...")
                .await
            {
                Ok(video_message) => video_message,
                Err(e) => {
                    if e.to_string().contains("message is not modified") {
                        println!("warning! message is not modified: {}", message_id);
                        continue;
                    } else if e.to_string().contains("failed to get HTTP URL content") {
                        println!("error! failed to get HTTP URL content: {}", video.url);
                        video.encountered_errors += 1;
                        let mut tx = database.begin_transaction().unwrap();
                        tx.save_video_info(IndexMap::from([(message_id, video.clone())]))
                            .unwrap();
                        continue;
                    } else if e
                        .to_string()
                        .contains("wrong file identifier/HTTP URL specified")
                    {
                        println!(
                            "error! wrong file identifier/HTTP URL specified: {}",
                            video.url
                        );
                        video.encountered_errors += 1;
                        let mut tx = database.begin_transaction().unwrap();
                        tx.save_video_info(IndexMap::from([(message_id, video.clone())]))
                            .unwrap();
                        continue;
                    } else {
                        println!("Unknown error: {}", e);
                        return Err(Box::new(e));
                    }
                }
            };

            let sent_message_id = video_message.id;

            let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
            let reject_action_text = ui_definitions.buttons.get("reject").unwrap();
            let edit_action_text = ui_definitions.buttons.get("edit").unwrap();

            let video_actions = [
                InlineKeyboardButton::callback(
                    accept_action_text,
                    format!("accept_{}", sent_message_id),
                ),
                InlineKeyboardButton::callback(
                    reject_action_text,
                    format!("reject_{}", sent_message_id),
                ),
                InlineKeyboardButton::callback(
                    edit_action_text,
                    format!("edit_{}", sent_message_id),
                ),
            ];

            let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
            let undo_action = [InlineKeyboardButton::callback(
                undo_action_text,
                format!("undo_{}", sent_message_id),
            )];

            if video.status == "accepted_hidden" {
                video.status = "accepted_shown".to_string();
                let accepted_caption_text = ui_definitions.labels.get("accepted_caption").unwrap();

                full_video_caption = format!("{}\n\n{}", full_video_caption, accepted_caption_text);

                let _edited_caption = bot
                    .edit_message_caption(CHAT_ID, sent_message_id)
                    .caption(full_video_caption)
                    .await?;

                let _edited_markup = bot
                    .edit_message_reply_markup(CHAT_ID, sent_message_id)
                    .reply_markup(InlineKeyboardMarkup::new([undo_action]))
                    .await?;

                let video_mapping: IndexMap<MessageId, VideoInfo> =
                    IndexMap::from([(sent_message_id, video.clone())]);
                tx.save_video_info(video_mapping).unwrap();
                continue;
            }

            if video.status == "rejected_shown" {
                let mut video_caption = String::new();
                if video.status == "accepted_shown" {
                    let accepted_caption_text = ui_definitions.buttons.get("accepted").unwrap();
                    video_caption = format!("{}: {}", accepted_caption_text, full_video_caption);
                }

                full_video_caption = format!("{}: {}", video_caption, full_video_caption);

                let _edited_caption = bot
                    .edit_message_caption(CHAT_ID, sent_message_id)
                    .caption(full_video_caption)
                    .await?;

                let _edited_markup = bot
                    .edit_message_reply_markup(CHAT_ID, sent_message_id)
                    .reply_markup(InlineKeyboardMarkup::new([undo_action]))
                    .await?;
            } else if video.status == "posted_shown" {
                panic!("posted_shown");
            } else {
                //println!("Video status: {}, video url: {}", video.status, video.url);
                video.status = "pending_shown".to_string();
                match bot
                    .edit_message_caption(CHAT_ID, sent_message_id)
                    .caption(full_video_caption)
                    .await
                {
                    Ok(_message) => {
                        //println!("Sent message to telegram with ID: {}", sent_message_id);
                    }
                    Err(e) => {
                        if e.to_string().contains("message is not modified") {
                            println!("warning! message is not modified: {}", message_id);
                        } else {
                            println!("Error: {}", e);
                            return Err(Box::new(e));
                        }
                    }
                };

                let _edited_markup = bot
                    .edit_message_reply_markup(CHAT_ID, sent_message_id)
                    .reply_markup(InlineKeyboardMarkup::new([video_actions]))
                    .await?;
            }

            //println!("Sent message to telegram with ID: {}", sent_message_id);
            let video_mapping: IndexMap<MessageId, VideoInfo> =
                IndexMap::from([(sent_message_id, video.clone())]);
            tx.save_video_info(video_mapping).unwrap();
            // NOTE
            //sleep(Duration::from_millis(250)).await;
        }
    }
    Ok(())
}
