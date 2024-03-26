use std::error::Error;
use std::fmt;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde::de::Visitor;

use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::{dialogue, UpdateFilterExt, UpdateHandler};
use teloxide::prelude::*;
use teloxide::types::MessageId;

use crate::telegram_bot::callbacks::{handle_accepted_view, handle_edit_view, handle_page_view, handle_rejected_view, handle_remove_from_view, handle_settings, handle_undo, handle_video_action};
use crate::telegram_bot::commands::{help, page, settings, start, Command};
use crate::telegram_bot::messages::{receive_caption, receive_hashtags, receive_posted_content_lifespan, receive_posting_interval, receive_random_interval, receive_rejected_content_lifespan};
use crate::telegram_bot::InnerBotManager;

#[derive(Clone, Default, PartialEq, Debug)]
pub enum State {
    #[default]
    StartView,
    PageView,
    SettingsView {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    AcceptedView,
    RejectedView,
    ReceivePostingInterval {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveRandomInterval {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveRejectedContentLifespan {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceivePostedContentLifespan {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveCaption {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    ReceiveHashtags {
        stored_messages_to_delete: Vec<MessageId>,
        original_message_id: MessageId,
    },
    EditView {
        stored_messages_to_delete: Vec<MessageId>,
    },
}

impl InnerBotManager {
    pub fn schema(&mut self) -> UpdateHandler<Box<dyn Error + Send + Sync + 'static>> {
        use dptree::case;

        let command_handler = teloxide::filter_command::<Command, _>()
            .branch(case![Command::Help].endpoint(help))
            .branch(case![Command::Start].endpoint(start))
            .branch(case![Command::Page].endpoint(page))
            .branch(case![Command::Settings].endpoint(settings));

        let message_handler = Update::filter_message()
            .branch(command_handler)
            .branch(case![State::ReceivePostingInterval { stored_messages_to_delete, original_message_id }].endpoint(receive_posting_interval))
            .branch(case![State::ReceiveRandomInterval { stored_messages_to_delete, original_message_id }].endpoint(receive_random_interval))
            .branch(case![State::ReceiveRejectedContentLifespan { stored_messages_to_delete, original_message_id }].endpoint(receive_rejected_content_lifespan))
            .branch(case![State::ReceivePostedContentLifespan { stored_messages_to_delete, original_message_id }].endpoint(receive_posted_content_lifespan))
            .branch(case![State::ReceiveCaption { stored_messages_to_delete, original_message_id }].endpoint(receive_caption))
            .branch(case![State::ReceiveHashtags { stored_messages_to_delete, original_message_id }].endpoint(receive_hashtags));

        let callback_handler = Update::filter_callback_query()
            .branch(case![State::AcceptedView].endpoint(handle_accepted_view))
            .branch(case![State::RejectedView].endpoint(handle_rejected_view))
            .branch(case![State::EditView { stored_messages_to_delete }].endpoint(handle_edit_view))
            .branch(case![State::PageView].endpoint(handle_page_view))
            .branch(case![State::PageView].endpoint(handle_video_action))
            .branch(case![State::PageView].endpoint(handle_undo))
            .branch(case![State::PageView].endpoint(handle_remove_from_view))
            .branch(case![State::SettingsView { stored_messages_to_delete, original_message_id }].endpoint(handle_settings));

        dialogue::enter::<Update, InMemStorage<State>, State, _>().branch(message_handler).branch(callback_handler)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum ContentStatus {
    Waiting,
    RemovedFromView,
    Pending {
        shown: bool,
    },
    Posted {
        shown: bool,
    },
    Queued {
        shown: bool,
    },
    Rejected {
        shown: bool,
    },
    Failed {
        shown: bool,
    },
}

impl Serialize for ContentStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
    {
        let s = match self {
            ContentStatus::Waiting => "waiting".to_string(),
            ContentStatus::Pending { shown } => if *shown { "pending_shown".to_string() } else { "pending_hidden".to_string() },
            ContentStatus::Posted { shown } => if *shown { "posted_shown".to_string() } else { "posted_hidden".to_string() },
            ContentStatus::Queued { shown } => if *shown { "queued_shown".to_string() } else { "queued_hidden".to_string() },
            ContentStatus::Rejected { shown } => if *shown { "rejected_shown".to_string() } else { "rejected_hidden".to_string() },
            ContentStatus::Failed { shown } => if *shown { "failed_shown".to_string() } else { "failed_hidden".to_string() },
            _ => {
                panic!("ContentStatus::to_string() called on an invalid variant {:#?}", self);
            }
        };
        serializer.serialize_str(&s)
    }
}

use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct ContentStatusParseError;

impl fmt::Display for ContentStatusParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "could not parse the provided string")
    }
}

impl Error for ContentStatusParseError {}

impl FromStr for ContentStatus {
    type Err = ContentStatusParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "waiting" => Ok(ContentStatus::Waiting),
            "pending_shown" => Ok(ContentStatus::Pending { shown: true }),
            "pending_hidden" => Ok(ContentStatus::Pending { shown: false }),
            "posted_shown" => Ok(ContentStatus::Posted { shown: true }),
            "posted_hidden" => Ok(ContentStatus::Posted { shown: false }),
            "queued_shown" => Ok(ContentStatus::Queued { shown: true }),
            "queued_hidden" => Ok(ContentStatus::Queued { shown: false }),
            "rejected_shown" => Ok(ContentStatus::Rejected { shown: true }),
            "rejected_hidden" => Ok(ContentStatus::Rejected { shown: false }),
            "failed_shown" => Ok(ContentStatus::Failed { shown: true }),
            "failed_hidden" => Ok(ContentStatus::Failed { shown: false }),
            _ => Err(ContentStatusParseError),
        }
    }
}
impl ContentStatus {
    pub fn to_string(&self) -> String {
        match self {
            ContentStatus::Waiting => "waiting".to_string(),
            ContentStatus::Pending { shown } => if *shown { "pending_shown".to_string() } else { "pending_hidden".to_string() },
            ContentStatus::Posted { shown } => if *shown { "posted_shown".to_string() } else { "posted_hidden".to_string() },
            ContentStatus::Queued { shown } => if *shown { "queued_shown".to_string() } else { "queued_hidden".to_string() },
            ContentStatus::Rejected { shown } => if *shown { "rejected_shown".to_string() } else { "rejected_hidden".to_string() },
            ContentStatus::Failed { shown } => if *shown { "failed_shown".to_string() } else { "failed_hidden".to_string() },
            _ => {
                panic!("ContentStatus::to_string() called on an invalid variant {:#?}", self);
            }
        }
    }
}

struct ContentStatusVisitor;

impl<'de> Visitor<'de> for ContentStatusVisitor {
    type Value = ContentStatus;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string representing ContentStatus")
    }

    fn visit_str<E>(self, value: &str) -> Result<ContentStatus, E>
        where
            E: de::Error,
    {
        match value {
            "waiting" => Ok(ContentStatus::Waiting),
            "pending_shown" => Ok(ContentStatus::Pending { shown: true }),
            "pending_hidden" => Ok(ContentStatus::Pending { shown: false }),
            "posted_shown" => Ok(ContentStatus::Posted { shown: true }),
            "posted_hidden" => Ok(ContentStatus::Posted { shown: false }),
            "queued_shown" => Ok(ContentStatus::Queued { shown: true }),
            "queued_hidden" => Ok(ContentStatus::Queued { shown: false }),
            "rejected_shown" => Ok(ContentStatus::Rejected { shown: true }),
            "rejected_hidden" => Ok(ContentStatus::Rejected { shown: false }),
            "failed_shown" => Ok(ContentStatus::Failed { shown: true }),
            "failed_hidden" => Ok(ContentStatus::Failed { shown: false }),
            _ => Err(de::Error::unknown_variant(value, &["waiting", "pending_shown", "pending_hidden", "posted_shown", "posted_hidden", "queued_shown", "queued_hidden", "rejected_shown", "rejected_hidden", "failed_shown", "failed_hidden"])),
        }
    }
}

impl<'de> Deserialize<'de> for ContentStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ContentStatusVisitor)
    }
}
