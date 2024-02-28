use indexmap::IndexMap;
use regex::Regex;
use teloxide::Bot;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::{Message, Requester};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use crate::telegram_bot::{BotDialogue, HandlerResult, State, UIDefinitions};
use crate::telegram_bot::commands::display_settings_message;
use crate::utils::Database;

pub async fn receive_posting_interval(
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

pub async fn receive_random_interval(
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

pub async fn receive_rejected_content_lifespan(
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

pub async fn receive_posted_content_lifespan(
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

pub async fn receive_caption(
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
                let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
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

                let edit_action_row_4 = [InlineKeyboardButton::callback(
                    accept_action_text,
                    format!("accept_{}", original_message_id),
                )];

                let edit_actions = [edit_action_row_1, edit_action_row_2, edit_action_row_3, edit_action_row_4];

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

pub async fn receive_hashtags(
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
                let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
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

                let edit_action_row_4 = [InlineKeyboardButton::callback(
                    accept_action_text,
                    format!("accept_{}", original_message_id),
                )];

                let edit_actions = [edit_action_row_1, edit_action_row_2, edit_action_row_3, edit_action_row_4];

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

