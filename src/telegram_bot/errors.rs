use std::error::Error;

use teloxide::prelude::Message;
use teloxide::RequestError;

pub async fn handle_message_is_not_modified_error(result: Result<Message, RequestError>, caption: String) -> Result<(), Box<dyn Error + Send + Sync>> {
    let span = tracing::span!(tracing::Level::INFO, "handle_message_is_not_modified_error");
    let _enter = span.enter();
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.to_string().contains("message is not modified") {
                tracing::warn!("Message is not modified, {}", caption);
                Ok(())
            } else {
                tracing::error!("Error: {}", e);
                Err(Box::new(e))
            }
        }
    }
}
