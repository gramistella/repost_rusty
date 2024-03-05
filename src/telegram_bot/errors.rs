use std::error::Error;

use teloxide::prelude::Message;
use teloxide::RequestError;

pub async fn handle_message_is_not_modified_error(result: Result<Message, RequestError>) -> Result<(), Box<dyn Error + Send + Sync>> {
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.to_string().contains("message is not modified") {
                Ok(())
            } else {
                Err(Box::new(e))
            }
        }
    }
}
