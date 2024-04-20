use chrono::{DateTime, Duration, Utc};
use indexmap::IndexMap;
use serenity::all::{ChannelId, Context, CreateActionRow, CreateAttachment, CreateMessage, EditMessage, MessageId};

use crate::discord_bot::bot::{ChannelIdMap, Handler, INTERFACE_UPDATE_INTERVAL, POSTED_CHANNEL_ID};
use crate::discord_bot::database::{ContentInfo, DatabaseTransaction, DEFAULT_FAILURE_EXPIRATION, DEFAULT_POSTED_EXPIRATION};
use crate::discord_bot::state::ContentStatus;
use crate::discord_bot::utils::{generate_full_caption, get_failed_buttons, get_pending_buttons, get_published_buttons, get_queued_buttons, get_rejected_buttons, now_in_my_timezone, randomize_now, should_update_buttons, should_update_caption};

impl Handler {
    pub async fn process_pending(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_pending_buttons(&self.ui_definitions);

        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();

        if content_info.status == (ContentStatus::Pending { shown: true }) {
            if now - last_updated_at.with_timezone(&Utc) >= Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                update_message_if_needed(&ctx, *content_id, channel_id, &msg_caption, msg_buttons).await;
                content_info.last_updated_at = randomize_now(tx).to_rfc3339();
            }
        } else {
            content_info.status = ContentStatus::Pending { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            *content_id = msg.id;
            content_info.last_updated_at = randomize_now(tx).to_rfc3339();
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);

        tx.save_content_mapping(content_mapping).unwrap();
    }

    pub async fn process_queued(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        //println!("Processing pending content");

        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let mut msg_buttons = get_queued_buttons(&self.ui_definitions);

        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();

        let queued_content = match tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()) {
            Some(queued_content) => queued_content,
            None => match tx.get_published_content_by_shortcode(content_info.original_shortcode.clone()) {
                Some(_posted_content) => {
                    return self.process_published(ctx, tx, content_id, content_info).await;
                }
                None => match tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()) {
                    Some(_failed_content) => {
                        return self.process_failed(ctx, tx, content_id, content_info).await;
                    }
                    None => {
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
            if now - last_updated_at.with_timezone(&Utc) >= Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                update_message_if_needed(&ctx, *content_id, channel_id, &msg_caption, msg_buttons).await;
                content_info.last_updated_at = randomize_now(tx).to_rfc3339();
            }
        } else {
            content_info.status = ContentStatus::Queued { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);

            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            *content_id = msg.id;
            content_info.last_updated_at = randomize_now(tx).to_rfc3339();
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
    }

    pub async fn process_rejected(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_rejected_buttons(&self.ui_definitions);

        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();

        let rejected_content = match tx.get_rejected_content_by_shortcode(content_info.original_shortcode.clone()) {
            Some(rejected_content) => rejected_content,
            None => {
                tracing::error!("Content not found in rejected table: {:?}", content_info);
                return;
            }
        };

        let will_expire_at = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap() + Duration::try_seconds(user_settings.rejected_content_lifespan * 60).unwrap();

        if content_info.status == (ContentStatus::Rejected { shown: true }) {
            if will_expire_at.with_timezone(&Utc) < now {
                content_info.status = ContentStatus::RemovedFromView;

                ctx.http.delete_message(channel_id, *content_id, None).await.unwrap();
            } else if now - last_updated_at.with_timezone(&Utc) >= Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                update_message_if_needed(&ctx, *content_id, channel_id, &msg_caption, msg_buttons).await;
                content_info.last_updated_at = randomize_now(tx).to_rfc3339();
            }
        } else {
            content_info.status = ContentStatus::Rejected { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);

            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            *content_id = msg.id;
            content_info.last_updated_at = randomize_now(tx).to_rfc3339();
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
        //println!("Finished processing pending content");
    }

    pub async fn process_published(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_published_buttons(&self.ui_definitions);

        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();

        let published_content = tx.get_published_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        let will_expire_at = DateTime::parse_from_rfc3339(&published_content.published_at).unwrap() + DEFAULT_POSTED_EXPIRATION;

        if content_info.status == (ContentStatus::Published { shown: true }) {
            if will_expire_at.with_timezone(&Utc) < now {
                content_info.status = ContentStatus::RemovedFromView;
                match ctx.http.delete_message(POSTED_CHANNEL_ID, *content_id, None).await {
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
            } else if now - last_updated_at.with_timezone(&Utc) >= Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                update_message_if_needed(&ctx, *content_id, POSTED_CHANNEL_ID, &msg_caption, msg_buttons).await;
                content_info.last_updated_at = randomize_now(tx).to_rfc3339();
            }
        } else {
            content_info.status = ContentStatus::Published { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = POSTED_CHANNEL_ID.send_message(&ctx.http, video_message).await.unwrap();
            match channel_id.delete_message(&ctx.http, *content_id).await {
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
            *content_id = msg.id;
            content_info.last_updated_at = randomize_now(tx).to_rfc3339();
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
    }

    pub async fn process_failed(&self, ctx: &Context, tx: &mut DatabaseTransaction, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_failed_buttons(&self.ui_definitions);

        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();

        let failed_content = tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        let will_expire_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap() + DEFAULT_FAILURE_EXPIRATION;

        if content_info.status == (ContentStatus::Failed { shown: true }) {
            if will_expire_at.with_timezone(&Utc) < now {
                content_info.status = ContentStatus::RemovedFromView;
                ctx.http.delete_message(POSTED_CHANNEL_ID, *content_id, None).await.unwrap();
            } else if now - last_updated_at.with_timezone(&Utc) >= Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                update_message_if_needed(&ctx, *content_id, POSTED_CHANNEL_ID, &msg_caption, msg_buttons).await;
                content_info.last_updated_at = randomize_now(tx).to_rfc3339();
            }
        } else {
            content_info.status = ContentStatus::Failed { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();

            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = POSTED_CHANNEL_ID.send_message(&ctx.http, video_message).await.unwrap();
            match channel_id.delete_message(&ctx.http, *content_id).await {
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
            *content_id = msg.id;

            content_info.last_updated_at = randomize_now(tx).to_rfc3339();
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
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
