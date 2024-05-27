use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use lazy_static::lazy_static;
use regex::Regex;
use serenity::all::{ChannelId, Context, CreateActionRow, CreateButton, CreateMessage, Http, Message};
use serenity::prelude::SerenityError;

use crate::database::database::{BotStatus, ContentInfo, DatabaseTransaction, QueuedContent, UserSettings, DEFAULT_FAILURE_EXPIRATION, DEFAULT_POSTED_EXPIRATION};
use crate::discord::bot::UiDefinitions;
use crate::discord::state::ContentStatus;
use crate::{POSTED_CHANNEL_ID, S3_EXPIRATION_TIME};

pub async fn generate_full_caption(user_settings: &UserSettings, tx: &mut DatabaseTransaction, ui_definitions: &UiDefinitions, content_info: &ContentInfo) -> String {
    // let upper_spacer = "^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^";
    // let upper_spacer = "## nununununununununununununununu";
    let upper_spacer = "### ->->->->->->->->->->->->->->->->->->->->->->";
    let base_caption = format!("{upper_spacer}\n‎\n{}\n‎\n(from @{})\n‎\n{}\n", content_info.caption, content_info.original_author, content_info.hashtags);

    match content_info.status {
        ContentStatus::Queued { .. } => {
            let mut formatted_will_post_at = "".to_string();
            let mut countdown_caption;
            let queued_caption = ui_definitions.labels.get("queued_caption").unwrap();
            match tx.get_queued_content_by_shortcode(&content_info.original_shortcode).await {
                None => {
                    format!("{base_caption}\n{}\n‎\nPosting now...\n\n{}‎", queued_caption, formatted_will_post_at)
                }
                Some(queued_content) => {
                    let will_post_at = DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap();
                    formatted_will_post_at = will_post_at.format("%Y-%m-%d %H:%M:%S").to_string();

                    countdown_caption = countdown_until_expiration(user_settings, will_post_at.with_timezone(&Utc)).await;

                    if countdown_caption.contains("0 hours, 0 minutes and 0 seconds") {
                        countdown_caption = "Posting now...".to_string();
                    }
                    format!("{base_caption}\n{}\nWill post at {}\n\n{}\n‎", queued_caption, formatted_will_post_at, countdown_caption)
                }
            }
        }
        ContentStatus::Pending { .. } => {
            format!("{base_caption}‎")
        }
        ContentStatus::Rejected { .. } => {
            let rejected_caption = ui_definitions.labels.get("rejected_caption").unwrap();
            let rejected_content = match tx.get_rejected_content_by_shortcode(&content_info.original_shortcode).await {
                Some(rejected_content) => rejected_content,
                None => {
                    return format!("{base_caption}\n{}\n‎", rejected_caption);
                }
            };
            let will_expire_at = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap() + Duration::seconds((user_settings.rejected_content_lifespan * 60) as i64);

            let countdown_caption = countdown_until_expiration(user_settings, will_expire_at.with_timezone(&Utc)).await;

            format!("{base_caption}\n{}\n{}\n‎", rejected_caption, countdown_caption)
        }
        ContentStatus::Published { .. } => {
            let published_caption = ui_definitions.labels.get("published_caption").unwrap();
            let published_content = tx.get_published_content_by_shortcode(&content_info.original_shortcode).await.unwrap();
            let published_at = DateTime::parse_from_rfc3339(&published_content.published_at).unwrap().format("%Y-%m-%d %H:%M:%S").to_string();
            let will_expire_at = DateTime::parse_from_rfc3339(&published_content.published_at).unwrap() + DEFAULT_POSTED_EXPIRATION;

            let countdown_caption = countdown_until_expiration(user_settings, will_expire_at.with_timezone(&Utc)).await;

            format!("{base_caption}\n{} at {}\n{}\n‎", published_caption, published_at, countdown_caption)
        }
        ContentStatus::Failed { .. } => {
            let failed_caption = ui_definitions.labels.get("failed_caption").unwrap();
            let failed_content = tx.get_failed_content_by_shortcode(&content_info.original_shortcode).await.unwrap();
            let will_expire_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap() + DEFAULT_FAILURE_EXPIRATION;

            let countdown_caption = countdown_until_expiration(user_settings, will_expire_at.with_timezone(&Utc)).await;
            format!("{base_caption}\n{}\n{}\n‎", failed_caption, countdown_caption)
        }
        _ => {
            panic!("Invalid status {}", content_info.status);
        }
    }
}

pub fn generate_bot_status_caption(user_settings: &UserSettings, bot_status: &BotStatus, content_mapping: Vec<ContentInfo>, content_queue: Vec<QueuedContent>, now: DateTime<Utc>) -> String {
    let mut full_status_string = bot_status.status_message.clone();
    if !bot_status.is_discord_warmed_up {
        full_status_string = format!("{}, discord is still warming up...", full_status_string);
    }

    //
    let content_mapping_len = content_mapping.len();
    let content_mapping_status_string;
    if content_mapping_len == 0 {
        content_mapping_status_string = "Not managing any content right now :3".to_string();
    } else if content_mapping_len == 1 {
        content_mapping_status_string = format!("Currently managing only {} piece of content", content_mapping_len);
    } else {
        content_mapping_status_string = format!("Currently managing {} pieces of content", content_mapping_len);
    }

    // Handle queue string
    let queueable_content = content_mapping.iter().filter(|content| content.status == ContentStatus::Pending { shown: true }).count();
    let content_queue_len = content_queue.len();
    let last_post_time = content_queue.iter().max_by_key(|content| DateTime::parse_from_rfc3339(&content.will_post_at).unwrap()).map(|content| DateTime::parse_from_rfc3339(&content.will_post_at).unwrap());

    let mut content_queue_string;
    if content_queue_len > 0 {
        if content_queue_len == 1 {
            content_queue_string = format!("Currently there is {} queued post", 1);
        } else {
            content_queue_string = format!("Currently there are {} queued posts", content_queue_len);
        }

        if queueable_content > 0 {
            content_queue_string = format!("{}, but you can add up to {} more!", content_queue_string, queueable_content);
        }
        content_queue_string = format!("{}\n\nLast post is scheduled on {}", content_queue_string, last_post_time.unwrap().format("%Y-%m-%d at %H:%M:%S"));
    } else if content_queue_len == 0 && queueable_content == 0 {
        content_queue_string = "Currently there are no queued posts!".to_string();
    } else {
        content_queue_string = "Currently there are no queued posts! You should probably add some because, you know, you can :3".to_string();
    }

    let update_interval = user_settings.interface_update_interval as f64 / 1000.0;
    let update_interval_string = format!("Current interface update interval: {:.2}s", update_interval);

    let formatted_now = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let msg_caption = format!("Bot is {}\n\n{}\n\n{}\n\n{}\n\nLast updated at: {}", full_status_string, update_interval_string, content_mapping_status_string, content_queue_string, formatted_now);

    msg_caption
}

/// Clears all messages in the chat and sets all content statuses to hidden.
pub async fn clear_all_messages(tx: &mut DatabaseTransaction, http: &Arc<Http>, channel_id: ChannelId, is_first_time: bool) {
    let previous_messages = http.get_messages(channel_id, None, None).await.unwrap();
    for message in previous_messages {
        if message.author.bot && message.content.contains("Welcome back! 🦀") && !is_first_time {
            continue;
        }

        http.delete_message(channel_id, message.id, None).await.unwrap();
    }

    for mut content in tx.load_content_mapping().await {
        if content.status == (ContentStatus::Pending { shown: true }) {
            content.status = ContentStatus::Pending { shown: false };
        } else if content.status == (ContentStatus::Queued { shown: true }) {
            content.status = ContentStatus::Queued { shown: false };
        } else if content.status == (ContentStatus::Published { shown: true }) {
            content.status = ContentStatus::Published { shown: false };
        } else if content.status == (ContentStatus::Rejected { shown: true }) {
            content.status = ContentStatus::Rejected { shown: false };
        } else if content.status == (ContentStatus::Failed { shown: true }) {
            content.status = ContentStatus::Failed { shown: false };
        }

        tx.save_content_info(&content).await;
    }
}

pub fn now_in_my_timezone(user_settings: &UserSettings) -> DateTime<Utc> {
    let utc_now = Utc::now();
    let timezone_offset = Duration::try_hours(user_settings.timezone_offset as i64).unwrap();
    utc_now + timezone_offset
}

pub async fn countdown_until_expiration(user_settings: &UserSettings, expiration_datetime: DateTime<Utc>) -> String {
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

pub fn get_edit_buttons(ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
    let go_back = ui_definitions.buttons.get("go_back").unwrap();
    let edit_caption = ui_definitions.buttons.get("edit_caption").unwrap();
    let edit_hashtags = ui_definitions.buttons.get("edit_hashtags").unwrap();
    vec![CreateActionRow::Buttons(vec![CreateButton::new("go_back").label(go_back), CreateButton::new("edit_caption").label(edit_caption), CreateButton::new("edit_hashtags").label(edit_hashtags)])]
}

pub fn get_pending_buttons(ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
    let accept = ui_definitions.buttons.get("accept").unwrap();
    let reject = ui_definitions.buttons.get("reject").unwrap();
    let edit = ui_definitions.buttons.get("edit").unwrap();
    vec![CreateActionRow::Buttons(vec![CreateButton::new("accept").label(accept), CreateButton::new("reject").label(reject), CreateButton::new("edit").label(edit)])]
}

pub fn get_queued_buttons(ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
    let remove_from_queue = ui_definitions.buttons.get("remove_from_queue").unwrap();
    let edit_queued = ui_definitions.buttons.get("edit").unwrap();
    let publish_now = ui_definitions.buttons.get("publish_now").unwrap();
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("remove_from_queue").label(remove_from_queue),
        CreateButton::new("edit_queued").label(edit_queued),
        CreateButton::new("publish_now").label(publish_now),
    ])]
}

pub fn get_rejected_buttons(ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
    let undo = ui_definitions.buttons.get("undo").unwrap();
    let remove_from_view = ui_definitions.buttons.get("remove_from_view").unwrap();
    vec![CreateActionRow::Buttons(vec![CreateButton::new("undo_rejected").label(undo), CreateButton::new("remove_from_view").label(remove_from_view)])]
}

pub fn get_failed_buttons(ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
    let remove_from_view = ui_definitions.buttons.get("remove_from_view").unwrap();
    vec![CreateActionRow::Buttons(vec![CreateButton::new("remove_from_view_failed").label(remove_from_view)])]
}

pub fn get_published_buttons(_ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
    // I initially wanted to add an edit button, which when clicked would show a "Delete from Instagram" button
    // Unfortunately, it appears that deleting and updating reels is not supported via the Instagram API.
    //  let edit = ui_definitions.buttons.get("edit").unwrap();
    // vec![CreateActionRow::Buttons(vec![CreateButton::new("edit_post").label(edit)])]
    vec![]
}

pub fn get_bot_status_buttons(bot_status: &BotStatus) -> Vec<CreateActionRow> {
    if bot_status.status == 1 {
        vec![CreateActionRow::Buttons(vec![CreateButton::new("resume_from_halt").label("Resume")])]
    } else if bot_status.manual_mode {
        vec![CreateActionRow::Buttons(vec![CreateButton::new("disable_manual_mode").label("Disable manual mode")])]
    } else {
        vec![CreateActionRow::Buttons(vec![CreateButton::new("enable_manual_mode").label("Enable manual mode")])]
    }
}

lazy_static! {
    static ref CUSTOM_ID_REGEX: Regex = Regex::new(r#"custom_id: "([^"]+)""#).unwrap();
}

pub async fn should_update_buttons(old_msg: Message, new_buttons: Vec<CreateActionRow>) -> bool {
    fn extract_custom_ids(component_description: &str) -> Vec<String> {
        CUSTOM_ID_REGEX.captures_iter(component_description).filter_map(|cap| cap.get(1).map(|match_| match_.as_str().to_string())).collect()
    }

    let first_component_element = match old_msg.components.first() {
        Some(component) => component,
        None => return !new_buttons.is_empty(),
    };

    let old_components = first_component_element.components.clone();
    let mut old_custom_ids = vec![];
    for old_component in old_components {
        let old_component = format!("{:?}", old_component);
        let custom_ids = extract_custom_ids(&old_component);
        for id in custom_ids {
            old_custom_ids.push(id);
        }
    }

    let mut new_custom_ids = vec![];
    for new_component in new_buttons {
        let new_component = format!("{:?}", new_component);
        let custom_ids = extract_custom_ids(&new_component);
        for id in custom_ids {
            new_custom_ids.push(id);
        }
    }

    !old_custom_ids.eq(&new_custom_ids)
}

pub async fn should_update_caption(old_msg: Message, new_content: String) -> bool {
    let old_content = old_msg.content.clone();
    !old_content.eq(&new_content)
}

pub fn handle_msg_deletion(delete_msg_result: Result<(), SerenityError>) {
    match delete_msg_result {
        Ok(_) => {}
        Err(e) => {
            let e = format!("{:?}", e);
            if e.contains("10008") && e.contains("Unknown Message") {
                // Message was already deleted
            } else {
                tracing::error!("Error deleting message: {}", e);
            }
        }
    }
}

pub async fn prune_expired_content(user_settings: &UserSettings, tx: &mut DatabaseTransaction, content: &mut ContentInfo) -> bool {
    
    match content.status {
        ContentStatus::Queued { .. } => {
            // Don't prune queued content, since a queued content is guaranteed to never expire
        }
        _ => {
            let added_at = DateTime::parse_from_rfc3339(&content.added_at).unwrap();
            if now_in_my_timezone(user_settings) > (added_at + Duration::seconds(S3_EXPIRATION_TIME as i64)) {
                tx.remove_content_info_with_shortcode(&content.original_shortcode).await;
                return true;
            }
        },
    }
    false
}

pub async fn send_message_with_retry(ctx: &Context, channel_id: ChannelId, video_message: CreateMessage) -> Message {
    match channel_id.send_message(&ctx.http, video_message.clone()).await {
        Ok(msg) => msg,
        Err(e) => {
            let e = format!("{:?}", e);
            tracing::warn!("Error sending message: {}", e);
            channel_id.send_message(&ctx.http, video_message).await.unwrap()
        }
    }
}
