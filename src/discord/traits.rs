use serenity::all::MessageId;

use crate::database::database::{BotStatus, ContentInfo};

pub trait Updatable {
    fn get_last_updated_at(&self) -> &str;
    fn set_last_updated_at(&mut self, updated_at: String);
    fn get_message_id(&self) -> MessageId;
}

impl Updatable for ContentInfo {
    fn get_last_updated_at(&self) -> &str {
        &self.last_updated_at
    }

    fn set_last_updated_at(&mut self, updated_at: String) {
        self.last_updated_at = updated_at;
    }

    fn get_message_id(&self) -> MessageId {
        self.message_id
    }
}

impl Updatable for BotStatus {
    fn get_last_updated_at(&self) -> &str {
        &self.last_updated_at
    }

    fn set_last_updated_at(&mut self, updated_at: String) {
        self.last_updated_at = updated_at;
    }

    fn get_message_id(&self) -> MessageId {
        self.message_id
    }
}
