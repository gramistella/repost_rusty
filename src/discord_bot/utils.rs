use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use indexmap::IndexMap;
use serenity::all::{ChannelId, CreateActionRow, CreateButton, Http};

use crate::discord_bot::bot::UiDefinitions;
use crate::discord_bot::database::{ContentInfo, Database, UserSettings, DEFAULT_FAILURE_EXPIRATION};
use crate::discord_bot::state::ContentStatus;

pub async fn generate_full_caption(database: &Database, ui_definitions: &UiDefinitions, content_info: &ContentInfo) -> String {
    // let upper_spacer = "^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^~^";
    // let upper_spacer = "## nununununununununununununununu";
    let upper_spacer = "### ->->->->->->->->->->->->->->->->->->->->->->";

    let mut tx = database.begin_transaction().await.unwrap();
    let user_settings = tx.load_user_settings().unwrap();

    match content_info.status {
        ContentStatus::Queued { .. } => {
            let mut formatted_will_post_at = "".to_string();
            let mut countdown_caption = "".to_string();
            match tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()) {
                None => {}
                Some(queued_content) => {
                    let will_post_at = DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap();
                    formatted_will_post_at = will_post_at.format("%Y-%m-%d %H:%M:%S").to_string();

                    countdown_caption = countdown_until_expiration(database, will_post_at.with_timezone(&Utc)).await;

                    if countdown_caption.contains("0 hours, 0 minutes and 0 seconds") {
                        countdown_caption = "Posting now...".to_string();
                    }
                }
            }

            let queued_caption = ui_definitions.labels.get("queued_caption").unwrap();
            if formatted_will_post_at.is_empty() {
                format!("{upper_spacer}\n‎\n{}\n(from @{})\n{}\n\n{}\n‎\nPosting now...\n\n{}‎", content_info.caption, content_info.original_author, content_info.hashtags, queued_caption, formatted_will_post_at)
            } else {
                format!(
                    "{upper_spacer}\n‎\n{}\n(from @{})\n{}\n\n{}\nWill post at {}\n\n{}\n‎",
                    content_info.caption, content_info.original_author, content_info.hashtags, queued_caption, formatted_will_post_at, countdown_caption
                )
            }
        }
        ContentStatus::Pending { .. } => {
            format!("{upper_spacer}\n‎\n{}\n(from @{})\n{}\n‎", content_info.caption, content_info.original_author, content_info.hashtags)
        }
        ContentStatus::Rejected { .. } => {
            let rejected_caption = ui_definitions.labels.get("rejected_caption").unwrap();
            let rejected_content = tx.get_rejected_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
            let will_expire_at = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap() + Duration::seconds(user_settings.rejected_content_lifespan * 60);

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;

            format!("{upper_spacer}\n‎\n{}\n(from @{})\n{}\n\n{}\n{}\n‎", content_info.caption, content_info.original_author, content_info.hashtags, rejected_caption, countdown_caption)
        }
        ContentStatus::Posted { .. } => {
            let posted_caption = ui_definitions.labels.get("posted_caption").unwrap();
            format!("{upper_spacer}\n‎\n{}\n(from @{})\n{}\n\n{}\n‎", content_info.caption, content_info.original_author, content_info.hashtags, posted_caption)
        }
        ContentStatus::Failed { .. } => {
            let failed_caption = ui_definitions.labels.get("failed_caption").unwrap();
            let failed_content = tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
            let will_expire_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap() + DEFAULT_FAILURE_EXPIRATION;

            let countdown_caption = countdown_until_expiration(database, will_expire_at.with_timezone(&Utc)).await;
            format!("{upper_spacer}\n‎\n{}\n(from @{})\n{}\n\n{}\n{}\n‎", content_info.caption, content_info.original_author, content_info.hashtags, failed_caption, countdown_caption)
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
        } else if content.status == (ContentStatus::Posted { shown: true }) {
            content.status = ContentStatus::Posted { shown: false };
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
    let post_now = ui_definitions.buttons.get("post_now").unwrap();
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("remove_from_queue").label(remove_from_queue),
        CreateButton::new("edit_queued").label(edit_queued),
        CreateButton::new("post_now").label(post_now),
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

pub fn get_posted_buttons(_ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
    // I initially wanted to add an edit button, which when clicked would show a "Delete from Instagram" button
    // Unfortunately, it appears that deleting and updating reels is not supported via the Instagram API.
    //  let edit = ui_definitions.buttons.get("edit").unwrap();
    // vec![CreateActionRow::Buttons(vec![CreateButton::new("edit_post").label(edit)])]
    vec![]
}
