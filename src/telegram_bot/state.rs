use std::error::Error;

use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::{dialogue, UpdateFilterExt, UpdateHandler};
use teloxide::prelude::*;
use teloxide::types::MessageId;

use crate::telegram_bot::callbacks::{handle_accepted_view, handle_edit_view, handle_page_view, handle_rejected_view, handle_remove_from_view, handle_settings, handle_undo, handle_video_action};
use crate::telegram_bot::commands::{help, page, settings, start, Command};
use crate::telegram_bot::messages::{receive_caption, receive_hashtags, receive_posted_content_lifespan, receive_posting_interval, receive_random_interval, receive_rejected_content_lifespan};

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

pub fn schema() -> UpdateHandler<Box<dyn Error + Send + Sync + 'static>> {
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
