use chrono::{DateTime, Duration, TimeDelta, Utc};
use indexmap::IndexMap;
use serenity::all::{Context, CreateActionRow, CreateAttachment, CreateButton, CreateMessage, EditMessage, GetMessages, MessageId};

use crate::discord_bot::bot::{ChannelIdMap, INTERFACE_UPDATE_INTERVAL, POSTED_CHANNEL_ID};
use crate::discord_bot::database::{ContentInfo, DEFAULT_FAILURE_EXPIRATION};
use crate::discord_bot::interactions::InnerEventHandler;
use crate::discord_bot::state::ContentStatus;
use crate::discord_bot::utils::{generate_full_caption, get_failed_buttons, get_pending_buttons, get_posted_buttons, get_queued_buttons, get_rejected_buttons, now_in_my_timezone};

impl InnerEventHandler {
    pub async fn process_pending(self, ctx: &Context, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        //println!("Processing pending content");

        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_pending_buttons(&self.ui_definitions);

        let mut tx = self.database.begin_transaction().await.unwrap();
        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        if content_info.status == (ContentStatus::Pending { shown: true }) {
            if now - last_updated_at.with_timezone(&Utc) > Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                content_info.last_updated_at = now.to_rfc3339();

                let edited_message = EditMessage::new().content(msg_caption).components(msg_buttons);
                ctx.http.edit_message(channel_id, *content_id, &edited_message, vec![]).await.unwrap();
            }
        } else {
            content_info.status = ContentStatus::Pending { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();

            //let status = self.ui_definitions.labels.get("pending_caption").unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            *content_id = msg.id;
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
        //println!("Finished processing pending content");
    }

    pub async fn process_queued(self, ctx: &Context, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        //println!("Processing pending content");

        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let mut msg_buttons = get_queued_buttons(&self.ui_definitions);

        let mut tx = self.database.begin_transaction().await.unwrap();
        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let queued_content = match tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()) {
            Some(queued_content) => queued_content,
            None => match tx.get_posted_content_by_shortcode(content_info.original_shortcode.clone()) {
                Some(_posted_content) => {
                    return self.process_posted(ctx, content_id, content_info).await;
                }
                None => match tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()) {
                    Some(_failed_content) => {
                        return self.process_failed(ctx, content_id, content_info).await;
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
            if now - last_updated_at.with_timezone(&Utc) > Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                content_info.last_updated_at = now.to_rfc3339();

                let edited_message = EditMessage::new().content(msg_caption).components(msg_buttons);
                ctx.http.edit_message(channel_id, *content_id, &edited_message, vec![]).await.unwrap();
            }
        } else {
            content_info.status = ContentStatus::Queued { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();
            //let status = self.ui_definitions.labels.get("pending_caption").unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            *content_id = msg.id;
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
        //println!("Finished processing pending content");
    }

    pub async fn process_rejected(self, ctx: &Context, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        //println!("Processing pending content");
        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_rejected_buttons(&self.ui_definitions);

        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let mut tx = self.database.begin_transaction().await.unwrap();
        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);
        let rejected_content = tx.get_rejected_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        let will_expire_at = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap() + Duration::try_seconds(user_settings.rejected_content_lifespan * 60).unwrap();

        if content_info.status == (ContentStatus::Rejected { shown: true }) {
            if will_expire_at.with_timezone(&Utc) < now {
                content_info.status = ContentStatus::RemovedFromView;
                ctx.http.delete_message(channel_id, *content_id, None).await.unwrap();
            } else if now - last_updated_at.with_timezone(&Utc) > Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                content_info.last_updated_at = now.to_rfc3339();

                let edited_message = EditMessage::new().content(msg_caption).components(msg_buttons);
                ctx.http.edit_message(channel_id, *content_id, &edited_message, vec![]).await.unwrap();
            }
        } else {
            content_info.status = ContentStatus::Rejected { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();

            //let status = self.ui_definitions.labels.get("pending_caption").unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = channel_id.send_message(&ctx.http, video_message).await.unwrap();
            *content_id = msg.id;
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
        //println!("Finished processing pending content");
    }

    pub async fn process_posted(self, ctx: &Context, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        //println!("Processing pending content");

        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_posted_buttons(&self.ui_definitions);

        let mut tx = self.database.begin_transaction().await.unwrap();
        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);
        //let posted_content = tx.get_posted_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        //let will_expire_at = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap() + Duration::try_seconds(user_settings.posted_content_lifespan * 60).unwrap();

        if content_info.status == (ContentStatus::Posted { shown: true }) {
            if now - last_updated_at.with_timezone(&Utc) > Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                content_info.last_updated_at = now.to_rfc3339();

                let edited_message = EditMessage::new().content(msg_caption).components(msg_buttons);
                ctx.http.edit_message(POSTED_CHANNEL_ID, *content_id, &edited_message, vec![]).await.unwrap();
            }
        } else {
            content_info.status = ContentStatus::Posted { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();

            //let status = self.ui_definitions.labels.get("pending_caption").unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = POSTED_CHANNEL_ID.send_message(&ctx.http, video_message).await.unwrap();
            match channel_id.delete_message(&ctx.http, *content_id).await {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("Error deleting message: {:?}", e);
                }
            }
            *content_id = msg.id;

            //let builder = GetMessages::new();
            //let _messages = channel_id.messages(&ctx, builder).await.unwrap();
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();
        //println!("Finished processing pending content");
    }

    pub async fn process_failed(self, ctx: &Context, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        //println!("Processing pending content");
        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions, content_info).await;
        let msg_buttons = get_failed_buttons(&self.ui_definitions);
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        let mut tx = self.database.begin_transaction().await.unwrap();
        let last_updated_at = DateTime::parse_from_rfc3339(&content_info.last_updated_at).unwrap();

        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let failed_content = tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        let will_expire_at = DateTime::parse_from_rfc3339(&failed_content.failed_at).unwrap() + DEFAULT_FAILURE_EXPIRATION;
        if content_info.status == (ContentStatus::Failed { shown: true }) {
            if will_expire_at.with_timezone(&Utc) < now {
                content_info.status = ContentStatus::RemovedFromView;
                ctx.http.delete_message(POSTED_CHANNEL_ID, *content_id, None).await.unwrap();
            } else if now - last_updated_at.with_timezone(&Utc) > Duration::seconds(INTERFACE_UPDATE_INTERVAL.as_secs() as i64) {
                content_info.last_updated_at = now.to_rfc3339();

                let edited_message = EditMessage::new().content(msg_caption).components(msg_buttons);
                match ctx.http.edit_message(POSTED_CHANNEL_ID, *content_id, &edited_message, vec![]).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!("Error editing message: {:?}", e);
                    }
                }
            }
        } else {
            content_info.status = ContentStatus::Failed { shown: true };

            let video_attachment = CreateAttachment::url(&ctx.http, &content_info.url).await.unwrap();

            //let status = self.ui_definitions.labels.get("pending_caption").unwrap();
            let video_message = CreateMessage::new().add_file(video_attachment).content(msg_caption).components(msg_buttons);
            let msg = POSTED_CHANNEL_ID.send_message(&ctx.http, video_message).await.unwrap();
            channel_id.delete_message(&ctx.http, *content_id).await.unwrap();
            *content_id = msg.id;
        }

        let content_mapping = IndexMap::from([(*content_id, content_info.clone())]);
        tx.save_content_mapping(content_mapping).unwrap();

        //println!("Finished processing pending content");
    }
}
