use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use chrono::{DateTime, Duration, FixedOffset, Utc};
use lazy_static::lazy_static;
use regex::Regex;
use serenity::all::{ChannelId, Context, CreateActionRow, CreateAttachment, CreateMessage, EditMessage, Mention, MessageId};
use tokio::time::sleep;

use crate::database::database::{ContentInfo, DatabaseTransaction, UserSettings, DEFAULT_FAILURE_EXPIRATION, DEFAULT_POSTED_EXPIRATION};
use crate::discord::bot::{ChannelIdMap, Handler};
use crate::discord::state::ContentStatus;
use crate::discord::state::ContentStatus::RemovedFromView;
use crate::discord::utils::{generate_bot_status_caption, generate_full_caption, get_bot_status_buttons, get_failed_buttons, get_pending_buttons, get_published_buttons, get_queued_buttons, get_rejected_buttons, handle_msg_deletion, now_in_my_timezone, should_update_buttons, should_update_caption};
use crate::s3::helper::delete_from_s3;
use crate::{INTERFACE_UPDATE_INTERVAL, MY_DISCORD_ID, POSTED_CHANNEL_ID, STATUS_CHANNEL_ID};

impl Handler {
    pub async fn process_bot_status(&self, ctx: &Context, tx: &mut DatabaseTransaction) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let mut bot_status = tx.load_bot_status().unwrap();
        let content_queue = tx.load_content_queue().unwrap();
        let content_info_vec = tx.load_content_mapping().unwrap();
        let content_queue_len = content_queue.len();

        let msg_caption = generate_bot_status_caption(&bot_status, &content_info_vec, content_queue, now);
        let msg_buttons = get_bot_status_buttons(&bot_status);

        if bot_status.message_id.get() == 1 {
            let msg = CreateMessage::new().content(msg_caption).components(msg_buttons);
            let msg = STATUS_CHANNEL_ID.send_message(&ctx.http, msg).await.unwrap();
            bot_status.message_id = msg.id;
            bot_status.last_updated_at = now.to_rfc3339();
        } else {
            let last_updated_at = DateTime::parse_from_rfc3339(&bot_status.last_updated_at).unwrap();
            if now - last_updated_at.with_timezone(&Utc) >= Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                update_message_if_needed(ctx, bot_status.message_id, STATUS_CHANNEL_ID, &msg_caption, msg_buttons).await;
                bot_status.last_updated_at = now.to_rfc3339();
            }
        }
        
        // find all content in content info that is suitable for queuing
        let queueable_content_count = content_info_vec.iter().filter(|content_info| {
            matches!(&content_info.status, ContentStatus::Pending { .. })
        }).count();

        // Remind the user if the content queue is about to run out
        if content_queue_len <= 2 && bot_status.queue_alert_message_id.get() == 1 && queueable_content_count >= 1 {
            let mention = Mention::from(MY_DISCORD_ID);
            let msg_caption = format!("Hey {mention}, the content queue is about to run out!");
            let msg = CreateMessage::new().content(msg_caption);
            bot_status.queue_alert_message_id = channel_id.send_message(&ctx.http, msg).await.unwrap().id;
        } else if content_queue_len > 2 && bot_status.queue_alert_message_id.get() != 1 {
            let delete_msg_result = channel_id.delete_message(&ctx.http, bot_status.queue_alert_message_id).await;
            handle_msg_deletion(delete_msg_result);
            bot_status.queue_alert_message_id = MessageId::new(1);
        }

        // Notify the user if the bot is halted
        if bot_status.status == 1 && bot_status.halt_alert_message_id.get() == 1 {
            let mention = Mention::from(MY_DISCORD_ID);
            let msg_caption = format!("Hey {mention}, the bot is halted!");
            let msg = CreateMessage::new().content(msg_caption);
            bot_status.halt_alert_message_id = STATUS_CHANNEL_ID.send_message(&ctx.http, msg).await.unwrap().id;
        } else if bot_status.status != 1 && bot_status.halt_alert_message_id.get() != 1 {
            let delete_msg_result = STATUS_CHANNEL_ID.delete_message(&ctx.http, bot_status.halt_alert_message_id).await;
            handle_msg_deletion(delete_msg_result);
            bot_status.halt_alert_message_id = MessageId::new(1);
        }

        tx.save_bot_status(&bot_status).unwrap();
    }

    pub async fn process_pending(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_info: &mut ContentInfo, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();
        let user_settings = tx.load_user_settings().unwrap();

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_pending_buttons(&self.ui_definitions);

        if content_info.status == (ContentStatus::Pending { shown: true }) {
            handle_shown_message_update(ctx, channel_id, content_info, &user_settings, &msg_caption, msg_buttons, global_last_updated_at).await;
        } else {
            content_info.status = ContentStatus::Pending { shown: true };

            let video_attachment = get_video_attachment(ctx, content_info).await;
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            content_info.message_id = msg.id;
            content_info.last_updated_at = now_in_my_timezone(&user_settings).to_rfc3339();
        }
    }

    pub async fn process_queued(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_info: &mut ContentInfo, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>) {
        //println!("Processing pending content");

        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let mut msg_buttons = get_queued_buttons(&self.ui_definitions);

        let queued_content = match tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()) {
            Ok(queued_content) => queued_content,
            Err(_) => match tx.get_published_content_by_shortcode(content_info.original_shortcode.clone()) {
                Ok(_posted_content) => {
                    return self.process_published(ctx, tx, content_info, global_last_updated_at).await;
                }
                Err(_) => match tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()) {
                    Ok(_failed_content) => {
                        return self.process_failed(ctx, tx, content_info, global_last_updated_at).await;
                    }
                    Err(_) => {
                        tracing::error!("Content not found in any table: {:?}", content_info);
                        return;
                    }
                },
            },
        };

        let will_post_at = DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap();

        // Check if the content expires within 30 seconds, if so, set the buttons to an empty vector
        if will_post_at.with_timezone(&Utc) < now + Duration::seconds(30) {
            msg_buttons = vec![];
        }

        if content_info.status == (ContentStatus::Queued { shown: true }) {
            handle_shown_message_update(ctx, channel_id, content_info, &user_settings, &msg_caption, msg_buttons, global_last_updated_at).await;
        } else {
            content_info.status = ContentStatus::Queued { shown: true };

            let video_attachment = get_video_attachment(ctx, content_info).await;
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);

            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            content_info.message_id = msg.id;
            content_info.last_updated_at = now_in_my_timezone(&user_settings).to_rfc3339();
        }
    }

    pub async fn process_rejected(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_info: &mut ContentInfo, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_rejected_buttons(&self.ui_definitions);

        let rejected_content = match tx.get_rejected_content_by_shortcode(content_info.original_shortcode.clone()) {
            Ok(rejected_content) => rejected_content,
            Err(_) => {
                tracing::error!("Content not found in rejected table: {:?}", content_info);
                return;
            }
        };

        let will_expire_at = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap() + Duration::try_seconds((user_settings.rejected_content_lifespan * 60) as i64).unwrap();

        if handle_deletion_due_to_expiration(&self.credentials, ctx, content_info, channel_id, now, will_expire_at).await {
            // If the content was deleted, there is no need to process it further
        } else if content_info.status == (ContentStatus::Rejected { shown: true }) {
            handle_shown_message_update(ctx, channel_id, content_info, &user_settings, &msg_caption, msg_buttons, global_last_updated_at).await;
        } else {
            content_info.status = ContentStatus::Rejected { shown: true };

            let video_attachment = get_video_attachment(ctx, content_info).await;
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);

            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            content_info.message_id = msg.id;
            content_info.last_updated_at = now_in_my_timezone(&user_settings).to_rfc3339();
        }
    }

    pub async fn process_published(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_info: &mut ContentInfo, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_published_buttons(&self.ui_definitions);

        let published_content = tx.get_published_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        let will_expire_at = DateTime::parse_from_rfc3339(&published_content.published_at).unwrap() + DEFAULT_POSTED_EXPIRATION;

        if handle_deletion_due_to_expiration(&self.credentials, ctx, content_info, channel_id, now, will_expire_at).await {
            // If the content was deleted, there is no need to process it further
        } else if content_info.status == (ContentStatus::Published { shown: true }) {
            handle_shown_message_update(ctx, POSTED_CHANNEL_ID, content_info, &user_settings, &msg_caption, msg_buttons, global_last_updated_at).await;
        } else {
            content_info.status = ContentStatus::Published { shown: true };

            let video_attachment = get_video_attachment(ctx, content_info).await;

            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = POSTED_CHANNEL_ID.send_message(&ctx.http, video_message).await.unwrap();

            let delete_msg_result = channel_id.delete_message(&ctx.http, content_info.message_id).await;
            handle_msg_deletion(delete_msg_result);
            content_info.message_id = msg.id;
            content_info.last_updated_at = now_in_my_timezone(&user_settings).to_rfc3339();
        }
    }

    pub async fn process_failed(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_info: &mut ContentInfo, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_failed_buttons(&self.ui_definitions);

        let failed_content = tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        let will_expire_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap() + DEFAULT_FAILURE_EXPIRATION;

        if handle_deletion_due_to_expiration(&self.credentials, ctx, content_info, channel_id, now, will_expire_at).await {
            // If the content was deleted, there is no need to process it further
        } else if content_info.status == (ContentStatus::Failed { shown: true }) {
            handle_shown_message_update(ctx, POSTED_CHANNEL_ID, content_info, &user_settings, &msg_caption, msg_buttons, global_last_updated_at).await;
        } else {
            content_info.status = ContentStatus::Failed { shown: true };

            let video_attachment = get_video_attachment(ctx, content_info).await;

            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = POSTED_CHANNEL_ID.send_message(&ctx.http, video_message).await.unwrap();
            let delete_msg_result = channel_id.delete_message(&ctx.http, content_info.message_id).await;
            handle_msg_deletion(delete_msg_result);
            content_info.message_id = msg.id;
            content_info.last_updated_at = now_in_my_timezone(&user_settings).to_rfc3339();
        }
    }
}

async fn update_message_if_needed(ctx: &Context, content_id: MessageId, channel_id: ChannelId, msg_caption: &String, msg_buttons: Vec<CreateActionRow>) {
    let old_msg = match channel_id.message(&ctx.http, content_id).await {
        Ok(msg) => msg,
        Err(_) => return,
    };

    let mut edited_message = EditMessage::new();
    let mut should_update = false;
    if should_update_caption(old_msg.clone(), msg_caption.clone()).await {
        edited_message = edited_message.content(msg_caption);
        should_update = true;
    }

    if should_update_buttons(old_msg, msg_buttons.clone()).await {
        edited_message = edited_message.components(msg_buttons);
        should_update = true;
    }

    if should_update {
        match ctx.http.edit_message(channel_id, content_id, &edited_message, vec![]).await {
            Ok(_) => {}
            Err(e) => {
                let e = format!("{:?}", e);
                tracing::error!("Error editing message: {}", e);
            }
        }
    }
}

lazy_static! {
    static ref CONTENT_DELETION_REGEX: Regex = Regex::new(r"https?:\/\/[^\/]+\/([^?]+)").unwrap();
    static ref LAST_UPDATED_AT: Arc<Mutex<DateTime<Utc>>> = Arc::new(Mutex::new(Utc::now()));
}

pub async fn handle_content_deletion(credentials: &HashMap<String, String>, ctx: &Context, content_info: &mut ContentInfo, channel_id: ChannelId) {
    content_info.status = RemovedFromView;

    let delete_msg_result = ctx.http.delete_message(channel_id, content_info.message_id, None).await;
    handle_msg_deletion(delete_msg_result);

    let filename = CONTENT_DELETION_REGEX.captures(&content_info.url).unwrap().get(1).unwrap().as_str();
    match delete_from_s3(credentials, filename.to_string()).await {
        Ok(_) => {}
        Err(e) => {
            let e = format!("{:?}", e);
            tracing::error!("Error deleting video from s3: {}", e);
        }
    }
}

async fn handle_shown_message_update(ctx: &Context, channel_id: ChannelId, content_info: &mut ContentInfo, user_settings: &UserSettings, msg_caption: &String, msg_buttons: Vec<CreateActionRow>, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>) {
    let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();
    let now = now_in_my_timezone(user_settings);

    if now - last_updated_at.with_timezone(&Utc) >= Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
        // Get the last_updated_at of the last updated message
        let last_updated_at_last_message = *global_last_updated_at.lock().await;

        // Check if the time difference between now and last_updated_at_last_message is less than half a second
        if (now - last_updated_at_last_message).num_milliseconds() < 333 {
            // If it is, skip the update for this iteration
            return;
        }

        update_message_if_needed(ctx, content_info.message_id, channel_id, msg_caption, msg_buttons).await;
        let instant_after_update = now_in_my_timezone(user_settings);

        *global_last_updated_at.lock().await = instant_after_update;
        content_info.last_updated_at = instant_after_update.to_rfc3339();
    }
}

async fn handle_deletion_due_to_expiration(credentials: &HashMap<String, String>, ctx: &Context, content_info: &mut ContentInfo, channel_id: ChannelId, now: DateTime<Utc>, will_expire_at: DateTime<FixedOffset>) -> bool {
    if will_expire_at.with_timezone(&Utc) < now {
        handle_content_deletion(credentials, ctx, content_info, channel_id).await;
        true
    } else {
        false
    }
}

async fn get_video_attachment(ctx: &Context, content_info: &ContentInfo) -> CreateAttachment {
    match CreateAttachment::url(&ctx.http, &content_info.url).await {
        Ok(attachment) => attachment,
        Err(_) => {
            sleep(Duration::seconds(1).to_std().unwrap()).await;
            match CreateAttachment::url(&ctx.http, &content_info.url).await {
                Ok(attachment) => attachment,
                Err(e) => {
                    tracing::error!("Error creating attachment for url {} {:?}", content_info.url, e);
                    panic!("Error creating attachment for url {} {:?}", content_info.url, e);
                }
            }
        }
    }
}
