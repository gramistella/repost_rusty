use std::ops::Deref;

use chrono::Duration;
use serenity::all::{Context, CreateMessage, EditMessage, Interaction, Mention, MessageId, MessageReference};

use crate::database::database::{BotStatus, ContentInfo, DatabaseTransaction, QueuedContent, RejectedContent, UserSettings};
use crate::discord::bot::{ChannelIdMap, Handler};
use crate::discord::state::ContentStatus;
use crate::discord::utils::{generate_full_caption, get_edit_buttons, get_pending_buttons, now_in_my_timezone};
use crate::discord::view::handle_content_deletion;
use crate::{INTERFACE_UPDATE_INTERVAL, POSTED_CHANNEL_ID};

impl Handler {
    pub async fn interaction_resume_from_halt(&self, user_settings: &UserSettings, bot_status: &mut BotStatus, tx: &mut DatabaseTransaction) {
        bot_status.status = 0;
        bot_status.status_message = "resuming...".to_string();
        bot_status.last_updated_at = (now_in_my_timezone(&user_settings) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
        tx.save_bot_status(bot_status)
    }

    pub async fn interaction_enable_manual_mode(&self, user_settings: &UserSettings, bot_status: &mut BotStatus, tx: &mut DatabaseTransaction) {
        bot_status.manual_mode = true;
        bot_status.status_message = "manual mode  🟡".to_string();
        bot_status.last_updated_at = (now_in_my_timezone(&user_settings) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
        tx.save_bot_status(bot_status)
    }

    pub async fn interaction_disable_manual_mode(&self, user_settings: &UserSettings, bot_status: &mut BotStatus, tx: &mut DatabaseTransaction) {
        bot_status.manual_mode = false;
        bot_status.status_message = "disabling manual mode...".to_string();
        bot_status.last_updated_at = (now_in_my_timezone(&user_settings) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
        tx.save_bot_status(bot_status)
    }

    pub async fn interaction_publish_now(&self, user_settings: &UserSettings, content_info: &mut ContentInfo, tx: &mut DatabaseTransaction) {
        let now = now_in_my_timezone(&user_settings);

        let mut queued_content = tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        queued_content.will_post_at = (now + Duration::seconds(30)).to_rfc3339();
        tx.save_queued_content(&queued_content);

        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }
    pub async fn interaction_accepted(&self, user_settings: &UserSettings, content_info: &mut ContentInfo, tx: &mut DatabaseTransaction) {
        content_info.status = ContentStatus::Queued { shown: true };

        let now = now_in_my_timezone(&user_settings);
        let will_post_at = tx.get_new_post_time();
        let queued_content = QueuedContent {
            username: content_info.username.clone(),
            url: content_info.url.clone(),
            caption: content_info.caption.clone(),
            hashtags: content_info.hashtags.clone(),
            original_author: content_info.original_author.clone(),
            original_shortcode: content_info.original_shortcode.clone(),
            will_post_at,
        };

        tx.save_queued_content(&queued_content);

        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_rejected(&self, user_settings: &UserSettings, content_info: &mut ContentInfo, tx: &mut DatabaseTransaction) {
        content_info.status = ContentStatus::Rejected { shown: true };

        let now = now_in_my_timezone(&user_settings);
        let rejected_content = RejectedContent {
            username: content_info.username.clone(),
            url: content_info.url.clone(),
            caption: content_info.caption.clone(),
            hashtags: content_info.hashtags.clone(),
            original_author: content_info.original_author.clone(),
            original_shortcode: content_info.original_shortcode.clone(),
            rejected_at: now.to_rfc3339(),
        };
        tx.save_rejected_content(rejected_content);

        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_remove_from_queue(&self, user_settings: &UserSettings, content_info: &mut ContentInfo, tx: &mut DatabaseTransaction) {
        content_info.status = ContentStatus::Pending { shown: true };

        let is_in_queue = tx.does_content_exist_with_shortcode_in_queue(content_info.original_shortcode.clone());
        if is_in_queue {
            tx.remove_post_from_queue_with_shortcode(content_info.original_shortcode.clone());
        }

        let now = now_in_my_timezone(&user_settings);
        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_undo_rejected(&self, user_settings: &UserSettings, content_info: &mut ContentInfo, tx: &mut DatabaseTransaction) {
        content_info.status = ContentStatus::Pending { shown: true };

        tx.remove_rejected_content_with_shortcode(content_info.original_shortcode.clone());

        let now = now_in_my_timezone(&user_settings);
        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_remove_from_view(&self, ctx: &Context, content_info: &mut ContentInfo) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();
        handle_content_deletion(&self.credentials, ctx, content_info, channel_id).await;
    }

    pub async fn interaction_remove_from_view_failed(&self, ctx: &Context, content_info: &mut ContentInfo) {
        handle_content_deletion(&self.credentials, ctx, content_info, POSTED_CHANNEL_ID).await;
    }

    pub async fn interaction_go_back(&self, ctx: &Context, content_info: &mut ContentInfo) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions.clone(), content_info).await;
        let msg_buttons = get_pending_buttons(&self.ui_definitions);

        let edited_msg = EditMessage::new();
        let edited_msg = edited_msg.content(msg_caption).components(msg_buttons);

        ctx.http.edit_message(channel_id, content_info.message_id, &edited_msg, vec![]).await.unwrap();

        *self.edited_content.lock().await = None;
    }

    pub async fn interaction_edit(&self, ctx: &Context, content_info: &mut ContentInfo) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions.clone(), content_info).await;
        let msg_buttons = get_edit_buttons(&self.ui_definitions);

        let edited_msg = EditMessage::new();
        let edited_msg = edited_msg.content(msg_caption).components(msg_buttons);

        ctx.http.edit_message(channel_id, content_info.message_id, &edited_msg, vec![]).await.unwrap();
    }

    pub async fn interaction_edit_caption(&self, ctx: &Context, interaction: &Interaction, content_info: &mut ContentInfo) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();

        let mention = Mention::User(interaction.clone().message_component().unwrap().user.id);
        let referenced_message = MessageReference::from(interaction.clone().message_component().unwrap().message.deref());
        let msg = CreateMessage::new().content(format!(" {mention} - Please enter the new caption for the content.")).reference_message(referenced_message);
        let msg = ctx.http.send_message(channel_id, vec![], &msg).await.unwrap();

        let content_info_dupe = ContentInfo {
            username: content_info.username.clone(),
            message_id: content_info.message_id.clone(),
            url: content_info.url.clone(),
            caption: content_info.caption.clone(),
            hashtags: content_info.hashtags.clone(),
            original_author: content_info.original_author.clone(),
            original_shortcode: content_info.original_shortcode.clone(),
            status: content_info.status.clone(),
            last_updated_at: content_info.last_updated_at.clone(),
            added_at: content_info.added_at.clone(),
            encountered_errors: content_info.encountered_errors.clone(),
        };

        *self.edited_content.lock().await = Some(EditedContent {
            kind: EditedContentKind::Caption,
            content_info: content_info_dupe,
            message_to_delete: Some(msg.id),
        });
    }

    pub async fn interaction_edit_hashtags(&self, ctx: &Context, interaction: &Interaction, content_info: &mut ContentInfo) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();

        let mention = Mention::User(interaction.clone().message_component().unwrap().user.id);
        let referenced_message = MessageReference::from(interaction.clone().message_component().unwrap().message.deref());
        let msg = CreateMessage::new().content(format!(" {mention} - Please enter the new hashtags for the content.")).reference_message(referenced_message);
        let msg = ctx.http.send_message(channel_id, vec![], &msg).await.unwrap();

        *self.edited_content.lock().await = Some(EditedContent {
            kind: EditedContentKind::Hashtags,
            content_info: content_info.clone(),
            message_to_delete: Some(msg.id),
        });
    }
}

#[derive(Clone)]
pub enum EditedContentKind {
    Caption,
    Hashtags,
}
#[derive(Clone)]
pub struct EditedContent {
    /// The kind of content that is being edited.
    /// 0 - Caption
    /// 1 - Hashtags
    pub(crate) kind: EditedContentKind,
    pub(crate) content_info: ContentInfo,
    pub(crate) message_to_delete: Option<MessageId>,
}
