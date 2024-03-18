use std::sync::Arc;

use indexmap::IndexMap;
use teloxide::adaptors::Throttle;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use teloxide::Bot;
use tokio::sync::Mutex;

use crate::database::{ContentInfo, Database, RejectedContent, UserSettings};
use crate::telegram_bot::commands::display_settings_message;
use crate::telegram_bot::helpers::clear_sent_messages;
use crate::telegram_bot::{generate_full_video_caption, process_accepted_shown, BotDialogue, HandlerResult, NavigationBar, State, UIDefinitions};
use crate::utils::now_in_my_timezone;
use crate::REFRESH_RATE;

#[tracing::instrument(skip(bot, dialogue, q, database, ui_definitions, execution_mutex))]
pub async fn handle_page_view(bot: Throttle<Bot>, dialogue: BotDialogue, q: CallbackQuery, database: Database, ui_definitions: UIDefinitions, nav_bar_mutex: Arc<Mutex<NavigationBar>>, execution_mutex: Arc<Mutex<()>>) -> HandlerResult {
    let _mutex_guard = execution_mutex.lock().await;

    let chat_id = q.message.clone().unwrap().chat.id;
    let action = q.data.clone().unwrap();
    let mut tx = database.begin_transaction().unwrap();
    let user_settings = tx.load_user_settings().unwrap();

    match action.as_str() {
        "next_page" => {
            bot.delete_message(chat_id, q.message.unwrap().id).await?;
            clear_sent_messages(bot.clone(), database.clone()).await?;
            tx.load_next_page().unwrap();
            return Ok(());
        }
        "previous_page" => {
            bot.delete_message(chat_id, q.message.unwrap().id).await?;
            clear_sent_messages(bot.clone(), database.clone()).await?;
            tx.load_previous_page().unwrap();
            return Ok(());
        }
        _ => {}
    }

    let (action, _message_id) = parse_callback_query(&q);

    match action.as_str() {
        "remove_from_queue" => {
            handle_remove_from_queue(bot, q, database, ui_definitions).await?;
        }
        "accept" => {
            dialogue.update(State::AcceptedView).await.unwrap();
            handle_accepted_view(bot, dialogue, q, database, ui_definitions).await?;
        }
        "reject" => {
            dialogue.update(State::RejectedView).await.unwrap();
            handle_rejected_view(dialogue, q, database, user_settings).await?;
        }
        "edit" => {
            dialogue.update(State::EditView { stored_messages_to_delete: Vec::new() }).await?;
            handle_edit_view(bot, dialogue, q, database, ui_definitions, nav_bar_mutex).await?;
        }
        "undo" => {
            dialogue.update(State::PageView).await.unwrap();
            handle_undo(bot, q, database, ui_definitions).await?;
        }
        "remove_from_view" => {
            dialogue.update(State::PageView).await.unwrap();
            handle_remove_from_view(bot, dialogue, q, database).await?;
        }
        _ => {}
    }

    Ok(())
}
#[tracing::instrument(skip(bot, dialogue, q, database, ui_definitions))]
pub async fn handle_accepted_view(bot: Throttle<Bot>, dialogue: BotDialogue, q: CallbackQuery, database: Database, ui_definitions: UIDefinitions) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    // Extract the message id from the callback data
    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_content_info_by_message_id(message_id).unwrap();
    //println!("original message id {}, action {}", message_id, action);

    let undo_action_text = ui_definitions.buttons.get("undo").unwrap();
    let undo_action = [InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", message_id))];

    let full_caption = generate_full_video_caption("accepted", &ui_definitions, &mut tx, &video_info);
    bot.edit_message_caption(chat_id, message_id).caption(full_caption.clone()).await?;

    let _msg = bot.edit_message_reply_markup(chat_id, message_id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
    //println!("accepted ids: {}, {}", msg.id, message_id);
    video_info.status = "accepted_shown".to_string();
    let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info.clone())]);
    tx.save_content_mapping(content_mapping).unwrap();

    process_accepted_shown(message_id, &mut video_info).await?;

    dialogue.update(State::PageView).await.unwrap();

    Ok(())
}
#[tracing::instrument(skip(dialogue, q, database, user_settings))]
pub async fn handle_rejected_view(dialogue: BotDialogue, q: CallbackQuery, database: Database, user_settings: UserSettings) -> HandlerResult {
    let _chat_id = q.message.clone().unwrap().chat.id;

    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_content_info_by_message_id(message_id).unwrap();
    //println!("original message id {}, action {}", message_id, action);

    video_info.status = "rejected_shown".to_string();
    let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info.clone())]);
    let mut tx = database.begin_transaction().unwrap();
    tx.save_content_mapping(content_mapping).unwrap();

    let now = now_in_my_timezone(user_settings.clone());

    // Subtract the refresh rate from the current time so that the rejected content is shown immediately
    let last_updated_at = now - REFRESH_RATE;

    let rejected_content = RejectedContent {
        url: video_info.url.clone(),
        caption: video_info.caption.clone(),
        hashtags: video_info.hashtags.clone(),
        original_author: video_info.original_author.clone(),
        original_shortcode: video_info.original_shortcode.clone(),
        rejected_at: now.clone().to_rfc3339(),
        last_updated_at: last_updated_at.clone().to_rfc3339(),
        expired: false,
    };

    let mut tx = database.begin_transaction().unwrap();
    tx.save_rejected_content(rejected_content).unwrap();

    //let _did_expire = expire_rejected_content(&bot, ui_definitions, &mut tx, message_id, &mut video_info).await?;
    //process_rejected_shown(&bot, &ui_definitions, &mut tx, message_id, &mut video_info, caption_body).await?;

    tx.save_content_mapping(IndexMap::from([(message_id, video_info)])).unwrap();
    dialogue.update(State::PageView).await.unwrap();

    Ok(())
}
#[tracing::instrument(skip(bot, q, database, ui_definitions))]
pub async fn handle_undo(bot: Throttle<Bot>, q: CallbackQuery, database: Database, ui_definitions: UIDefinitions) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    // Extract the message id from the callback data
    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_content_info_by_message_id(message_id).unwrap();

    video_info.status = "pending_shown".to_string();
    let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info.clone())]);
    let mut tx = database.begin_transaction().unwrap();
    tx.save_content_mapping(content_mapping).unwrap();

    let full_video_caption = generate_full_video_caption("pending", &ui_definitions, &mut tx, &video_info);
    bot.edit_message_caption(chat_id, message_id).caption(full_video_caption).await?;

    let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
    let reject_action_text = ui_definitions.buttons.get("reject").unwrap();
    let edit_action_text = ui_definitions.buttons.get("edit").unwrap();

    let video_actions = [
        InlineKeyboardButton::callback(accept_action_text, format!("accept_{}", message_id)),
        InlineKeyboardButton::callback(reject_action_text, format!("reject_{}", message_id)),
        InlineKeyboardButton::callback(edit_action_text, format!("edit_{}", message_id)),
    ];

    let _edited_markup = bot.edit_message_reply_markup(chat_id, message_id).reply_markup(InlineKeyboardMarkup::new([video_actions])).await?;

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
#[tracing::instrument(skip(bot, dialogue, q, database))]
pub async fn handle_remove_from_view(bot: Throttle<Bot>, dialogue: BotDialogue, q: CallbackQuery, database: Database) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let video_info = tx.get_content_info_by_message_id(message_id).unwrap();

    tx.remove_content_info_with_shortcode(video_info.original_shortcode).unwrap();

    bot.delete_message(chat_id, message_id).await?;

    dialogue.update(State::PageView).await.unwrap();

    Ok(())
}
#[tracing::instrument(skip(bot, dialogue, q, database, ui_definitions, nav_bar_mutex))]
pub async fn handle_video_action(bot: Throttle<Bot>, dialogue: BotDialogue, q: CallbackQuery, database: Database, ui_definitions: UIDefinitions, nav_bar_mutex: Arc<Mutex<NavigationBar>>) -> HandlerResult {
    if let Some(_data) = &q.data {
        let (action, _message_id) = parse_callback_query(&q);

        if action == "remove_from_queue" {
            handle_remove_from_queue(bot, q, database, ui_definitions).await?;
        } else if action == "accept" {
            dialogue.update(State::AcceptedView).await.unwrap();
            handle_accepted_view(bot, dialogue, q, database, ui_definitions).await?;
        } else if action == "reject" {
            dialogue.update(State::RejectedView).await.unwrap();
            let mut tx = database.begin_transaction().unwrap();
            let user_settings = tx.load_user_settings().unwrap();
            handle_rejected_view(dialogue, q, database, user_settings).await?;
        } else if action == "edit" {
            dialogue.update(State::EditView { stored_messages_to_delete: Vec::new() }).await?;
            handle_edit_view(bot, dialogue, q, database, ui_definitions, nav_bar_mutex).await?;
        } else if action == "undo" {
            dialogue.update(State::PageView).await.unwrap();
            handle_undo(bot, q, database, ui_definitions).await?;
        } else if action == "remove_from_view" {
            dialogue.update(State::PageView).await.unwrap();
            handle_remove_from_view(bot, dialogue, q, database).await?;
        } else {
            println!("Invalid action received: {}", action);
            dialogue.update(State::PageView).await.unwrap();
        }
    }

    Ok(())
}
#[tracing::instrument(skip(bot, dialogue, database, ui_definitions))]
pub async fn handle_settings(bot: Throttle<Bot>, dialogue: BotDialogue, q: CallbackQuery, database: Database, ui_definitions: UIDefinitions) -> HandlerResult {
    let (action, message_id) = parse_callback_query(&q);

    if action == "go_back" {
        bot.delete_message(q.message.unwrap().chat.id, message_id).await?;
        dialogue.update(State::PageView).await.unwrap();
    } else if action == "turn_on" {
        bot.delete_message(q.message.clone().unwrap().chat.id, message_id).await?;
        let mut tx = database.begin_transaction().unwrap();
        let mut user_settings = tx.load_user_settings().unwrap();

        user_settings.can_post = true;

        let mut tx = database.begin_transaction().unwrap();
        tx.save_user_settings(user_settings).unwrap();

        display_settings_message(bot, dialogue, database, ui_definitions).await?;
    } else if action == "turn_off" {
        bot.delete_message(q.message.clone().unwrap().chat.id, message_id).await?;
        let mut tx = database.begin_transaction().unwrap();
        let mut user_settings = tx.load_user_settings().unwrap();
        user_settings.can_post = false;
        let mut tx = database.begin_transaction().unwrap();
        tx.save_user_settings(user_settings).unwrap();

        display_settings_message(bot, dialogue, database, ui_definitions).await?;
    } else if action == "adjust_posting_interval" {
        let msg = bot.send_message(q.message.unwrap().chat.id, "Send your desired interval (in minutes) between posts").await?;

        dialogue
            .update(State::ReceivePostingInterval {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else if action == "adjust_random_interval" {
        let msg = bot.send_message(q.message.unwrap().chat.id, "Send your desired random interval (in minutes) between posts").await?;
        dialogue
            .update(State::ReceiveRandomInterval {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else if action == "adjust_rejected_content_lifespan" {
        let msg = bot.send_message(q.message.unwrap().chat.id, "Send your desired rejected content lifespan (in minutes)").await?;
        dialogue
            .update(State::ReceiveRejectedContentLifespan {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else if action == "adjust_posted_content_lifespan" {
        let msg = bot.send_message(q.message.unwrap().chat.id, "Send your desired posted content lifespan (in minutes)").await?;
        dialogue
            .update(State::ReceivePostedContentLifespan {
                stored_messages_to_delete: vec![msg.id],
                original_message_id: message_id,
            })
            .await
            .unwrap();
    } else {
        println!("invalid action in handle_setting_callback original message id {}, action {}", message_id, action);
    }

    Ok(())
}

#[tracing::instrument(skip(bot, dialogue, q, database, ui_definitions, nav_bar_mutex))]
pub async fn handle_edit_view(bot: Throttle<Bot>, dialogue: BotDialogue, q: CallbackQuery, database: Database, ui_definitions: UIDefinitions, nav_bar_mutex: Arc<Mutex<NavigationBar>>) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    let nav_bar_guard = nav_bar_mutex.lock().await;

    let (action, message_id) = parse_callback_query(&q);
    //println!("handle_edit_view original message id {}, action {}", message_id, action);
    if action == "go_back" {
        // Clear the sent messages, from message_id to the latest message
        bot.delete_message(chat_id, q.message.unwrap().id).await?;
        clear_sent_messages(bot.clone(), database.clone()).await?;
        dialogue.update(State::PageView).await.unwrap();
        return Ok(());
    } else if action == "accept" {
        //println!("accept - message id: {}", message_id);
        // Clear the sent messages, from message_id to the latest message
        let mut tx = database.begin_transaction().unwrap();
        let mut video = tx.get_content_info_by_message_id(message_id).unwrap();

        video.status = "accepted_hidden".to_string();
        tx.save_content_mapping(IndexMap::from([(message_id, video)])).unwrap();

        bot.delete_message(chat_id, q.message.unwrap().id).await?;
        //send_videos(bot.clone(), dialogue.clone(), execution_mutex, database, ui_definitions).await?;

        dialogue.update(State::PageView).await.unwrap();
        return Ok(());
    } else if action == "edit" {
        match bot.delete_message(chat_id, nav_bar_guard.message_id).await {
            Ok(_) => {}
            Err(e) => {
                if e.to_string().contains("MessageToDeleteNotFound") {
                    //println!("Message to delete not found");
                } else {
                    println!("handle_edit_view - Error deleting message: {}", e);
                }
            }
        }
        let mut tx = database.begin_transaction().unwrap();
        let video = tx.get_content_info_by_message_id(message_id).unwrap();
        clear_sent_messages(bot.clone(), database).await.unwrap();

        let go_back_action_text = ui_definitions.buttons.get("go_back").unwrap();
        let edit_caption_action_text = ui_definitions.buttons.get("edit_caption").unwrap();
        let edit_hashtags_action_text = ui_definitions.buttons.get("edit_hashtags").unwrap();
        let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
        let edit_action_row_1 = [InlineKeyboardButton::callback(go_back_action_text, format!("go_back_{}", message_id))];

        let edit_action_row_2 = [InlineKeyboardButton::callback(edit_caption_action_text, format!("edit_caption_{}", message_id))];

        let edit_action_row_3 = [InlineKeyboardButton::callback(edit_hashtags_action_text, format!("edit_hashtags_{}", message_id))];

        let edit_action_row_4 = [InlineKeyboardButton::callback(accept_action_text, format!("accept_{}", message_id))];

        let edit_actions = [edit_action_row_1, edit_action_row_2, edit_action_row_3, edit_action_row_4];

        let msg2 = bot
            .send_message(q.message.clone().unwrap().chat.id, format!("Url: {}\nCaption: {}\nHashtags: {}\n(from @{})", video.url, video.caption, video.hashtags, video.original_author))
            .reply_markup(InlineKeyboardMarkup::new(edit_actions))
            .await?;

        // Update the dialogue with the new state
        dialogue.update(State::EditView { stored_messages_to_delete: vec![msg2.id] }).await?;
    } else if action == "edit_caption" {
        let mut messages_to_delete = retrieve_state_stored_messages(&dialogue).await;

        let caption_message = bot.send_message(chat_id, "Please send your caption.\nUse '!' to empty the field").await?;
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
        let mut messages_to_delete = retrieve_state_stored_messages(&dialogue).await;

        let caption_message = bot.send_message(chat_id, "Please send your hashtags.\nUse '!' to empty the field").await?;
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
#[tracing::instrument(skip(dialogue))]
async fn retrieve_state_stored_messages(dialogue: &BotDialogue) -> Vec<MessageId> {
    let mut messages_to_delete = Vec::new();
    if let State::EditView { stored_messages_to_delete } = dialogue.get().await.unwrap().unwrap() {
        for message_id in stored_messages_to_delete {
            messages_to_delete.push(message_id);
        }
    }
    messages_to_delete
}
#[tracing::instrument]
pub fn parse_callback_query(q: &CallbackQuery) -> (String, MessageId) {
    // Extract the message id from the callback data
    let data_parts: Vec<&str> = q.data.as_ref().unwrap().split('_').collect();

    let (action, message_id) = if data_parts.len() == 5 {
        let action = format!("{}_{}_{}_{}", data_parts[0], data_parts[1], data_parts[2], data_parts[3]);
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
        panic!("Unrecognized callback query data: {}", q.data.as_ref().unwrap());
    };
    let message_id = MessageId(message_id);
    (action, message_id)
}
#[tracing::instrument(skip(bot, q, database, ui_definitions))]
pub async fn handle_remove_from_queue(bot: Throttle<Bot>, q: CallbackQuery, database: Database, ui_definitions: UIDefinitions) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    let (_action, message_id) = parse_callback_query(&q);

    let mut tx = database.begin_transaction().unwrap();
    let mut video_info = tx.get_content_info_by_message_id(message_id).unwrap();

    let mut tx = database.begin_transaction().unwrap();
    tx.remove_post_from_queue_with_shortcode(video_info.original_shortcode.clone()).unwrap();

    video_info.status = "pending_shown".to_string();
    let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info.clone())]);
    let mut tx = database.begin_transaction().unwrap();
    tx.save_content_mapping(content_mapping).unwrap();

    let full_video_caption = generate_full_video_caption("pending", &ui_definitions, &mut tx, &video_info);
    bot.edit_message_caption(chat_id, message_id).caption(full_video_caption).await?;

    let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
    let reject_action_text = ui_definitions.buttons.get("reject").unwrap();
    let edit_action_text = ui_definitions.buttons.get("edit").unwrap();

    let video_actions = [
        InlineKeyboardButton::callback(accept_action_text, format!("accept_{}", message_id)),
        InlineKeyboardButton::callback(reject_action_text, format!("reject_{}", message_id)),
        InlineKeyboardButton::callback(edit_action_text, format!("edit_{}", message_id)),
    ];

    let _edited_markup = bot.edit_message_reply_markup(chat_id, message_id).reply_markup(InlineKeyboardMarkup::new([video_actions])).await?;

    Ok(())
}
