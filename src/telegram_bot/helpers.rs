use std::error::Error;

use chrono::{DateTime, Utc};
use teloxide::adaptors::Throttle;
use teloxide::payloads::EditMessageTextSetters;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::Requester;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use teloxide::Bot;

use crate::database::{ContentInfo, Database, DatabaseTransaction, QueuedContent, DEFAULT_FAILURE_EXPIRATION};
use crate::telegram_bot::state::{ContentStatus, State};
use crate::telegram_bot::{InnerBotManager, UIDefinitions, CHAT_ID};
use crate::utils::now_in_my_timezone;
use crate::INTERFACE_UPDATE_INTERVAL;

pub async fn clear_sent_messages(bot: Throttle<Bot>, database: Database) -> anyhow::Result<()> {
    let span = tracing::span!(tracing::Level::INFO, "clear_sent_messages");
    let _enter = span.enter();

    let mut tx = database.begin_transaction().await.unwrap();
    let mut content_mapping = tx.load_content_mapping().unwrap();
    for (message_id, video_info) in &mut content_mapping {
        tracing::info!("Clearing message with ID: {}, status {}", message_id, video_info.status.to_string());
        if video_info.status == (ContentStatus::Pending { shown: true }) {
            video_info.status = ContentStatus::Pending { shown: false };
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    tracing::info!("Deleted pending message with ID: {}", message_id);
                }
                Err(e) => {
                    tracing::warn!("Error deleting pending message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == (ContentStatus::Rejected { shown: true }) {
            video_info.status = ContentStatus::Rejected { shown: false };
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    tracing::info!("Deleted rejected message with ID: {}", message_id);
                }
                Err(e) => {
                    tracing::warn!("Error deleting rejected message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == (ContentStatus::Posted { shown: true }) {
            video_info.status = ContentStatus::Posted { shown: false };
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    tracing::info!("Deleted posted message with ID: {}", message_id);
                }
                Err(e) => {
                    tracing::warn!("Error deleting posted message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == (ContentStatus::Queued { shown: true }) {
            video_info.status = ContentStatus::Queued { shown: false };
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    tracing::info!("Deleted queued message with ID: {}", message_id);
                }
                Err(e) => {
                    tracing::warn!("Error deleting queued message with ID: {}: {}", message_id, e);
                }
            }
        }

        if video_info.status == (ContentStatus::Failed { shown: true }) {
            video_info.status = ContentStatus::Failed { shown: false };
            match bot.delete_message(CHAT_ID, *message_id).await {
                Ok(_) => {
                    tracing::info!("Deleted failed message with ID: {}", message_id);
                }
                Err(e) => {
                    tracing::warn!("Error deleting failed message with ID: {}: {}", message_id, e);
                }
            }
        }
    }

    // Save the updated video mappings
    tx.save_content_mapping(content_mapping).unwrap();

    Ok(())
}

impl InnerBotManager {
    pub async fn send_or_replace_navigation_bar(&mut self) {
        if self.dialogue.get().await.unwrap() != Some(State::PageView) {
            return;
        }

        let mut tx = self.database.begin_transaction().await.unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let current_page = user_settings.current_page;
        let total_pages = tx.get_total_pages().unwrap();
        let mut navigation_string = format!("Page {} of {}", current_page, total_pages);

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

        let mut navigation_bar_guard = self.nav_bar_mutex.lock().await;
        if navigation_bar_guard.halted {
            navigation_string = format!("⚠️  {}", navigation_string);
        }
        let now = now_in_my_timezone(user_settings);
        if navigation_bar_guard.last_updated_at < now - INTERFACE_UPDATE_INTERVAL || navigation_bar_guard.current_total_pages != total_pages {
            navigation_bar_guard.last_updated_at = now;

            if navigation_bar_guard.message_id == MessageId(0) {
                navigation_bar_guard.message_id = self.bot.send_message(CHAT_ID, navigation_string.clone()).reply_markup(InlineKeyboardMarkup::new([navigation_actions])).await.unwrap().id;
                navigation_bar_guard.last_caption = navigation_string.clone();
            } else {
                if max_id.0 > navigation_bar_guard.message_id.0 {
                    match self.bot.delete_message(CHAT_ID, navigation_bar_guard.message_id).await {
                        Ok(_) => {}
                        Err(e) => {
                            if e.to_string() != "message to delete not found" {
                                //tracing::warn!("Error deleting message: {}", e);
                            } else {
                                tracing::error!("ERROR in helpers.rs: \n{}", e.to_string());
                            }
                        }
                    }
                    navigation_bar_guard.message_id = self.bot.send_message(CHAT_ID, navigation_string.clone()).reply_markup(InlineKeyboardMarkup::new([navigation_actions])).await.unwrap().id;
                    navigation_bar_guard.last_caption = navigation_string.clone();
                } else {
                    if navigation_bar_guard.last_caption != navigation_string {
                        match self.bot.edit_message_text(CHAT_ID, navigation_bar_guard.message_id, navigation_string.clone()).reply_markup(InlineKeyboardMarkup::new([navigation_actions])).await {
                            Ok(_) => {}
                            Err(e) => {
                                if e.to_string() != "message to edit not found" {
                                    tracing::warn!("Error editing message: {}", e);
                                } else {
                                    tracing::error!("ERROR in helpers.rs: \n{}", e.to_string());
                                }
                            }
                        }
                        navigation_bar_guard.last_caption = navigation_string.clone();
                    }
                }
            }
        }
    }
}

pub async fn generate_full_content_caption(database: Database, ui_definitions: UIDefinitions, caption_type: &str, video_info: &ContentInfo) -> String {
    let span = tracing::span!(tracing::Level::INFO, "generate_full_video_caption");
    let _enter = span.enter();
    let caption_body = format!("{}\n{}\n(from @{})", video_info.caption, video_info.hashtags, video_info.original_author);
    let mut tx = database.begin_transaction().await.unwrap();
    match caption_type {
        "accepted" => {
            let accepted_caption = ui_definitions.labels.get("accepted_caption").unwrap();
            let full_video_caption = format!("{}\n\n{}\n\n", caption_body, accepted_caption);
            full_video_caption
        }
        "rejected" => {
            let rejected_caption = ui_definitions.labels.get("rejected_caption").unwrap();
            let rejected_content = tx.get_rejected_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
            let rejected_at = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap();
            let formatted_rejected_at = rejected_at.format("%H:%M %m/%d").to_string();
            let will_expire_at = rejected_at.checked_add_signed(chrono::Duration::try_seconds(tx.load_user_settings().unwrap().rejected_content_lifespan * 60).unwrap()).unwrap();

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;

            let full_video_caption = format!("{}\n\n{} at {}\n\nWill expire in {}", caption_body, rejected_caption, formatted_rejected_at, countdown_caption);
            full_video_caption
        }
        "queued" => {
            let queued_caption = ui_definitions.labels.get("queued_caption").unwrap();
            let queued_content = tx.get_queued_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
            let will_post_at = DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap();
            let formatted_will_post_at = will_post_at.format("%H:%M %m/%d").to_string();

            let mut countdown_caption = countdown_until_expiration(database, will_post_at.with_timezone(&Utc)).await;

            if countdown_caption.contains("0 hours, 0 minutes and 0 seconds") {
                countdown_caption = "Posting now...".to_string();
            }

            let countdown_caption = format!("({})", countdown_caption);

            let full_video_caption = format!(
                "{}\n{}\n(from @{})\n\n{}\n\nWill post at {}\n{}",
                queued_content.caption, queued_content.hashtags, queued_content.original_author, queued_caption, formatted_will_post_at, countdown_caption
            );
            full_video_caption
        }
        "failed" => {
            let failed_caption = ui_definitions.labels.get("failed_caption").unwrap();

            let failed_content = tx.get_failed_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
            let failed_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap();
            let will_expire_at = failed_at.checked_add_signed(chrono::Duration::from_std(DEFAULT_FAILURE_EXPIRATION).unwrap()).unwrap();
            let formatted_failed_at = failed_at.format("%H:%M %m/%d").to_string();

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;

            let full_video_caption = format!("{}\n\n{} at {}\n\nWill expire in {}", caption_body, failed_caption, formatted_failed_at, countdown_caption);
            full_video_caption
        }
        "pending" => caption_body.to_string(),
        "waiting" => caption_body.to_string(),
        "posted" => {
            let posted_caption = ui_definitions.labels.get("posted_caption").unwrap();

            let posted_content = tx.get_posted_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
            let posted_at = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
            let formatted_posted_at = posted_at.format("%H:%M %m/%d").to_string();
            let will_expire_at = posted_at.checked_add_signed(chrono::Duration::try_seconds(tx.load_user_settings().unwrap().posted_content_lifespan * 60).unwrap()).unwrap();

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;

            let full_video_caption = format!("{}\n\n{} at {}\n\nWill expire in {}", caption_body, posted_caption, formatted_posted_at, countdown_caption);
            full_video_caption
        }
        _ => {
            panic!("Unknown caption type: {}", caption_type);
        }
    }
}

async fn countdown_until_expiration(database: Database, expiration_datetime: DateTime<Utc>) -> String {
    let user_settings = database.begin_transaction().await.unwrap().load_user_settings().unwrap();
    let now = now_in_my_timezone(user_settings);
    let duration_until_expiration = expiration_datetime.signed_duration_since(now);

    let mut hours = duration_until_expiration.num_hours();
    let mut minutes = duration_until_expiration.num_minutes() % 60;
    let mut seconds = duration_until_expiration.num_seconds() % 60;

    if hours <= 0 && minutes <= 0 && seconds <= 0 {
        hours = 0;
        minutes = 0;
        seconds = 0;
    }

    let hour_txt = if hours == 1 { "hour" } else { "hours" };
    let minute_txt = if minutes == 1 { "minute" } else { "minutes" };
    let second_txt = if seconds == 1 { "second" } else { "seconds" };

    //ex. 1 hour, 2 minutes and 3 seconds
    format!("{hours} {hour_txt}, {minutes} {minute_txt} and {seconds} {second_txt}")
}

pub fn update_content_status_if_posted(content_info: &mut ContentInfo, tx: &mut DatabaseTransaction, mut queued_content: QueuedContent, now: DateTime<Utc>) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !tx.load_posted_content().unwrap().iter().any(|content| content.original_shortcode == content_info.original_shortcode) {
        queued_content.last_updated_at = now.to_rfc3339();
        tx.save_content_queue(queued_content.clone())?;
    } else {
        content_info.status = ContentStatus::Posted { shown: false };
    }
    Ok(())
}

pub fn create_queued_content(content_info: &mut ContentInfo, last_updated_at: DateTime<Utc>, will_post_at: String) -> QueuedContent {
    let new_queued_post = QueuedContent {
        username: content_info.username.clone(),
        url: content_info.url.clone(),
        caption: content_info.caption.clone(),
        hashtags: content_info.hashtags.clone(),
        original_author: content_info.original_author.clone(),
        original_shortcode: content_info.original_shortcode.clone(),
        last_updated_at: last_updated_at.to_rfc3339(),
        will_post_at,
    };
    new_queued_post
}
