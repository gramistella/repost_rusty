use crate::telegram_bot::commands::display_settings_message;
use crate::telegram_bot::helpers::{clear_sent_messages, expire_rejected_content};
use crate::telegram_bot::{send_videos, BotDialogue, HandlerResult, State, UIDefinitions};
use crate::database::{Database, RejectedContent, UserSettings, VideoInfo};
use indexmap::IndexMap;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use teloxide::Bot;
use tokio::sync::Mutex;
use crate::utils::now_in_my_timezone;

pub async fn handle_accepted_view(
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

pub async fn handle_rejected_view(
    bot: Bot,
    dialogue: BotDialogue,
    q: CallbackQuery,
    database: Database,
    ui_definitions: UIDefinitions,
    user_settings: UserSettings
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
        rejected_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
        expired: false,
    };

    let mut tx = database.begin_transaction().unwrap();
    tx.save_rejected_content(rejected_content).unwrap();

    let _did_expire =
        expire_rejected_content(&bot, ui_definitions, &mut tx, message_id, &mut video_info).await?;

    dialogue.update(State::ScrapeView).await.unwrap();

    Ok(())
}

pub async fn handle_undo(
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
            tx.remove_rejected_content_with_shortcode(video_info.original_shortcode.clone())
                .unwrap();
        }
    }
    //println!("undo pressed, original message id {}, action {}", message_id, action);

    Ok(())
}

pub async fn handle_remove_from_view(
    bot: Bot,
    q: CallbackQuery,
    database: Database,
) -> HandlerResult {
    let chat_id = q.message.clone().unwrap().chat.id;

    let (_action, message_id) = parse_callback_query(&q);
    let mut tx = database.begin_transaction().unwrap();
    let video_info = tx.get_video_info_by_message_id(message_id).unwrap();

    tx.remove_video_info_with_shortcode(video_info.original_shortcode)
        .unwrap();

    bot.delete_message(chat_id, message_id).await?;

    Ok(())
}

pub async fn handle_video_action(
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
            let mut tx = database.begin_transaction().unwrap();
            let user_settings = tx.load_user_settings().unwrap();
            handle_rejected_view(bot, dialogue, q, database, ui_definitions, user_settings).await?;
        } else if action == "edit" {
            dialogue
                .update(State::EditView {
                    stored_messages_to_delete: Vec::new(),
                })
                .await?;
            handle_edit_view(bot, dialogue, q, execution_mutex, database, ui_definitions).await?;
        } else if action == "undo" {
            dialogue.update(State::ScrapeView).await.unwrap();
            handle_undo(bot, q, database, ui_definitions).await?;
        } else if action == "remove_from_view" {
            dialogue.update(State::ScrapeView).await.unwrap();
            handle_remove_from_view(bot, q, database).await?;
        } else {
            println!("Invalid action received: {}", action);
            dialogue.update(State::ScrapeView).await.unwrap();
        }
    }

    Ok(())
}

pub async fn handle_settings(
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

pub async fn handle_edit_view(
    bot: Bot,
    dialogue: BotDialogue,
    q: CallbackQuery,
    execution_mutex: Arc<Mutex<()>>,
    database: Database,
    ui_definitions: UIDefinitions,
) -> HandlerResult {
    {
        let _execution_lock_copy = Arc::clone(&execution_mutex);
        let _execution_lock_copy = _execution_lock_copy.lock().await;
    }

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
    } else if action == "accept" {
        //println!("accept - message id: {}", message_id);
        // Clear the sent messages, from message_id to the latest message
        let mut tx = database.begin_transaction().unwrap();
        let mut video = tx.get_video_info_by_message_id(message_id).unwrap();

        video.status = "accepted_hidden".to_string();
        tx.save_video_info(IndexMap::from([(message_id, video)]))
            .unwrap();

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
        let accept_action_text = ui_definitions.buttons.get("accept").unwrap();
        let edit_action_row_1 = [InlineKeyboardButton::callback(
            go_back_action_text,
            format!("go_back_{}", message_id),
        )];

        let edit_action_row_2 = [InlineKeyboardButton::callback(
            edit_caption_action_text,
            format!("edit_caption_{}", message_id),
        )];

        let edit_action_row_3 = [InlineKeyboardButton::callback(
            edit_hashtags_action_text,
            format!("edit_hashtags_{}", message_id),
        )];

        let edit_action_row_4 = [InlineKeyboardButton::callback(
            accept_action_text,
            format!("accept_{}", message_id),
        )];

        let edit_actions = [
            edit_action_row_1,
            edit_action_row_2,
            edit_action_row_3,
            edit_action_row_4,
        ];

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

pub fn parse_callback_query(q: &CallbackQuery) -> (String, MessageId) {
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

pub async fn handle_remove_from_queue(
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
    tx.remove_post_from_queue_with_shortcode(video_info.original_shortcode.clone())
        .unwrap();

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
