use crate::telegram_bot::{UIDefinitions, CHAT_ID};
use crate::database::{Database, DatabaseTransaction, VideoInfo};
use chrono::DateTime;
use indexmap::IndexMap;
use std::error::Error;
use std::time::Duration;
use teloxide::payloads::{
    EditMessageCaptionSetters, EditMessageReplyMarkupSetters, SendVideoSetters,
};
use teloxide::prelude::Requester;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId};
use teloxide::Bot;
use crate::utils::now_in_my_timezone;

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

pub async fn expire_rejected_content(
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
    let now = now_in_my_timezone(user_settings);

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

pub async fn expire_posted_content(
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
    let now = now_in_my_timezone(user_settings);

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
