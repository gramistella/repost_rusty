
use chrono::{DateTime, Timelike};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
use teloxide::types::{InputFile, MessageId};
use teloxide::{
    dispatching::{dialogue, dialogue::InMemStorage, UpdateHandler},
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
    utils::command::BotCommands,
};

use tokio::sync::mpsc::Receiver;
use tokio::time::sleep;

use crate::utils::{Database, DatabaseTransaction, PostedContent, RejectedContent, VideoInfo};
use indexmap::IndexMap;

use tokio::sync::Mutex;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Clone, Default, PartialEq)]
pub enum State {
    #[default]
    ScrapeView,
    SettingsView {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    AcceptedView,
    RejectedView,
    ReceivePostingInterval {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveRandomInterval {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveRejectedContentLifespan {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceivePostedContentLifespan {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveCaption {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveHashtags {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    EditView {
        stored_messages_to_delete: Vec<MessageId>,
    },
}

/// These commands are supported:
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    /// Display this text.
    Start,
    Help,
    Settings,
    Restore,
}
#[derive(Serialize, Deserialize, Clone)]
struct UIDefinitions {
    buttons: HashMap<String, String>,
    labels: HashMap<String, String>,
}

type BotDialogue = Dialogue<State, InMemStorage<State>>;

const CHAT_ID: ChatId = ChatId(34957918);

pub(crate) async fn run_bot(mut rx: Receiver<(String, String, String, String)>, database: Database, credentials: HashMap<String, String>) {
    let api_token = credentials.get("telegram_bot_token").unwrap();
    let bot = Bot::new(api_token);

    let storage = InMemStorage::new();
    let dialogue = BotDialogue::new(storage.clone(), CHAT_ID);

    let mut file =
        File::open("src/telegram_bot/ui_definitions.yaml").expect("Unable to open config file");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("Unable to read the file");
    let config: UIDefinitions = serde_yaml::from_str(&contents).expect("Error parsing config file");

    let execution_mutex = Arc::new(Mutex::new(()));

    tokio::select! {
        _ = async {

            let  _ = execution_mutex.lock().await;
            let mut dispatcher_builder = Dispatcher::builder(bot.clone(), schema())
                .dependencies(dptree::deps![dialogue.clone(), storage.clone(), execution_mutex.clone(), database.clone(), config.clone()])
                .enable_ctrlc_handler()
                .build();
            let dispatcher_future = dispatcher_builder.dispatch();
            dispatcher_future.await;

        } => {},
        _ = async {
            sleep(Duration::from_secs(1)).await;
            loop {


                let mut tx = database.begin_transaction().unwrap();
                let _ = execution_mutex.lock().await;

                if let Some((received_url, received_caption, original_author, original_shortcode)) = rx.recv().await {

                    if tx.does_content_exist_with_shortcode(original_shortcode.clone()) == false {
                        let video = VideoInfo {
                            url: received_url.clone(),
                            status: "waiting".to_string(),
                            caption: received_caption.clone(),
                            hashtags: "#meme #cringe".to_string(),
                            original_author: original_author.clone(),
                            original_shortcode: original_shortcode.clone(),
                            encountered_errors: 0,
                        };

                        let timestamp = chrono::Utc::now().num_seconds_from_midnight() as i32;
                        let video_mapping: IndexMap<MessageId, VideoInfo> = IndexMap::from([(MessageId(timestamp), video.clone())]);
                        let mut tx = database.begin_transaction().unwrap();
                        tx.save_video_info(video_mapping).unwrap();
                    };

                    //println!("Received URL: \n\"{}\"\ncaption: \n\"{}\"\n", received_url, received_caption);
                    let _success = match send_videos(bot.clone(), dialogue.clone(), execution_mutex.clone(), database.clone(), config.clone()).await {
                        Ok(..) => {
                            //println!("Video sent successfully")
                        },
                        Err(e) => {
                            println!("Error sending video in loop: {}", e)
                        }
                    };
                }
            }
        } => {},
    }
}

fn schema() -> UpdateHandler<Box<dyn Error + Send + Sync + 'static>> {
    use dptree::case;

    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(case![Command::Help].endpoint(help))
        .branch(case![Command::Start].endpoint(start))
        .branch(case![Command::Settings].endpoint(settings))
        .branch(case![Command::Restore].endpoint(restore_sent_messages));

    let message_handler = Update::filter_message()
        .branch(command_handler)
        .branch(
            case![State::ReceivePostingInterval {
                stored_messages_to_delete,
                original_message_id,
            }]
            .endpoint(receive_posting_interval_message),
        )
        .branch(
            case![State::ReceiveRandomInterval {
                stored_messages_to_delete,
                original_message_id,
            }]
            .endpoint(receive_random_interval_message),
        )
        .branch(
            case![State::ReceiveRejectedContentLifespan {
                stored_messages_to_delete,
                original_message_id,
            }]
            .endpoint(receive_rejected_content_lifespan_message),
        )
        .branch(
            case![State::ReceivePostedContentLifespan {
                stored_messages_to_delete,
                original_message_id,
            }]
            .endpoint(receive_posted_content_lifespan_message),
        )
        .branch(
            case![State::ReceiveCaption {
                stored_messages_to_delete,
                original_message_id,
            }]
            .endpoint(receive_caption_message),
        )
        .branch(
            case![State::ReceiveHashtags {
                stored_messages_to_delete,
                original_message_id,
            }]
            .endpoint(receive_hashtags_message),
        );

    let callback_handler = Update::filter_callback_query()
        .branch(case![State::AcceptedView].endpoint(handle_accepted_view))
        .branch(case![State::RejectedView].endpoint(handle_rejected_view))
        .branch(
            case![State::EditView {
                stored_messages_to_delete
            }]
            .endpoint(handle_edit_view),
        )
        .branch(case![State::ScrapeView].endpoint(handle_video_action))
        .branch(case![State::ScrapeView].endpoint(handle_undo_callback))
        .branch(case![State::ScrapeView].endpoint(handle_remove_from_view_callback))
        .branch(
            case![State::SettingsView {
                stored_messages_to_delete,
                original_message_id
            }]
            .endpoint(handle_settings_callback),
        );

    dialogue::enter::<Update, InMemStorage<State>, State, _>()
        .branch(message_handler)
        .branch(callback_handler)
}

async fn receive_posting_interval_message(
    bot: Bot,
    dialogue: BotDialogue,
    msg: Message,
    state: State,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    match msg.text().map(ToOwned::to_owned) {
        Some(received_posting_interval) => {
            if let State::ReceivePostingInterval {
                stored_messages_to_delete,
                original_message_id,
            } = state
            {
                bot.delete_message(msg.chat.id, original_message_id).await?;

                for message_id in stored_messages_to_delete {
                    bot.delete_message(msg.chat.id, message_id).await?;
                }

                // Delete user response
                bot.delete_message(msg.chat.id, msg.id).await?;

                let mut tx = database.begin_transaction().unwrap();
                let mut user_settings = tx.load_user_settings().unwrap();

                match received_posting_interval.parse::<i64>() {
                    Ok(_) => {
                        user_settings.posting_interval = received_posting_interval.parse().unwrap();
                    }
                    Err(_) => {
                        let re = Regex::new(r"\d+").unwrap();
                        let first_number = re
                            .find_iter(&received_posting_interval)
                            .filter_map(|mat| mat.as_str().parse::<i64>().ok())
                            .next();

                        match first_number {
                            Some(number) => {
                                user_settings.posting_interval = number;
                            }
                            None => {}
                        }
                    }
                };
                let mut tx = database.begin_transaction().unwrap();
                tx.save_user_settings(user_settings).unwrap();
                display_settings_message(bot.clone(), dialogue.clone(), database, ui_definitions)
                    .await?;
            }
        }
        None => {
            // I should find a way to delete this message
            //bot.send_message(msg.chat.id, "Please send your caption.").await?;
        }
    }
    Ok(())
}

async fn receive_random_interval_message(
    bot: Bot,
    dialogue: BotDialogue,
    msg: Message,
    state: State,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    match msg.text().map(ToOwned::to_owned) {
        Some(received_random_interval) => {
            if let State::ReceiveRandomInterval {
                stored_messages_to_delete,
                original_message_id,
            } = state
            {
                bot.delete_message(msg.chat.id, original_message_id).await?;

                for message_id in stored_messages_to_delete {
                    bot.delete_message(msg.chat.id, message_id).await?;
                }

                // Delete user response
                bot.delete_message(msg.chat.id, msg.id).await?;

                let mut tx = database.begin_transaction().unwrap();
                let mut user_settings = tx.load_user_settings().unwrap();
                match received_random_interval.parse::<i64>() {
                    Ok(_) => {
                        user_settings.random_interval_variance =
                            received_random_interval.parse().unwrap();
                    }
                    Err(_) => {
                        let re = Regex::new(r"\d+").unwrap();
                        let first_number = re
                            .find_iter(&received_random_interval)
                            .filter_map(|mat| mat.as_str().parse::<i64>().ok())
                            .next();

                        match first_number {
                            Some(number) => {
                                user_settings.random_interval_variance = number;
                            }
                            None => {}
                        }
                    }
                };
                let mut tx = database.begin_transaction().unwrap();
                tx.save_user_settings(user_settings).unwrap();
                display_settings_message(bot.clone(), dialogue.clone(), database, ui_definitions)
                    .await?;
            }
        }
        None => {
            // I should find a way to delete this message
            //bot.send_message(msg.chat.id, "Please send your caption.").await?;
        }
    }
    Ok(())
}

async fn receive_rejected_content_lifespan_message(
    bot: Bot,
    dialogue: BotDialogue,
    msg: Message,
    state: State,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    match msg.text().map(ToOwned::to_owned) {
        Some(received_random_interval) => {
            if let State::ReceiveRejectedContentLifespan {
                stored_messages_to_delete,
                original_message_id,
            } = state
            {
                bot.delete_message(msg.chat.id, original_message_id).await?;

                for message_id in stored_messages_to_delete {
                    bot.delete_message(msg.chat.id, message_id).await?;
                }

                // Delete user response
                bot.delete_message(msg.chat.id, msg.id).await?;

                let mut tx = database.begin_transaction().unwrap();
                let mut user_settings = tx.load_user_settings().unwrap();
                match received_random_interval.parse::<i64>() {
                    Ok(_) => {
                        user_settings.rejected_content_lifespan =
                            received_random_interval.parse().unwrap();
                    }
                    Err(_) => {
                        let re = Regex::new(r"\d+").unwrap();
                        let first_number = re
                            .find_iter(&received_random_interval)
                            .filter_map(|mat| mat.as_str().parse::<i64>().ok())
                            .next();

                        match first_number {
                            Some(number) => {
                                user_settings.rejected_content_lifespan = number;
                            }
                            None => {}
                        }
                    }
                };
                let mut tx = database.begin_transaction().unwrap();
                tx.save_user_settings(user_settings).unwrap();
                display_settings_message(bot.clone(), dialogue.clone(), database, ui_definitions)
                    .await?;
            }
        }
        None => {
            // I should find a way to delete this message
            //bot.send_message(msg.chat.id, "Please send your caption.").await?;
        }
    }
    Ok(())
}

async fn receive_posted_content_lifespan_message(
    bot: Bot,
    dialogue: BotDialogue,
    msg: Message,
    state: State,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    match msg.text().map(ToOwned::to_owned) {
        Some(received_random_interval) => {
            if let State::ReceivePostedContentLifespan {
                stored_messages_to_delete,
                original_message_id,
            } = state
            {
                bot.delete_message(msg.chat.id, original_message_id).await?;

                for message_id in stored_messages_to_delete {
                    bot.delete_message(msg.chat.id, message_id).await?;
                }

                // Delete user response
                bot.delete_message(msg.chat.id, msg.id).await?;

                let mut tx = database.begin_transaction().unwrap();
                let mut user_settings = tx.load_user_settings().unwrap();
                match received_random_interval.parse::<i64>() {
                    Ok(_) => {
                        user_settings.posted_content_lifespan =
                            received_random_interval.parse().unwrap();
                    }
                    Err(_) => {
                        let re = Regex::new(r"\d+").unwrap();
                        let first_number = re
                            .find_iter(&received_random_interval)
                            .filter_map(|mat| mat.as_str().parse::<i64>().ok())
                            .next();

                        match first_number {
                            Some(number) => {
                                user_settings.posted_content_lifespan = number;
                            }
                            None => {}
                        }
                    }
                };
                let mut tx = database.begin_transaction().unwrap();
                tx.save_user_settings(user_settings).unwrap();
                display_settings_message(bot.clone(), dialogue.clone(), database, ui_definitions)
                    .await?;
            }
        }
        None => {
            // I should find a way to delete this message
            //bot.send_message(msg.chat.id, "Please send your caption.").await?;
        }
    }
    Ok(())
}

async fn receive_caption_message(
    bot: Bot,
    dialogue: BotDialogue,
    msg: Message,
    state: State,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    match msg.text().map(ToOwned::to_owned) {
        Some(caption) => {
            if let State::ReceiveCaption {
                stored_messages_to_delete,
                original_message_id,
            } = state
            {
                for message_id in stored_messages_to_delete {
                    bot.delete_message(msg.chat.id, message_id).await?;
                }

                // Delete user response
                bot.delete_message(msg.chat.id, msg.id).await?;

                let mut tx = database.begin_transaction().unwrap();
                let mut video_info = tx
                    .get_video_info_by_message_id(original_message_id)
                    .unwrap();
                video_info.caption = caption;
                let mut tx = database.begin_transaction().unwrap();
                tx.save_video_info(IndexMap::from([(original_message_id, video_info.clone())]))?;

                let go_back_action_text = ui_definitions.buttons.get("go_back").unwrap();
                let edit_caption_action_text = ui_definitions.buttons.get("edit_caption").unwrap();
                let edit_hashtags_action_text =
                    ui_definitions.buttons.get("edit_hashtags").unwrap();
                let edit_action_row_1 = [InlineKeyboardButton::callback(
                    go_back_action_text,
                    format!("go_back_{}", original_message_id),
                )];

                let edit_action_row_2 = [InlineKeyboardButton::callback(
                    edit_caption_action_text,
                    format!("edit_caption_{}", original_message_id),
                )];

                let edit_action_row_3 = [InlineKeyboardButton::callback(
                    edit_hashtags_action_text,
                    format!("edit_hashtags_{}", original_message_id),
                )];

                let edit_actions = [edit_action_row_1, edit_action_row_2, edit_action_row_3];

                let msg2 = bot
                    .send_message(
                        msg.chat.id,
                        format!(
                            "Url: {}\nCaption: {}\nHashtags: {}\n(from @{})",
                            video_info.url,
                            video_info.caption,
                            video_info.hashtags,
                            video_info.original_author
                        ),
                    )
                    .reply_markup(InlineKeyboardMarkup::new(edit_actions))
                    .await?;

                // Update the dialogue with the new state
                dialogue
                    .update(State::EditView {
                        stored_messages_to_delete: vec![msg2.id],
                    })
                    .await?;
            }
        }
        None => {
            //bot.send_message(msg.chat.id, "Please send your caption.").await?;
        }
    }
    Ok(())
}

async fn receive_hashtags_message(
    bot: Bot,
    dialogue: BotDialogue,
    msg: Message,
    state: State,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    match msg.text().map(ToOwned::to_owned) {
        Some(hashtags) => {
            if let State::ReceiveHashtags {
                stored_messages_to_delete,
                original_message_id,
            } = state
            {
                for message_id in stored_messages_to_delete {
                    bot.delete_message(msg.chat.id, message_id).await?;
                }

                // Delete user response
                bot.delete_message(msg.chat.id, msg.id).await?;

                let mut tx = database.begin_transaction().unwrap();
                let mut video_info = tx
                    .get_video_info_by_message_id(original_message_id)
                    .unwrap();
                video_info.hashtags = hashtags;
                let mut tx = database.begin_transaction().unwrap();
                tx.save_video_info(IndexMap::from([(original_message_id, video_info.clone())]))?;

                let go_back_action_text = ui_definitions.buttons.get("go_back").unwrap();
                let edit_caption_action_text = ui_definitions.buttons.get("edit_caption").unwrap();
                let edit_hashtags_action_text =
                    ui_definitions.buttons.get("edit_hashtags").unwrap();
                let edit_action_row_1 = [InlineKeyboardButton::callback(
                    go_back_action_text,
                    format!("go_back_{}", original_message_id),
                )];

                let edit_action_row_2 = [InlineKeyboardButton::callback(
                    edit_caption_action_text,
                    format!("edit_caption_{}", original_message_id),
                )];

                let edit_action_row_3 = [InlineKeyboardButton::callback(
                    edit_hashtags_action_text,
                    format!("edit_hashtags_{}", original_message_id),
                )];

                let edit_actions = [edit_action_row_1, edit_action_row_2, edit_action_row_3];

                let msg2 = bot
                    .send_message(
                        msg.chat.id,
                        format!(
                            "Url: {}\nCaption: {}\nHashtags: {}\n(from @{})",
                            video_info.url,
                            video_info.caption,
                            video_info.hashtags,
                            video_info.original_author
                        ),
                    )
                    .reply_markup(InlineKeyboardMarkup::new(edit_actions))
                    .await?;

                // Update the dialogue with the new state
                dialogue
                    .update(State::EditView {
                        stored_messages_to_delete: vec![msg2.id],
                    })
                    .await?;
            }
        }
        None => {
            //bot.send_message(msg.chat.id, "Please send your hashtags.").await?;
        }
    }
    Ok(())
}

async fn handle_edit_view(
    bot: Bot,
    dialogue: BotDialogue,
    q: CallbackQuery,
    execution_mutex: Arc<Mutex<()>>,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    let (action, message_id) = parse_callback_query(&q);
    //println!("handle_edit_view original message id {}, action {}", message_id, action);

    if action == "go_back" {
        //println!("go back pressed");

        // Clear the sent messages, from message_id to the latest message
        bot.delete_message(chat_id, q.message.unwrap().id).await?;
        send_videos(
            bot.clone(),
            dialogue.clone(),
            execution_mutex,
            database,
            ui_definitions,
        )
        .await?;

        dialogue.update(State::ScrapeView).await.unwrap();
        return Ok(());
    } else if action == "edit" {
        let mut tx = database.begin_transaction().unwrap();
        let video = tx.get_video_info_by_message_id(message_id).unwrap();
        clear_sent_messages(bot.clone(), database).await.unwrap();

        let go_back_action_text = ui_definitions.buttons.get("go_back").unwrap();
        let edit_caption_action_text = ui_definitions.buttons.get("edit_caption").unwrap();
        let edit_hashtags_action_text = ui_definitions.buttons.get("edit_hashtags").unwrap();
        let edit_action_row_1 = [InlineKeyboardButton::callback(
            go_back_action_text,
            format!("go_back_{}", q.message.clone().unwrap().id),
        )];

        let edit_action_row_2 = [InlineKeyboardButton::callback(
            edit_caption_action_text,
            format!("edit_caption_{}", q.message.clone().unwrap().id),
        )];

        let edit_action_row_3 = [InlineKeyboardButton::callback(
            edit_hashtags_action_text,
            format!("edit_hashtags_{}", q.message.clone().unwrap().id),
        )];

        let edit_actions = [edit_action_row_1, edit_action_row_2, edit_action_row_3];

        let msg2 = bot
            .send_message(
                q.message.clone().unwrap().chat.id,
                format!(
                    "Url: {}\nCaption: {}\nHashtags: {}\n(from @{})",
                    video.url, video.caption, video.hashtags, video.original_author
                ),
            )
            .reply_markup(InlineKeyboardMarkup::new(edit_actions))
            .await?;

        // Update the dialogue with the new state
        dialogue
            .update(State::EditView {
                stored_messages_to_delete: vec![msg2.id],
            })
            .await?;
    } else if action == "edit_caption" {
        let mut messages_to_delete = Vec::new();

        if let State::EditView {
            stored_messages_to_delete,
        } = dialogue.get().await.unwrap().unwrap()
        {
            for message_id in stored_messages_to_delete {
                messages_to_delete.push(message_id);
            }
        }

        let caption_message = bot
            .send_message(chat_id, "Please send your caption.")
            .await?;
        messages_to_delete.push(caption_message.id);
        // Update the dialogue with the new state

        dialogue
            .update(State::ReceiveCaption {
                stored_messages_to_delete: messages_to_delete,
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else if action == "edit_hashtags" {
        let mut messages_to_delete = Vec::new();

        if let State::EditView {
            stored_messages_to_delete,
        } = dialogue.get().await.unwrap().unwrap()
        {
            for message_id in stored_messages_to_delete {
                messages_to_delete.push(message_id);
            }
        }

        let caption_message = bot
            .send_message(chat_id, "Please send your hashtags.")
            .await?;
        messages_to_delete.push(caption_message.id);
        // Update the dialogue with the new state

        dialogue
            .update(State::ReceiveHashtags {
                stored_messages_to_delete: messages_to_delete,
                original_message_id: message_id,
            })
            .await
            .unwrap();
    }

    Ok(())
}

fn parse_callback_query(q: &CallbackQuery) -> (String, MessageId) {
    // Extract the message id from the callback data
    let data_parts: Vec<&str> = q.data.as_ref().unwrap().split('_').collect();

    let (action, message_id) = if data_parts.len() == 5 {
        let action = format!(
            "{}_{}_{}_{}",
            data_parts[0], data_parts[1], data_parts[2], data_parts[3]
        );
        let message_id = data_parts[4].parse().unwrap();
        (action, message_id)
    } else if data_parts.len() == 4 {
        let action = format!("{}_{}_{}", data_parts[0], data_parts[1], data_parts[2]);
        let message_id = data_parts[3].parse().unwrap();
        (action, message_id)
    } else if data_parts.len() == 3 {
        let action = format!("{}_{}", data_parts[0], data_parts[1]);
        let message_id = data_parts[2].parse().unwrap();
        (action, message_id)
    } else if data_parts.len() == 2 {
        let action = data_parts[0].parse().unwrap();
        let message_id = data_parts[1].parse().unwrap();
        (action, message_id)
    } else {
        panic!(
            "Unrecognized callback query data: {}",
            q.data.as_ref().unwrap()
        );
    };
    let message_id = MessageId(message_id);
    (action, message_id)
}

async fn handle_remove_from_queue(
    bot: Bot,
    q: CallbackQuery,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    let (_action, message_id) = parse_callback_query(&q);

    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_video_info_by_message_id(message_id).unwrap();

    let mut tx = database.begin_transaction().unwrap();
    tx.remove_post_from_queue_with_shortcode(video_info.original_shortcode.clone()).unwrap();

    video_info.status = "pending_shown".to_string();
    let video_mapping: IndexMap<MessageId, VideoInfo> =
        IndexMap::from([(message_id, video_info.clone())]);
    let mut tx = database.begin_transaction().unwrap();
    tx.save_video_info(video_mapping).unwrap();

    let full_video_caption = format!(
        "{}\n{}\n(from @{})",
        video_info.caption, video_info.hashtags, video_info.original_author
    );
    bot.edit_message_caption(chat_id, message_id)
        .caption(full_video_caption)
        .await?;

    let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
    let reject_action_text = ui_definitions.buttons.get("reject").unwrap();
    let edit_action_text = ui_definitions.buttons.get("edit").unwrap();

    let video_actions = [
        InlineKeyboardButton::callback(accept_action_text, format!("accept_{}", message_id)),
        InlineKeyboardButton::callback(reject_action_text, format!("reject_{}", message_id)),
        InlineKeyboardButton::callback(edit_action_text, format!("edit_{}", message_id)),
    ];

    let _edited_markup = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(InlineKeyboardMarkup::new([video_actions]))
        .await?;

    Ok(())
}
async fn handle_accepted_view(
    bot: Bot,
    dialogue: BotDialogue,
    q: CallbackQuery,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    // Extract the message id from the callback data
    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_video_info_by_message_id(message_id).unwrap();
    //println!("original message id {}, action {}", message_id, action);

    let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
    let undo_action = [InlineKeyboardButton::callback(
        undo_action_text,
        format!("undo_{}", message_id),
    )];

    let accepted_caption_text = ui_definitions.labels.get("accepted_caption").unwrap();
    let full_caption = format!(
        "{}\n{}\n(from @{})\n\n{}",
        video_info.caption, video_info.hashtags, video_info.original_author, accepted_caption_text
    );
    bot.edit_message_caption(chat_id, message_id)
        .caption(full_caption)
        .await?;

    let _msg = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(InlineKeyboardMarkup::new([undo_action]))
        .await?;
    //println!("accepted ids: {}, {}", msg.id, message_id);
    video_info.status = "accepted_shown".to_string();
    let video_mapping: IndexMap<MessageId, VideoInfo> = IndexMap::from([(message_id, video_info)]);
    let mut tx = database.begin_transaction().unwrap();
    tx.save_video_info(video_mapping).unwrap();

    dialogue.update(State::ScrapeView).await.unwrap();

    Ok(())
}

async fn handle_rejected_view(
    bot: Bot,
    dialogue: BotDialogue,
    q: CallbackQuery,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    let _chat_id = q.message.clone().unwrap().chat.id;

    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_video_info_by_message_id(message_id).unwrap();
    //println!("original message id {}, action {}", message_id, action);

    video_info.status = "rejected_shown".to_string();
    let video_mapping: IndexMap<MessageId, VideoInfo> =
        IndexMap::from([(message_id, video_info.clone())]);
    let mut tx = database.begin_transaction().unwrap();
    tx.save_video_info(video_mapping).unwrap();

    let rejected_content = RejectedContent {
        url: video_info.url.clone(),
        caption: video_info.caption.clone(),
        hashtags: video_info.hashtags.clone(),
        original_author: video_info.original_author.clone(),
        original_shortcode: video_info.original_shortcode.clone(),
        rejected_at: chrono::Utc::now().to_rfc3339(),
        expired: false,
    };

    let mut tx = database.begin_transaction().unwrap();
    tx.save_rejected_content(rejected_content).unwrap();

    let _did_expire =
        expire_rejected_content(&bot, ui_definitions, &mut tx, message_id, &mut video_info).await?;

    dialogue.update(State::ScrapeView).await.unwrap();

    Ok(())
}

async fn handle_undo_callback(
    bot: Bot,
    q: CallbackQuery,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    // Extract the message id from the callback data
    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_video_info_by_message_id(message_id).unwrap();

    video_info.status = "pending_shown".to_string();
    let video_mapping: IndexMap<MessageId, VideoInfo> =
        IndexMap::from([(message_id, video_info.clone())]);
    let mut tx = database.begin_transaction().unwrap();
    tx.save_video_info(video_mapping).unwrap();

    let full_video_caption = format!(
        "{}\n{}\n(from @{})",
        video_info.caption, video_info.hashtags, video_info.original_author
    );
    bot.edit_message_caption(chat_id, message_id)
        .caption(full_video_caption)
        .await?;

    let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
    let reject_action_text = ui_definitions.buttons.get("reject").unwrap();
    let edit_action_text = ui_definitions.buttons.get("edit").unwrap();

    let video_actions = [
        InlineKeyboardButton::callback(accept_action_text, format!("accept_{}", message_id)),
        InlineKeyboardButton::callback(reject_action_text, format!("reject_{}", message_id)),
        InlineKeyboardButton::callback(edit_action_text, format!("edit_{}", message_id)),
    ];

    let _edited_markup = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(InlineKeyboardMarkup::new([video_actions]))
        .await?;

    // Check if it is a rejected video
    let mut tx = database.begin_transaction().unwrap();
    let rejected_content = tx.load_rejected_content().unwrap();
    for content in rejected_content {
        if content.url == video_info.url {
            let mut tx = database.begin_transaction().unwrap();
            tx.remove_rejected_content_with_shortcode(video_info.original_shortcode.clone()).unwrap();
        }
    }
    //println!("undo pressed, original message id {}, action {}", message_id, action);

    Ok(())
}

async fn handle_remove_from_view_callback(
    bot: Bot,
    q: CallbackQuery,
    database: Database,
) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let video_info = tx.get_video_info_by_message_id(message_id).unwrap();

    tx.remove_video_info_with_shortcode(video_info.original_shortcode).unwrap();

    bot.delete_message(chat_id, message_id).await?;

    Ok(())
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
                let mut tx = database.begin_transaction().unwrap();
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

                        if will_expire_at < chrono::Utc::now() {
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
                let mut tx = database.begin_transaction().unwrap();
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

                        if will_expire_at < chrono::Utc::now() {
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
                let mut tx = database.begin_transaction().unwrap();
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
                        "{}\n\n{} at {}\nWill expire at {}",
                        full_video_caption,
                        posted_video_caption,
                        formatted_datetime,
                        will_expire_at
                    );
                } else {
                    let posted_content = PostedContent {
                        url: queue_element.clone().unwrap().url.clone(),
                        caption: queue_element.clone().unwrap().caption.clone(),
                        hashtags: queue_element.clone().unwrap().caption.clone(),
                        original_author: queue_element.clone().unwrap().original_author.clone(),
                        original_shortcode: queue_element.clone().unwrap().original_shortcode.clone(),
                        posted_at: chrono::Utc::now().to_rfc3339(),
                        expired: false,
                    };

                    // The matching QueuedPost will be cleared by save_posted_content
                    let mut tx = database.begin_transaction().unwrap();
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
                        "{}\n\n{} at {}\nWill expire at {}",
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
                        let mut tx = database.begin_transaction().unwrap();
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

                        let mut tx = database.begin_transaction().unwrap();
                        tx.save_video_info(video_mapping).unwrap();
                    }
                }
                continue;
            }

            if video.status == "queued_hidden" {
                video.status = "queued_shown".to_string();

                let mut tx = database.begin_transaction().unwrap();
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
                        let mut tx = database.begin_transaction().unwrap();
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

                        let mut tx = database.begin_transaction().unwrap();
                        tx.save_video_info(video_mapping).unwrap();
                    }
                }

                continue;
            }

            if video.status == "accepted_hidden" {
                video.status = "accepted_shown".to_string();
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

            if video.status == "accepted_shown" || video.status == "rejected_shown" {
                let mut video_caption = String::new();
                if video.status == "accepted_shown" {
                    let accepted_caption_text =
                        ui_definitions.buttons.get("accepted_caption").unwrap();
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
            let mut tx = database.begin_transaction().unwrap();
            tx.save_video_info(video_mapping).unwrap();
            // NOTE
            //sleep(Duration::from_millis(250)).await;
        }
    }
    Ok(())
}

async fn expire_rejected_content(
    bot: &Bot,
    ui_definitions: UIDefinitions,
    tx: &mut DatabaseTransaction,
    message_id: MessageId,
    video: &mut VideoInfo,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let rejected_content = tx.load_rejected_content()?;
    let mut did_expire = false;

    let user_settings = tx.load_user_settings()?;
    let expiry_duration =
        Duration::from_secs((user_settings.rejected_content_lifespan * 60) as u64);
    let now = chrono::Utc::now();

    for mut content in rejected_content {
        let datetime = DateTime::parse_from_rfc3339(&content.rejected_at)?;

        if content.url == video.url && !content.expired {
            if now > datetime + expiry_duration {
                video.status = "removed_from_view".to_string();
                bot.delete_message(CHAT_ID, message_id).await?;
                did_expire = true;
                content.expired = true;
                tx.save_rejected_content(content.clone())?;
                tx.remove_video_info_with_shortcode(content.original_shortcode.clone())?;
                //println!("Permanently removed post from video_info: {}", content.url);
            } else {
                let expiry_date = datetime + expiry_duration;
                let rejected_caption = ui_definitions.labels.get("rejected_caption").unwrap();
                let full_video_caption = format!(
                    "{}\n{}\n(from @{})\n\n{}: will expire at {}",
                    content.caption,
                    content.hashtags,
                    content.original_author,
                    rejected_caption,
                    expiry_date.format("%H:%M %m-%d")
                );
                let _ = match bot
                    .edit_message_caption(CHAT_ID, message_id)
                    .caption(full_video_caption.clone())
                    .await
                {
                    Ok(_) => {
                        let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                        let remove_from_view_action_text =
                            ui_definitions.buttons.get("remove_from_view").unwrap();
                        let undo_action = [
                            InlineKeyboardButton::callback(
                                undo_action_text,
                                format!("undo_{}", message_id),
                            ),
                            InlineKeyboardButton::callback(
                                remove_from_view_action_text,
                                format!("remove_from_view_{}", message_id),
                            ),
                        ];

                        let _msg = bot
                            .edit_message_reply_markup(CHAT_ID, message_id)
                            .reply_markup(InlineKeyboardMarkup::new([undo_action]))
                            .await?;
                    }
                    Err(_) => {
                        let new_message = bot
                            .send_video(
                                CHAT_ID,
                                InputFile::url(content.url.clone().parse().unwrap()),
                            )
                            .caption(full_video_caption)
                            .await?;

                        let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                        let remove_from_view_action_text =
                            ui_definitions.buttons.get("remove_from_view").unwrap();
                        let undo_action = [
                            InlineKeyboardButton::callback(
                                undo_action_text,
                                format!("undo_{}", new_message.id),
                            ),
                            InlineKeyboardButton::callback(
                                remove_from_view_action_text,
                                format!("remove_from_view_{}", new_message.id),
                            ),
                        ];

                        tx.save_video_info(IndexMap::from([(new_message.id, video.clone())]))?;

                        let _msg = bot
                            .edit_message_reply_markup(CHAT_ID, new_message.id)
                            .reply_markup(InlineKeyboardMarkup::new([undo_action]))
                            .await?;
                    }
                };
            }
        }
    }
    Ok(did_expire)
}

async fn expire_posted_content(
    bot: &Bot,
    ui_definitions: UIDefinitions,
    tx: &mut DatabaseTransaction,
    message_id: MessageId,
    video: &mut VideoInfo,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let posted_content = tx.load_posted_content()?;
    let mut did_expire = false;

    let user_settings = tx.load_user_settings()?;
    let expiry_duration = Duration::from_secs((user_settings.posted_content_lifespan * 60) as u64);
    let now = chrono::Utc::now();

    for mut content in posted_content {
        let datetime = DateTime::parse_from_rfc3339(&content.posted_at)?;

        if content.url == video.url && !content.expired {
            if now > datetime + expiry_duration {
                video.status = "removed_from_view".to_string();
                bot.delete_message(CHAT_ID, message_id).await?;
                did_expire = true;
                content.expired = true;
                tx.save_posted_content(content.clone())?;
                tx.remove_video_info_with_shortcode(content.original_shortcode.clone())?;
                //println!("Permanently removed posted content from video_info: {}", content.url);
            } else {
                let expiry_date = datetime + expiry_duration;
                let posted_video_caption =
                    ui_definitions.labels.get("posted_video_caption").unwrap();
                let full_video_caption = format!(
                    "{posted_video_caption}: will expire at {}",
                    expiry_date.format("%H:%M %m-%d")
                );
                let _ = match bot
                    .edit_message_caption(CHAT_ID, message_id)
                    .caption(full_video_caption.clone())
                    .await
                {
                    Ok(_) => {
                        let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                        let remove_from_view_action_text =
                            ui_definitions.buttons.get("remove_from_view").unwrap();
                        let undo_action = [
                            InlineKeyboardButton::callback(
                                undo_action_text,
                                format!("undo_{}", message_id),
                            ),
                            InlineKeyboardButton::callback(
                                remove_from_view_action_text,
                                format!("remove_from_view_{}", message_id),
                            ),
                        ];

                        let _msg = bot
                            .edit_message_reply_markup(CHAT_ID, message_id)
                            .reply_markup(InlineKeyboardMarkup::new([undo_action]))
                            .await?;
                    }
                    Err(_) => {
                        let new_message = bot
                            .send_video(
                                CHAT_ID,
                                InputFile::url(content.url.clone().parse().unwrap()),
                            )
                            .caption(full_video_caption)
                            .await?;

                        let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
                        let remove_from_view_action_text =
                            ui_definitions.buttons.get("remove_from_view").unwrap();
                        let undo_action = [
                            InlineKeyboardButton::callback(
                                undo_action_text,
                                format!("undo_{}", new_message.id),
                            ),
                            InlineKeyboardButton::callback(
                                remove_from_view_action_text,
                                format!("remove_from_view_{}", new_message.id),
                            ),
                        ];

                        tx.save_video_info(IndexMap::from([(new_message.id, video.clone())]))?;

                        let _msg = bot
                            .edit_message_reply_markup(CHAT_ID, new_message.id)
                            .reply_markup(InlineKeyboardMarkup::new([undo_action]))
                            .await?;
                    }
                };
            }
        }
    }
    Ok(did_expire)
}

async fn handle_video_action(
    bot: Bot,
    dialogue: BotDialogue,
    execution_mutex: Arc<Mutex<()>>,
    q: CallbackQuery,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    if let Some(_data) = &q.data {
        let (action, _message_id) = parse_callback_query(&q);

        if action == "remove_from_queue" {
            handle_remove_from_queue(bot, q, database, ui_definitions).await?;
        } else if action == "accept" {
            dialogue.update(State::AcceptedView).await.unwrap();
            handle_accepted_view(bot, dialogue, q, database, ui_definitions).await?;
        } else if action == "reject" {
            dialogue.update(State::RejectedView).await.unwrap();
            handle_rejected_view(bot, dialogue, q, database, ui_definitions).await?;
        } else if action == "edit" {
            dialogue
                .update(State::EditView {
                    stored_messages_to_delete: Vec::new(),
                })
                .await?;
            handle_edit_view(bot, dialogue, q, execution_mutex, database, ui_definitions).await?;
        } else if action == "undo" {
            dialogue.update(State::ScrapeView).await.unwrap();
            handle_undo_callback(bot, q, database, ui_definitions).await?;
        } else if action == "remove_from_view" {
            dialogue.update(State::ScrapeView).await.unwrap();
            handle_remove_from_view_callback(bot, q, database).await?;
        } else {
            println!("Invalid action received: {}", action);
            dialogue.update(State::ScrapeView).await.unwrap();
        }
    }

    Ok(())
}

async fn help(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, Command::descriptions().to_string())
        .await?;
    Ok(())
}

async fn start(bot: Bot, msg: Message) -> HandlerResult {
    if msg.chat.id == ChatId(34957918) {
        bot.send_message(
            msg.chat.id,
            format!("Welcome back, {}! 🦀", msg.chat.first_name().unwrap()).to_string(),
        )
        .await?;
    } else {
        bot.send_message(msg.chat.id, "You can't use this bot.".to_string())
            .await?;
    }

    // Can't delete this unfortunately
    // bot.delete_message(msg.chat.id, msg.id).await?;
    Ok(())
}
async fn handle_settings_callback(
    bot: Bot,
    dialogue: BotDialogue,
    q: CallbackQuery,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    let (action, message_id) = parse_callback_query(&q);

    if action == "go_back" {
        bot.delete_message(q.message.unwrap().chat.id, message_id)
            .await?;
        dialogue.update(State::ScrapeView).await.unwrap();
    } else if action == "turn_on" {
        bot.delete_message(q.message.clone().unwrap().chat.id, message_id)
            .await?;
        let mut tx = database.begin_transaction().unwrap();
        let mut user_settings = tx.load_user_settings().unwrap();

        user_settings.can_post = true;

        let mut tx = database.begin_transaction().unwrap();
        tx.save_user_settings(user_settings).unwrap();

        display_settings_message(bot, dialogue, database, ui_definitions).await?;
    } else if action == "turn_off" {
        bot.delete_message(q.message.clone().unwrap().chat.id, message_id)
            .await?;
        let mut tx = database.begin_transaction().unwrap();
        let mut user_settings = tx.load_user_settings().unwrap();
        user_settings.can_post = false;
        let mut tx = database.begin_transaction().unwrap();
        tx.save_user_settings(user_settings).unwrap();

        display_settings_message(bot, dialogue, database, ui_definitions).await?;
    } else if action == "adjust_posting_interval" {
        let msg = bot
            .send_message(
                q.message.unwrap().chat.id,
                "Send your desired interval (in minutes) between posts",
            )
            .await?;

        dialogue
            .update(State::ReceivePostingInterval {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else if action == "adjust_random_interval" {
        let msg = bot
            .send_message(
                q.message.unwrap().chat.id,
                "Send your desired random interval (in minutes) between posts",
            )
            .await?;
        dialogue
            .update(State::ReceiveRandomInterval {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else if action == "adjust_rejected_content_lifespan" {
        let msg = bot
            .send_message(
                q.message.unwrap().chat.id,
                "Send your desired rejected content lifespan (in minutes)",
            )
            .await?;
        dialogue
            .update(State::ReceiveRejectedContentLifespan {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else if action == "adjust_posted_content_lifespan" {
        let msg = bot
            .send_message(
                q.message.unwrap().chat.id,
                "Send your desired posted content lifespan (in minutes)",
            )
            .await?;
        dialogue
            .update(State::ReceivePostedContentLifespan {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else {
        println!(
            "invalid action in handle_setting_callback original message id {}, action {}",
            message_id, action
        );
    }

    Ok(())
}
async fn settings(
    bot: Bot,
    dialogue: BotDialogue,
    msg: Message,
    database: Database,
    ui_definitions: UIDefinitions,
    execution_mutex: Arc<Mutex<()>>,
) -> HandlerResult {
    if let Some(state) = dialogue.get().await.unwrap() {
        if state != State::ScrapeView {
            let _ = bot.delete_message(CHAT_ID, msg.id).await;
            return Ok(());
        }
    }

    let _guard = execution_mutex.lock().await;
    let _ = bot.delete_message(CHAT_ID, msg.id).await;
    let _ = clear_sent_messages(bot.clone(), database.clone()).await;

    display_settings_message(bot, dialogue, database, ui_definitions).await?;

    Ok(())
}

async fn display_settings_message(
    bot: Bot,
    dialogue: BotDialogue,
    database: Database,
    ui_definitions: UIDefinitions,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut tx = database.begin_transaction().unwrap();
    let user_settings = tx.load_user_settings().unwrap();
    let mut posting_status = "disabled";
    if user_settings.can_post {
        posting_status = "enabled";
    }

    let settings_title_text = ui_definitions.labels.get("settings_title").unwrap();
    let current_time = chrono::Utc::now().format("%H:%M %m-%d").to_string();
    let settings_message_string = format!("{}  {}\n\nPosting is currently {}.\nInterval between posts is {} minutes.\nRandom interval is ±{} minutes\nRejected content expires after {} minutes\nPosted content expires after {} minutes\n\nWhat would you like to change?", settings_title_text, current_time, posting_status, user_settings.posting_interval, user_settings.random_interval_variance, user_settings.rejected_content_lifespan, user_settings.posted_content_lifespan);

    let settings_message = bot.send_message(CHAT_ID, settings_message_string).await?;

    dialogue
        .update(State::SettingsView {
            stored_messages_to_delete: vec![],
            original_message_id: settings_message.id,
        })
        .await
        .unwrap();

    let turn_off_action_text = ui_definitions.buttons.get("turn_off").unwrap();
    let turn_on_action_text = ui_definitions.buttons.get("turn_on").unwrap();
    let settings_action_row_1_col_2 = if user_settings.can_post {
        InlineKeyboardButton::callback(
            turn_off_action_text,
            format!("turn_off_{}", settings_message.id),
        )
    } else {
        InlineKeyboardButton::callback(
            turn_on_action_text,
            format!("turn_on_{}", settings_message.id),
        )
    };

    let go_back_action_text = ui_definitions.buttons.get("go_back").unwrap();
    let settings_actions_row_1 = vec![
        InlineKeyboardButton::callback(
            go_back_action_text,
            format!("go_back_{}", settings_message.id),
        ),
        settings_action_row_1_col_2,
    ];

    let adjust_posting_interval_action_text = ui_definitions
        .buttons
        .get("adjust_posting_interval")
        .unwrap();
    let settings_actions_row_2 = vec![InlineKeyboardButton::callback(
        adjust_posting_interval_action_text,
        format!("adjust_posting_interval_{}", settings_message.id),
    )];

    let adjust_random_interval_action_text = ui_definitions
        .buttons
        .get("adjust_random_interval")
        .unwrap();
    let settings_actions_row_3 = vec![InlineKeyboardButton::callback(
        adjust_random_interval_action_text,
        format!("adjust_random_interval_{}", settings_message.id),
    )];

    let adjust_rejected_content_lifespan_action_text = ui_definitions
        .buttons
        .get("adjust_rejected_content_lifespan")
        .unwrap();
    let settings_actions_row_4 = vec![InlineKeyboardButton::callback(
        adjust_rejected_content_lifespan_action_text,
        format!("adjust_rejected_content_lifespan_{}", settings_message.id),
    )];

    let adjust_posted_content_lifespan_action_text = ui_definitions
        .buttons
        .get("adjust_posted_content_lifespan")
        .unwrap();
    let settings_actions_row_5 = vec![InlineKeyboardButton::callback(
        adjust_posted_content_lifespan_action_text,
        format!("adjust_posted_content_lifespan_{}", settings_message.id),
    )];

    let settings_actions = [
        settings_actions_row_1,
        settings_actions_row_2,
        settings_actions_row_3,
        settings_actions_row_4,
        settings_actions_row_5,
    ];

    bot.edit_message_reply_markup(CHAT_ID, settings_message.id)
        .reply_markup(InlineKeyboardMarkup::new(settings_actions))
        .await?;
    Ok(())
}

pub async fn clear_sent_messages(bot: Bot, database: Database) -> std::io::Result<()> {
    // Load the video mappings

    let mut tx = database.begin_transaction().unwrap();
    let mut video_mapping = tx.load_video_mapping().unwrap();
    // println!("Trying to clear messages");
    for (message_id, video_info) in &mut video_mapping {
        if video_info.status == "pending_shown" {
            video_info.status = "pending_hidden".to_string();
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    // Update the status to "pending_hidden"
                    //println!("Deleted pending message with ID: {}", message_id);
                }
                Err(_e) => {
                    //println!("Error deleting pending message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == "accepted_shown" {
            video_info.status = "accepted_hidden".to_string();
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    // Update the status to "pending_hidden"
                    //println!("Deleted accepted message with ID: {}", message_id);
                }
                Err(_e) => {
                    //println!("Error deleting accepted message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == "rejected_shown" {
            video_info.status = "rejected_hidden".to_string();
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    // Update the status to "pending_hidden"
                    //println!("Deleted rejected message with ID: {}", message_id);
                }
                Err(_e) => {
                    //println!("Error deleting rejected message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == "posted_shown" {
            video_info.status = "posted_hidden".to_string();
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    // Update the status to "pending_hidden"
                    //println!("Deleted posted message with ID: {}", message_id);
                }
                Err(_e) => {
                    //println!("Error deleting posted message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == "queued_shown" {
            video_info.status = "queued_hidden".to_string();
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    // Update the status to "pending_hidden"
                    //println!("Deleted queued message with ID: {}", message_id);
                }
                Err(_e) => {
                    //println!("Error deleting queued message with ID: {}: {}", message_id, e);
                }
            }
        }
    }

    // Save the updated video mappings
    let mut tx = database.begin_transaction().unwrap();
    tx.save_video_info(video_mapping).unwrap();

    Ok(())
}

pub async fn restore_sent_messages(
    bot: Bot,
    dialogue: BotDialogue,
    database: Database,
    execution_mutex: Arc<Mutex<()>>,
    ui_definitions: UIDefinitions,
    msg: Message,
) -> HandlerResult {
    // Load the video mappings

    let _ = bot.delete_message(CHAT_ID, msg.id).await;
    clear_sent_messages(bot.clone(), database.clone()).await?;
    send_videos(bot, dialogue, execution_mutex, database, ui_definitions).await?;

    Ok(())
}
