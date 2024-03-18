use std::error::Error;

use teloxide::prelude::Message;
use teloxide::RequestError;

#[tracing::instrument]
pub async fn handle_message_is_not_modified_error(result: Result<Message, RequestError>, caption: String) -> Result<(), Box<dyn Error + Send + Sync>> {
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
