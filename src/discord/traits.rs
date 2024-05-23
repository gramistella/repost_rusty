#![allow(dead_code)]

use serenity::all::{CreateActionRow, MessageId};
use serenity::async_trait;

use crate::database::database::{BotStatus, ContentInfo, DatabaseTransaction};
use crate::discord::bot::UiDefinitions;
use crate::discord::state::ContentStatus;
use crate::discord::utils::{generate_full_caption, get_failed_buttons, get_pending_buttons, get_published_buttons, get_queued_buttons, get_rejected_buttons};

pub trait Updatable {
    fn get_last_updated_at(&self) -> String;
    fn set_last_updated_at(&mut self, updated_at: String);
    fn get_message_id(&self) -> MessageId;
}

impl Updatable for ContentInfo {
    fn get_last_updated_at(&self) -> String {
        self.last_updated_at.clone()
    }

    fn set_last_updated_at(&mut self, updated_at: String) {
        self.last_updated_at = updated_at;
    }

    fn get_message_id(&self) -> MessageId {
        self.message_id
    }
}

impl Updatable for BotStatus {
    fn get_last_updated_at(&self) -> String {
        self.last_updated_at.clone()
    }

    fn set_last_updated_at(&mut self, updated_at: String) {
        self.last_updated_at = updated_at;
    }

    fn get_message_id(&self) -> MessageId {
        self.message_id
    }
}

#[async_trait]
pub trait ProcessableContent {
    async fn get_status(&self) -> ContentStatus;
    async fn is_shown(&self) -> bool;
    async fn set_status(&mut self, status: ContentStatus);
    fn get_message_id(&self) -> MessageId;
    fn set_message_id(&mut self, message_id: MessageId);
    fn get_last_updated_at(&self) -> String;
    fn set_last_updated_at(&mut self, last_updated_at: String);
    async fn generate_caption(&self, tx: &mut DatabaseTransaction, ui_definitions: &UiDefinitions) -> String;
    async fn generate_buttons(&self, ui_definitions: &UiDefinitions) -> Vec<CreateActionRow>;
    fn get_url(&self) -> &String;
}

impl Updatable for dyn ProcessableContent {
    fn get_last_updated_at(&self) -> String {
        self.get_last_updated_at()
    }

    fn set_last_updated_at(&mut self, updated_at: String) {
        self.set_last_updated_at(updated_at)
    }

    fn get_message_id(&self) -> MessageId {
        self.get_message_id()
    }
}

#[async_trait]
impl ProcessableContent for ContentInfo {
    async fn get_status(&self) -> ContentStatus {
        self.status.clone()
    }

    async fn is_shown(&self) -> bool {
        match self.status {
            ContentStatus::Pending { shown } => shown,
            ContentStatus::Published { shown } => shown,
            ContentStatus::Queued { shown } => shown,
            ContentStatus::Rejected { shown } => shown,
            ContentStatus::Failed { shown } => shown,
            ContentStatus::RemovedFromView => false,
        }
    }
    async fn set_status(&mut self, status: ContentStatus) {
        self.status = status;
    }

    fn get_message_id(&self) -> MessageId {
        self.message_id
    }

    fn set_message_id(&mut self, message_id: MessageId) {
        self.message_id = message_id;
    }

    fn get_last_updated_at(&self) -> String {
        self.last_updated_at.clone()
    }

    fn set_last_updated_at(&mut self, last_updated_at: String) {
        self.last_updated_at = last_updated_at;
    }

    async fn generate_caption(&self, tx: &mut DatabaseTransaction, ui_definitions: &UiDefinitions) -> String {
        let user_settings = tx.load_user_settings().await;
        generate_full_caption(&user_settings, tx, ui_definitions, self).await
    }

    async fn generate_buttons(&self, ui_definitions: &UiDefinitions) -> Vec<CreateActionRow> {
        match self.status {
            ContentStatus::Pending { .. } => get_pending_buttons(ui_definitions),
            ContentStatus::Failed { .. } => get_failed_buttons(ui_definitions),
            ContentStatus::Published { .. } => get_published_buttons(ui_definitions),
            ContentStatus::Queued { .. } => get_queued_buttons(ui_definitions),
            ContentStatus::Rejected { .. } => get_rejected_buttons(ui_definitions),
            ContentStatus::RemovedFromView => {
                vec![]
            }
        }
    }

    fn get_url(&self) -> &String {
        &self.url
    }
}
