use crate::database::{Database, DatabaseTransaction, VideoInfo};
use crate::telegram_bot::{UIDefinitions, CHAT_ID};
use crate::utils::now_in_my_timezone;
use chrono::DateTime;
use indexmap::IndexMap;
use std::error::Error;
use std::time::Duration;
use teloxide::payloads::{EditMessageCaptionSetters, EditMessageReplyMarkupSetters, SendVideoSetters};
use teloxide::prelude::Requester;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId};
use teloxide::Bot;

pub async fn clear_sent_messages(bot: Bot, database: Database) -> std::io::Result<()> {
    // Load the video mappings

    let mut tx = database.begin_transaction().unwrap();
    let mut content_mapping = tx.load_content_mapping().unwrap();
    // println!("Trying to clear messages");
    for (message_id, video_info) in &mut content_mapping {
        //println!("Clearing message with ID: {}, status {}", message_id, video_info.status);

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
            println!("Deleting accepted message with ID: {}", message_id);
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    // Update the status to "pending_hidden"
                    println!("Deleted accepted message with ID: {}", message_id);
                }
                Err(_e) => {
                    println!("Error deleting accepted message with ID: {}: {}", message_id, _e);
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
                    println!("Deleted queued message with ID: {}", message_id);
                }
                Err(_e) => {
                    println!("Error deleting queued message with ID: {}: {}", message_id, _e);
                }
            }
        }
    }

    // Save the updated video mappings
    let mut tx = database.begin_transaction().unwrap();
    tx.save_content_mapping(content_mapping).unwrap();

    Ok(())
}
