use crate::database::Database;
use crate::telegram_bot::{NavigationBar, CHAT_ID, REFRESH_RATE};
use std::sync::Arc;

use teloxide::payloads::EditMessageTextSetters;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::Requester;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use teloxide::Bot;
use tokio::sync::Mutex;

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

pub async fn send_or_replace_navigation_bar(bot: Bot, database: Database, navigation_bar: Arc<Mutex<NavigationBar>>) {
    let mut tx = database.begin_transaction().unwrap();
    let user_settings = tx.load_user_settings().unwrap();
    let current_page = user_settings.current_page;
    let total_pages = ((tx.get_max_records_in_content_info().unwrap() as f64 / user_settings.page_size as f64) + 0.5).floor() as i32;
    let navigation_string = format!("Page {} of {}", current_page, total_pages);

    let mut navigation_actions = Vec::new();

    if current_page > 1 {
        navigation_actions.push(InlineKeyboardButton::callback("Previous page", "previous_page"));
    }
    if current_page < total_pages {
        navigation_actions.push(InlineKeyboardButton::callback("Next page", "next_page"));
    }

    let current_page = tx.load_page().unwrap();
    let mut max_id = MessageId(0);
    for element in current_page {
        if element.0 .0 > max_id.0 {
            max_id = element.0;
        }
    }

    let mut navigation_bar_guard = navigation_bar.lock().await;

    if navigation_bar_guard.last_updated_at < chrono::Utc::now() - REFRESH_RATE || navigation_bar_guard.current_total_pages != total_pages {
        navigation_bar_guard.last_updated_at = chrono::Utc::now();

        if navigation_bar_guard.message_id == MessageId(0) {
            navigation_bar_guard.message_id = bot.send_message(CHAT_ID, navigation_string.clone()).reply_markup(InlineKeyboardMarkup::new([navigation_actions])).await.unwrap().id;
            navigation_bar_guard.last_caption = navigation_string.clone();
        } else {
            if max_id.0 > navigation_bar_guard.message_id.0 {
                match bot.delete_message(CHAT_ID, navigation_bar_guard.message_id).await {
                    Ok(_) => {}
                    Err(e) => {
                        if e.to_string() != "message to delete not found" {
                            //println!("Error deleting message: {}", e);
                        } else {
                            println!("ERROR in helpers.rs: \n{}", e.to_string());
                        }
                    }
                }
                navigation_bar_guard.message_id = bot.send_message(CHAT_ID, navigation_string.clone()).reply_markup(InlineKeyboardMarkup::new([navigation_actions])).await.unwrap().id;
                navigation_bar_guard.last_caption = navigation_string.clone();
            } else {
                if navigation_bar_guard.last_caption != navigation_string {
                    match bot.edit_message_text(CHAT_ID, navigation_bar_guard.message_id, navigation_string.clone()).reply_markup(InlineKeyboardMarkup::new([navigation_actions])).await {
                        Ok(_) => {}
                        Err(e) => {
                            if e.to_string() != "message to edit not found" {
                                //println!("Error editing message: {}", e);
                            } else {
                                println!("ERROR in helpers.rs: \n{}", e.to_string());
                            }
                        }
                    }
                    navigation_bar_guard.last_caption = navigation_string.clone();
                }
            }
        }
    }
}
