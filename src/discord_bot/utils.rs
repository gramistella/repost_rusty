use chrono::{DateTime, Duration, Utc};
use indexmap::IndexMap;
use lazy_static::lazy_static;
use regex::Regex;
use serenity::all::{ChannelId, Context, CreateActionRow, CreateButton, Http, Message, MessageId};
use std::sync::Arc;

use crate::discord_bot::bot::UiDefinitions;
use crate::discord_bot::database::{ContentInfo, Database, DatabaseTransaction, UserSettings, DEFAULT_FAILURE_EXPIRATION, DEFAULT_POSTED_EXPIRATION};
use crate::discord_bot::state::ContentStatus;

pub async fn generate_full_caption(database: &Database, ui_definitions: &UiDefinitions, content_info: &ContentInfo) -> String {
    // let upper_spacer = "^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^";
    // let upper_spacer = "## nununununununununununununununu";
    let upper_spacer = "### ->->->->->->->->->->->->->->->->->->->->->->";
    let base_caption = format!("{upper_spacer}\n‎\n{}\n‎\n(from @{})\n‎\n{}\n", content_info.caption, content_info.original_author, content_info.hashtags);

    let mut tx = database.begin_transaction().await.unwrap();
    let user_settings = tx.load_user_settings().unwrap();

    match content_info.status {
        ContentStatus::Queued { .. } => {
            let mut formatted_will_post_at = "".to_string();
            let mut countdown_caption;
            let queued_caption = ui_definitions.labels.get("queued_caption").unwrap();
            match tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()) {
                None => {
                    format!("{base_caption}\n{}\n‎\nPosting now...\n\n{}‎", queued_caption, formatted_will_post_at)
                }
                Some(queued_content) => {
                    let will_post_at = DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap();
                    formatted_will_post_at = will_post_at.format("%Y-%m-%d %H:%M:%S").to_string();

                    countdown_caption = countdown_until_expiration(database, will_post_at.with_timezone(&Utc)).await;

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
            let rejected_content = match tx.get_rejected_content_by_shortcode(content_info.original_shortcode.clone()) {
                Some(rejected_content) => rejected_content,
                None => {
                    return format!("{base_caption}\n{}\n‎", rejected_caption);
                }
            };
            let will_expire_at = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap() + Duration::seconds(user_settings.rejected_content_lifespan * 60);

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;

            format!("{base_caption}\n{}\n{}\n‎", rejected_caption, countdown_caption)
        }
        ContentStatus::Published { .. } => {
            let published_caption = ui_definitions.labels.get("published_caption").unwrap();
            let published_content = tx.get_published_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
            let published_at = DateTime::parse_from_rfc3339(&published_content.published_at).unwrap().format("%Y-%m-%d %H:%M:%S").to_string();
            let will_expire_at = DateTime::parse_from_rfc3339(&published_content.published_at).unwrap() + DEFAULT_POSTED_EXPIRATION;

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;

            format!("{base_caption}\n{} at {}\n{}\n‎", published_caption, published_at, countdown_caption)
        }
        ContentStatus::Failed { .. } => {
            let failed_caption = ui_definitions.labels.get("failed_caption").unwrap();
            let failed_content = tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
            let will_expire_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap() + DEFAULT_FAILURE_EXPIRATION;

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;
            format!("{base_caption}\n{}\n{}\n‎", failed_caption, countdown_caption)
        }
        _ => {
            panic!("Invalid status {}", content_info.status.to_string());
        }
    }
}

/// Clears all messages in the chat and sets all content statuses to hidden.
pub async fn clear_all_messages(database: &Database, http: &Arc<Http>, channel_id: ChannelId, is_first_time: bool) {
    let previous_messages = http.get_messages(channel_id, None, None).await.unwrap();
    for message in previous_messages {
        if message.author.bot && message.content.contains("Welcome back! 🦀") && !is_first_time {
            continue;
        }

        http.delete_message(channel_id, message.id, None).await.unwrap();
    }

    let mut tx = database.begin_transaction().await.unwrap();
    for (id, mut content) in tx.load_content_mapping().unwrap() {
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

        tx.save_content_mapping(IndexMap::from([(id, content)])).unwrap();
    }
}

pub fn now_in_my_timezone(user_settings: &UserSettings) -> DateTime<Utc> {
    let utc_now = Utc::now();
    let timezone_offset = Duration::try_hours(user_settings.timezone_offset as i64).unwrap();
    utc_now + timezone_offset
}

pub async fn countdown_until_expiration(database: &Database, expiration_datetime: DateTime<Utc>) -> String {
    let user_settings = database.begin_transaction().await.unwrap().load_user_settings().unwrap();
    let now = now_in_my_timezone(&user_settings);
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

lazy_static! {
    static ref CUSTOM_ID_REGEX: Regex = Regex::new(r#"custom_id: "([^"]+)""#).unwrap();
}
pub async fn should_update_buttons(old_msg: Message, new_buttons: Vec<CreateActionRow>) -> bool {
    fn extract_custom_ids(component_description: &str) -> Vec<String> {
        CUSTOM_ID_REGEX.captures_iter(component_description).filter_map(|cap| cap.get(1).map(|match_| match_.as_str().to_string())).collect()
    }

    let first_component_element = match old_msg.components.get(0) {
        Some(component) => component,
        None => return if new_buttons.is_empty() { false } else { true },
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

    if old_custom_ids == new_custom_ids {
        false
    } else {
        true
    }
}

pub async fn should_update_caption(old_msg: Message, new_content: String) -> bool {

    let old_content = old_msg.content.clone();
    if old_content == new_content {
        false
    } else {
        true
    }
}

pub fn randomize_now(tx: &mut DatabaseTransaction) -> DateTime<Utc> {
    let content_mapping = tx.load_content_mapping().unwrap();
    let all_update_times = content_mapping.iter().map(|(_id, content)| DateTime::parse_from_rfc3339(&content.last_updated_at).unwrap().with_timezone(&Utc)).collect::<Vec<DateTime<Utc>>>();

    let max_time = *all_update_times.iter().max().unwrap();

    // 5 seconds is the minimum time between updates to avoid discord rate limiting
    max_time + Duration::seconds(6)
}
