use std::error::Error;
use std::sync::Arc;

use teloxide::adaptors::Throttle;
use teloxide::payloads::EditMessageReplyMarkupSetters;
use teloxide::prelude::{ChatId, Message, Requester};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use teloxide::utils::command::BotCommands;
use teloxide::Bot;
use tokio::sync::Mutex;

use crate::database::Database;
use crate::telegram_bot::helpers::clear_sent_messages;
use crate::telegram_bot::{BotDialogue, HandlerResult, NavigationBar, State, UIDefinitions, CHAT_ID};
use crate::utils::now_in_my_timezone;

/// These commands are supported:
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    /// Display this text.
    Start,
    Page,
    Help,
    Settings,
}

pub async fn help(bot: Throttle<Bot>, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, Command::descriptions().to_string()).await?;
    Ok(())
}

pub async fn start(bot: Throttle<Bot>, dialogue: BotDialogue, database: Database, msg: Message) -> HandlerResult {
    if msg.chat.id == ChatId(34957918) {
        bot.send_message(msg.chat.id, format!("Welcome back, {}! 🦀", msg.chat.first_name().unwrap()).to_string()).await?;
        clear_sent_messages(bot, database).await.unwrap();
        dialogue.update(State::PageView).await.unwrap();
    } else {
        bot.send_message(msg.chat.id, "You can't use this bot.".to_string()).await?;
    }

    Ok(())
}

pub async fn page(bot: Throttle<Bot>, dialogue: BotDialogue, database: Database, msg: Message) -> HandlerResult {
    let _ = bot.delete_message(CHAT_ID, msg.id).await;

    if let Some(state) = dialogue.get().await.unwrap() {
        if state != State::PageView {
            return Ok(());
        }
    }

    let _ = clear_sent_messages(bot.clone(), database.clone()).await;
    let mut tx = database.begin_transaction().unwrap();
    let mut user_settings = tx.load_user_settings().unwrap();
    let page_num = msg.text().unwrap().split(" ").collect::<Vec<&str>>()[1].parse::<i32>();
    let page_num = match page_num {
        Ok(num) => num,
        Err(_) => {
            bot.send_message(CHAT_ID, "Invalid page number.".to_string()).await?;
            return Ok(());
        }
    };

    if 0 < page_num && page_num <= tx.get_total_pages().unwrap() {
        user_settings.current_page = page_num;
        tx.save_user_settings(user_settings).unwrap();
    } else {
        bot.send_message(CHAT_ID, "Invalid page number.".to_string()).await?;
    }

    Ok(())
}

pub async fn settings(bot: Throttle<Bot>, dialogue: BotDialogue, msg: Message, database: Database, ui_definitions: UIDefinitions, execution_mutex: Arc<Mutex<()>>, nav_bar_mutex: Arc<Mutex<NavigationBar>>) -> HandlerResult {
    if let Some(mut state) = dialogue.get().await.unwrap() {
        if state != State::PageView {
            let _ = bot.delete_message(CHAT_ID, msg.id).await;
            return Ok(());
        }
    }

    let _guard = execution_mutex.lock().await;
    let navigation_bar = nav_bar_mutex.lock().await;
    let _ = bot.delete_message(CHAT_ID, msg.id).await;
    let _ = clear_sent_messages(bot.clone(), database.clone()).await;
    let _ = bot.delete_message(CHAT_ID, navigation_bar.message_id).await;

    display_settings_message(bot.clone(), dialogue, database, ui_definitions).await?;

    Ok(())
}

pub async fn display_settings_message(bot: Throttle<Bot>, dialogue: BotDialogue, database: Database, ui_definitions: UIDefinitions) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut tx = database.begin_transaction().unwrap();
    let user_settings = tx.load_user_settings().unwrap();
    let mut posting_status = "disabled";
    if user_settings.can_post {
        posting_status = "enabled";
    }

    let settings_title_text = ui_definitions.labels.get("settings_title").unwrap();
    let current_time = now_in_my_timezone(user_settings.clone()).format("%H:%M %m-%d").to_string();
    let timezone_offset = user_settings.timezone_offset.clone();
    let utc_string = if timezone_offset > 0 {
        format!("UTC+{}", timezone_offset)
    } else if timezone_offset < 0 {
        format!("UTC{}", timezone_offset)
    } else {
        "UTC".to_string()
    };
    let settings_message_string = format!(
        "{}  {} {}\n\nPosting is currently {}.\nInterval between posts is {} minutes.\nRandom interval is ±{} minutes\nRejected content expires after {} minutes\nPosted content expires after {} minutes\n\nWhat would you like to change?",
        settings_title_text, current_time, utc_string, posting_status, user_settings.posting_interval, user_settings.random_interval_variance, user_settings.rejected_content_lifespan, user_settings.posted_content_lifespan
    );

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
        InlineKeyboardButton::callback(turn_off_action_text, format!("turn_off_{}", settings_message.id))
    } else {
        InlineKeyboardButton::callback(turn_on_action_text, format!("turn_on_{}", settings_message.id))
    };

    let go_back_action_text = ui_definitions.buttons.get("go_back").unwrap();
    let settings_actions_row_1 = vec![InlineKeyboardButton::callback(go_back_action_text, format!("go_back_{}", settings_message.id)), settings_action_row_1_col_2];

    let adjust_posting_interval_action_text = ui_definitions.buttons.get("adjust_posting_interval").unwrap();
    let settings_actions_row_2 = vec![InlineKeyboardButton::callback(adjust_posting_interval_action_text, format!("adjust_posting_interval_{}", settings_message.id))];

    let adjust_random_interval_action_text = ui_definitions.buttons.get("adjust_random_interval").unwrap();
    let settings_actions_row_3 = vec![InlineKeyboardButton::callback(adjust_random_interval_action_text, format!("adjust_random_interval_{}", settings_message.id))];

    let adjust_rejected_content_lifespan_action_text = ui_definitions.buttons.get("adjust_rejected_content_lifespan").unwrap();
    let settings_actions_row_4 = vec![InlineKeyboardButton::callback(adjust_rejected_content_lifespan_action_text, format!("adjust_rejected_content_lifespan_{}", settings_message.id))];

    let adjust_posted_content_lifespan_action_text = ui_definitions.buttons.get("adjust_posted_content_lifespan").unwrap();
    let settings_actions_row_5 = vec![InlineKeyboardButton::callback(adjust_posted_content_lifespan_action_text, format!("adjust_posted_content_lifespan_{}", settings_message.id))];

    let settings_actions = [settings_actions_row_1, settings_actions_row_2, settings_actions_row_3, settings_actions_row_4, settings_actions_row_5];

    bot.edit_message_reply_markup(CHAT_ID, settings_message.id).reply_markup(InlineKeyboardMarkup::new(settings_actions)).await?;
    Ok(())
}
