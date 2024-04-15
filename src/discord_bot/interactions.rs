use std::ops::Deref;
use std::sync::Arc;

use chrono::Duration;
use serenity::all::{Context, CreateMessage, EditMessage, Interaction, Mention, MessageId, MessageReference};
use tokio::sync::Mutex;

use crate::discord_bot::bot::{ChannelIdMap, UiDefinitions, INTERFACE_UPDATE_INTERVAL, POSTED_CHANNEL_ID};
use crate::discord_bot::database::{ContentInfo, Database, QueuedContent, RejectedContent};
use crate::discord_bot::state::ContentStatus;
use crate::discord_bot::utils::{generate_full_caption, get_edit_buttons, get_pending_buttons, now_in_my_timezone};

#[derive(Clone)]
pub struct InnerEventHandler {
    pub(crate) database: Database,
    pub(crate) ui_definitions: UiDefinitions,
    pub(crate) edited_content: Arc<Mutex<Option<EditedContent>>>,
    pub(crate) interaction_mutex: Arc<Mutex<()>>,
}

impl InnerEventHandler {
    pub async fn interaction_publish_now(self, content_info: &mut ContentInfo) {
        let mut tx = self.database.begin_transaction().await.unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);

        let mut queued_content = tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
        queued_content.will_post_at = (now + Duration::seconds(30)).to_rfc3339();
        tx.save_queued_content(queued_content).unwrap();

        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }
    pub async fn interaction_accepted(self, content_info: &mut ContentInfo) {
        content_info.status = ContentStatus::Queued { shown: true };

        let mut tx = self.database.begin_transaction().await.unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);
        let will_post_at = tx.get_new_post_time().unwrap();
        let queued_content = QueuedContent {
            username: content_info.username.clone(),
            url: content_info.url.clone(),
            caption: content_info.caption.clone(),
            hashtags: content_info.hashtags.clone(),
            original_author: content_info.original_author.clone(),
            original_shortcode: content_info.original_shortcode.clone(),
            last_updated_at: now.to_rfc3339(),
            will_post_at,
        };

        tx.save_queued_content(queued_content).unwrap();

        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_rejected(self, content_info: &mut ContentInfo) {
        content_info.status = ContentStatus::Rejected { shown: true };

        let mut tx = self.database.begin_transaction().await.unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);
        let rejected_content = RejectedContent {
            username: content_info.username.clone(),
            url: content_info.url.clone(),
            caption: content_info.caption.clone(),
            hashtags: content_info.hashtags.clone(),
            original_author: content_info.original_author.clone(),
            original_shortcode: content_info.original_shortcode.clone(),
            last_updated_at: now.to_rfc3339(),
            rejected_at: now.to_rfc3339(),
            expired: false,
        };
        tx.save_rejected_content(rejected_content).unwrap();

        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_remove_from_queue(self, content_info: &mut ContentInfo) {
        content_info.status = ContentStatus::Pending { shown: true };

        let mut tx = self.database.begin_transaction().await.unwrap();
        tx.remove_post_from_queue_with_shortcode(content_info.original_shortcode.clone()).unwrap();

        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);
        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_undo_rejected(self, content_info: &mut ContentInfo) {
        content_info.status = ContentStatus::Pending { shown: true };

        let mut tx = self.database.begin_transaction().await.unwrap();
        tx.remove_rejected_content_with_shortcode(content_info.original_shortcode.clone()).unwrap();

        let user_settings = tx.load_user_settings().unwrap();
        let now = now_in_my_timezone(&user_settings);
        content_info.last_updated_at = (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
    }

    pub async fn interaction_remove_from_view(self, ctx: &Context, content_id: MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        content_info.status = ContentStatus::RemovedFromView;

        ctx.http.delete_message(channel_id, content_id, None).await.unwrap();
    }

    pub async fn interaction_remove_from_view_failed(self, ctx: &Context, content_id: MessageId, content_info: &mut ContentInfo) {
        content_info.status = ContentStatus::RemovedFromView;

        ctx.http.delete_message(POSTED_CHANNEL_ID, content_id, None).await.unwrap();
    }

    pub async fn interaction_go_back(&mut self, ctx: &Context, content_id: MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions.clone(), content_info).await;
        let msg_buttons = get_pending_buttons(&self.ui_definitions);

        let edited_msg = EditMessage::new();
        let edited_msg = edited_msg.content(msg_caption).components(msg_buttons);

        ctx.http.edit_message(channel_id, content_id, &edited_msg, vec![]).await.unwrap();

        *self.edited_content.lock().await = None;
    }

    pub async fn interaction_edit(&mut self, ctx: &Context, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        let msg_caption = generate_full_caption(&self.database, &self.ui_definitions.clone(), content_info).await;
        let msg_buttons = get_edit_buttons(&self.ui_definitions);

        let edited_msg = EditMessage::new();
        let edited_msg = edited_msg.content(msg_caption).components(msg_buttons);

        ctx.http.edit_message(channel_id, *content_id, &edited_msg, vec![]).await.unwrap();
    }

    pub async fn interaction_edit_caption(&mut self, ctx: &Context, interaction: &Interaction, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        let mention = Mention::User(interaction.clone().message_component().unwrap().user.id);
        let referenced_message = MessageReference::from(interaction.clone().message_component().unwrap().message.deref());
        let msg = CreateMessage::new().content(format!(" {mention} - Please enter the new caption for the content.")).reference_message(referenced_message);
        let msg = ctx.http.send_message(channel_id, vec![], &msg).await.unwrap();

        *self.edited_content.lock().await = Some(EditedContent {
            kind: EditedContentKind::Caption,
            content_id: *content_id,
            content_info: content_info.clone(),
            message_to_delete: Some(msg.id),
        });
    }

    pub async fn interaction_edit_hashtags(&mut self, ctx: &Context, interaction: &Interaction, content_id: &mut MessageId, content_info: &mut ContentInfo) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        let mention = Mention::User(interaction.clone().message_component().unwrap().user.id);
        let referenced_message = MessageReference::from(interaction.clone().message_component().unwrap().message.deref());
        let msg = CreateMessage::new().content(format!(" {mention} - Please enter the new hashtags for the content.")).reference_message(referenced_message);
        let msg = ctx.http.send_message(channel_id, vec![], &msg).await.unwrap();

        *self.edited_content.lock().await = Some(EditedContent {
            kind: EditedContentKind::Hashtags,
            content_id: *content_id,
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
    pub(crate) content_id: MessageId,
    pub(crate) content_info: ContentInfo,
    pub(crate) message_to_delete: Option<MessageId>,
}
