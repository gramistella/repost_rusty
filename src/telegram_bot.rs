use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use teloxide::adaptors::throttle::{Limits, Throttle};
use teloxide::types::{InputFile, MessageId};
use teloxide::{
    dispatching::dialogue::InMemStorage,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};
use tokio::sync::mpsc::Receiver;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::database::{ContentInfo, Database, FailedContent, DEFAULT_FAILURE_EXPIRATION};
use crate::telegram_bot::errors::handle_message_is_not_modified_error;
use crate::telegram_bot::helpers::{generate_full_video_caption, update_content_status_if_posted};
use crate::telegram_bot::state::{ContentStatus, State};
use crate::utils::now_in_my_timezone;
use crate::{CHAT_ID, INTERFACE_UPDATE_INTERVAL, REFRESH_RATE};

mod callbacks;
mod commands;
mod errors;
mod helpers;
mod messages;
pub(crate) mod state;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct UIDefinitions {
    buttons: HashMap<String, String>,
    labels: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct NavigationBar {
    message_id: MessageId,
    current_total_pages: i32,
    halted: bool,
    last_caption: String,
    last_updated_at: DateTime<Utc>,
}
type BotDialogue = Dialogue<State, InMemStorage<State>>;

#[derive(Clone)]
pub struct InnerBotManager {
    bot: Throttle<Bot>,
    dialogue: BotDialogue,
    storage: Arc<InMemStorage<State>>,
    execution_mutex: Arc<Mutex<()>>,
    database: Database,
    ui_definitions: UIDefinitions,
    nav_bar_mutex: Arc<Mutex<NavigationBar>>,
}

impl fmt::Debug for InnerBotManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Convert the bot to a debug string and replace the token

        // This is needed if you want to print the bot field
        // let bot_debug_string = format!("{:?}", self.bot);
        // let redacted_bot_debug_string = bot_debug_string.replace(self.bot.inner().token(), "[REDACTED]");

        f.debug_struct("InnerBotManager")
            //.field("bot", &redacted_bot_debug_string) // Use the redacted string
            .field("dialogue", &self.dialogue)
            //.field("execution_mutex", &self.execution_mutex)
            .field("database", &self.database)
            //.field("ui_definitions", &self.ui_definitions) // This field is omitted
            //.field("nav_bar_mutex", &self.nav_bar_mutex)
            .finish()
    }
}

pub struct BotManager {
    inner: Arc<Mutex<InnerBotManager>>,
}
impl InnerBotManager {
    pub fn new(database: Database, credentials: HashMap<String, String>) -> Self {
        let span = tracing::span!(tracing::Level::INFO, "InnerBotManager::new");
        let _enter = span.enter();

        let api_token = credentials.get("telegram_api_token").unwrap();
        let bot = Bot::new(api_token).throttle(Limits::default());

        let storage = InMemStorage::new();
        let dialogue = BotDialogue::new(storage.clone(), CHAT_ID);

        let ui_definitions_yaml_data = include_str!("../config/ui_definitions.yaml");
        let ui_definitions: UIDefinitions = serde_yaml::from_str(&ui_definitions_yaml_data).expect("Error parsing config file");

        let execution_mutex = Arc::new(Mutex::new(()));

        let nav_bar = NavigationBar {
            message_id: MessageId(0),
            current_total_pages: 0,
            halted: false,
            last_caption: "".to_string(),
            last_updated_at: Utc::now(),
        };

        let nav_bar_mutex = Arc::new(Mutex::new(nav_bar));

        Self {
            bot,
            dialogue,
            storage,
            execution_mutex,
            database,
            ui_definitions,
            nav_bar_mutex,
        }
    }

    pub(crate) async fn run_bot(&mut self, rx: Receiver<(String, String, String, String)>) {
        let mut cloned_self = self.clone();
        tokio::select! {
            _ = cloned_self.start_dispatcher() => {},
            _ = self.receive_videos(rx) => {},
        }
    }

    async fn start_dispatcher(&mut self) {
        let span = tracing::span!(tracing::Level::INFO, "InnerBotManager::start_dispatcher");
        let _enter = span.enter();
        let cloned_mutex = Arc::clone(&self.execution_mutex);
        let _ = cloned_mutex.lock().await;
        let mut dispatcher_builder = Dispatcher::builder(self.bot.clone(), self.schema())
            .dependencies(dptree::deps![self.dialogue.clone(), self.storage.clone(), self.execution_mutex.clone(), self.database.clone(), self.ui_definitions.clone(), self.nav_bar_mutex.clone()])
            .enable_ctrlc_handler()
            .build();
        let dispatcher_future = dispatcher_builder.dispatch();
        dispatcher_future.await;
    }

    async fn receive_videos(&mut self, mut rx: Receiver<(String, String, String, String)>) {
        let span = tracing::span!(tracing::Level::INFO, "receive_videos");
        let _enter = span.enter();

        // Give a head start to the dispatcher
        sleep(Duration::from_secs(1)).await;

        loop {
            let mut tx = self.database.begin_transaction().unwrap();
            if let Some((received_url, received_caption, original_author, original_shortcode)) = rx.recv().await {
                //println!("Received URL: \n\"{}\"\nshortcode: \n\"{}\"\n", received_url, original_shortcode);

                if original_shortcode == "halted" {
                    let mut nav_bar = self.nav_bar_mutex.lock().await;
                    nav_bar.halted = true;
                } else if original_shortcode == "ignore" {
                    // Do nothing
                } else if !tx.does_content_exist_with_shortcode(original_shortcode.clone()) {
                    let mut nav_bar = self.nav_bar_mutex.lock().await;
                    nav_bar.halted = false;

                    let re = regex::Regex::new(r"#\w+").unwrap();
                    let cloned_caption = received_caption.clone();
                    let hashtags: Vec<&str> = re.find_iter(&cloned_caption).map(|mat| mat.as_str()).collect();
                    let hashtags = hashtags.join(" ");

                    let caption = re.replace_all(&received_caption.clone(), "").to_string();
                    let user_settings = tx.load_user_settings().unwrap();

                    let video = ContentInfo {
                        url: received_url.clone(),
                        status: ContentStatus::Waiting,
                        caption,
                        hashtags,
                        original_author: original_author.clone(),
                        original_shortcode: original_shortcode.clone(),
                        last_updated_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
                        url_last_updated_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
                        page_num: 1,
                        encountered_errors: 0,
                    };

                    let message_id = tx.get_temp_message_id(user_settings);
                    let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(MessageId(message_id), video.clone())]);

                    tx.save_content_mapping(content_mapping).unwrap();
                };

                //println!("Received URL: \n\"{}\"\ncaption: \n\"{}\"\n", received_url, received_caption);
            }
            let _success = match self.update_view().await {
                Ok(..) => {
                    tracing::info!("Updated view successfully")
                }
                Err(e) => {
                    tracing::error!("Error updating view, {}", e);
                }
            };
            sleep(REFRESH_RATE).await;
        }
    }
    async fn update_view(&mut self) -> HandlerResult {
        let span = tracing::span!(tracing::Level::INFO, "InnerBotManager::update_view");
        let _enter = span.enter();

        let mutex_clone = Arc::clone(&self.execution_mutex);
        let _mutex_guard = mutex_clone.lock().await;

        let dialogue_state = self.dialogue.get().await.unwrap().unwrap_or_else(|| State::PageView);

        if dialogue_state != State::PageView {
            return Ok(());
        }

        let mut tx = self.database.begin_transaction().unwrap();
        if let Ok(content_mapping) = tx.load_page() {
            for (message_id, mut content_info) in content_mapping {
                let input_file = InputFile::url(content_info.url.parse().unwrap());

                if content_info.encountered_errors > 0 {
                    continue;
                }
                let result = match content_info.status {
                    ContentStatus::Waiting  => self.process_waiting(&mut content_info, input_file).await,
                    ContentStatus::Pending {..} => self.process_pending(message_id, &mut content_info, input_file).await,
                    ContentStatus::Posted {..} => self.process_posted(message_id, &mut content_info, &input_file).await,
                    ContentStatus::Queued {..} => self.process_queued(message_id, &mut content_info, &input_file).await,
                    ContentStatus::Rejected {..} => self.process_rejected(message_id, &mut content_info).await,
                    ContentStatus::Failed {..} => self.process_failed(message_id, &mut content_info, input_file).await,
                    ContentStatus::RemovedFromView => Ok((message_id, content_info.clone())),
                };
                

                match result {
                    Ok((message_id, video_info)) => {
                        if video_info.status == ContentStatus::RemovedFromView {
                            // println!("Removing video from view: {}", video_info.original_shortcode);
                            tx.remove_content_info_with_shortcode(video_info.original_shortcode).unwrap();
                        } else {
                            let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info)]);
                            tx.save_content_mapping(content_mapping).unwrap();
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error processing video {}, with status {}, with error {}", content_info.original_shortcode, content_info.status.to_string(), e);
                    }
                }
            }
        }

        self.send_or_replace_navigation_bar().await;
        Ok(())
    }

    async fn process_waiting(&mut self, video_info: &mut ContentInfo, input_file: InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
        let span = tracing::span!(tracing::Level::INFO, "process_waiting");
        let _enter = span.enter();

        let mut tx = self.database.begin_transaction().unwrap();
        let sent_message_id = match self.send_video_and_get_id(input_file).await {
            Ok(id) => id,
            Err(e) => {
                if e.to_string().contains("wrong file identifier/HTTP URL specified") || e.to_string().contains("failed to get HTTP URL content") {
                    let now = now_in_my_timezone(tx.load_user_settings()?);
                    let failed_content = FailedContent {
                        url: video_info.url.clone(),
                        caption: video_info.caption.clone(),
                        hashtags: video_info.hashtags.clone(),
                        original_author: video_info.original_author.clone(),
                        original_shortcode: video_info.original_shortcode.clone(),
                        last_updated_at: (now - INTERFACE_UPDATE_INTERVAL).to_rfc3339(),
                        failed_at: now.to_rfc3339(),
                        expired: false,
                    };
                    tx.save_failed_content(failed_content)?;
                    video_info.status = ContentStatus::RemovedFromView;
                    return Ok((MessageId(0), video_info.clone()));
                } else {
                    panic!("Error sending video in process_waiting: {}", e);
                }
            }
        };
        let video_actions = self.get_action_buttons(&["accept", "reject", "edit"], sent_message_id);
        video_info.status = ContentStatus::Pending { shown: true };
        let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "pending", video_info);
        self.edit_message_caption_and_markup(CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
        Ok((sent_message_id, video_info.clone()))
    }

    async fn process_failed(&mut self, message_id: MessageId, content_info: &mut ContentInfo, input_file: InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
        
        if content_info.status == (ContentStatus::Failed { shown: true }) {
            let span = tracing::span!(tracing::Level::INFO, "process_failed_shown");
            let _enter = span.enter();

            let mut tx = self.database.begin_transaction().unwrap();
            let mut content = tx.get_failed_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
            // Parse last_updated_at at into a DateTime
            let last_updated_at = DateTime::parse_from_rfc3339(&content.last_updated_at).unwrap();
            let will_expire_at = DateTime::parse_from_rfc3339(&content.failed_at).unwrap().checked_add_signed(chrono::Duration::from_std(DEFAULT_FAILURE_EXPIRATION).unwrap()).unwrap();
            let now = now_in_my_timezone(tx.load_user_settings()?);
            // Check

            if content.expired {
                return Ok((message_id, content_info.clone()));
            }

            if now > will_expire_at {
                content_info.status = ContentStatus::RemovedFromView;
                match self.bot.delete_message(CHAT_ID, message_id).await {
                    Ok(_) => {}
                    Err(_) => {}
                };
                content.expired = true;
            } else if now > last_updated_at + INTERFACE_UPDATE_INTERVAL {
                content_info.status = ContentStatus::Failed { shown: true };

                let full_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "failed", content_info);
                let video_actions = self.get_action_buttons(&["remove_from_view"], message_id);
                self.edit_message_caption_and_markup(CHAT_ID, message_id, full_caption, video_actions).await?;

                content.last_updated_at = now.to_rfc3339();
            }
            tx.update_failed_content(content.clone())?;
            Ok((message_id, content_info.clone()))
        } else {
            let span = tracing::span!(tracing::Level::INFO, "process_failed_hidden");
            let _enter = span.enter();

            let sent_message_id = self.send_video_and_get_id(input_file).await?;
            let video_actions = self.get_action_buttons(&["remove_from_view"], sent_message_id);
            content_info.status = ContentStatus::Failed { shown: true };
            //let full_video_caption = format!("{}\n\n{}\n\n", full_video_caption, ui_definitions.labels.get("failed_caption").unwrap());

            let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "failed", content_info);
            self.edit_message_caption_and_markup(CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
            Ok((sent_message_id, content_info.clone()))
        }
        
    }
  
    async fn process_pending(&mut self, message_id: MessageId, content_info: &mut ContentInfo, input_file: InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
        
        if content_info.status == (ContentStatus::Pending { shown: true }) {
            let span = tracing::span!(tracing::Level::INFO, "process_pending_shown");
            let _enter = span.enter();
            return Ok((message_id, content_info.clone()));
        } else {
            let span = tracing::span!(tracing::Level::INFO, "process_pending_hidden");
            let _enter = span.enter();

            let sent_message_id = self.send_video_and_get_id(input_file).await?;
            let video_actions = self.get_action_buttons(&["accept", "reject", "edit"], sent_message_id);
            content_info.status = ContentStatus::Pending { shown: true };
            let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "pending", content_info);
            self.edit_message_caption_and_markup(CHAT_ID, sent_message_id, full_video_caption, video_actions).await?;
            Ok((sent_message_id, content_info.clone()))
        }
        
    }
    
    async fn process_rejected(&mut self, mut message_id: MessageId, content_info: &mut ContentInfo) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
        
        if content_info.status == (ContentStatus::Rejected { shown: true}) {
            let span = tracing::span!(tracing::Level::INFO, "process_rejected_shown");
            let _enter = span.enter();

            let mut tx = self.database.begin_transaction().unwrap();
            let mut rejected_content = tx.get_rejected_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
            let datetime = DateTime::parse_from_rfc3339(&rejected_content.rejected_at).unwrap();
            //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

            let will_expire_at = datetime.checked_add_signed(chrono::Duration::try_seconds(tx.load_user_settings().unwrap().rejected_content_lifespan * 60).unwrap()).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            if will_expire_at < now_in_my_timezone(user_settings.clone()) {
                content_info.status = ContentStatus::RemovedFromView;
                self.bot.delete_message(CHAT_ID, message_id).await?;
                //tx.remove_content_info_with_shortcode(rejected_content.original_shortcode.clone())?;
                rejected_content.expired = true;
            } else {
                let now = now_in_my_timezone(user_settings.clone());

                let last_updated_at = DateTime::parse_from_rfc3339(&rejected_content.last_updated_at).unwrap();
                if last_updated_at < now - INTERFACE_UPDATE_INTERVAL {
                    let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "rejected", content_info);

                    let _ = match self.bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                        Ok(_) => {
                            let undo_action_text = self.ui_definitions.buttons.get("undo").unwrap();
                            let remove_from_view_action_text = self.ui_definitions.buttons.get("remove_from_view").unwrap();
                            let undo_action = [
                                InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", message_id)),
                                InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", message_id)),
                            ];

                            let _msg = self.bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                        }
                        Err(e) => {
                            println!("Error: {}", e);
                            let new_message = self.bot.send_video(CHAT_ID, InputFile::url(rejected_content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

                            let undo_action_text = self.ui_definitions.buttons.get("undo").unwrap();
                            let remove_from_view_action_text = self.ui_definitions.buttons.get("remove_from_view").unwrap();
                            let undo_action = [
                                InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", new_message.id)),
                                InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id)),
                            ];

                            tx.save_content_mapping(IndexMap::from([(new_message.id, content_info.clone())]))?;

                            let _msg = self.bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                        }
                    };
                    rejected_content.last_updated_at = now.to_rfc3339();
                }
                tx.save_rejected_content(rejected_content.clone())?;
            }

            Ok((message_id, content_info.clone()))
        } else {
            let span = tracing::span!(tracing::Level::INFO, "process_rejected_hidden");
            let _enter = span.enter();

            let mut tx = self.database.begin_transaction().unwrap();
            content_info.status = ContentStatus::Rejected { shown: true };

            let user_settings = tx.load_user_settings()?;
            let expiry_duration = Duration::from_secs((user_settings.rejected_content_lifespan * 60) as u64);
            let now = now_in_my_timezone(user_settings.clone());

            let mut content = tx.get_rejected_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();
            let rejected_at = DateTime::parse_from_rfc3339(&content.rejected_at)?;

            if content.expired {
                return Ok((message_id, content_info.clone()));
            }

            if now > rejected_at + expiry_duration {
                content_info.status = ContentStatus::RemovedFromView;
                match self.bot.delete_message(CHAT_ID, message_id).await {
                    Ok(_) => {
                        // println!("Permanently removed post from video_info: {}", content.url);
                    }
                    Err(_e) => {
                        // This will happen when the message is rejected_hidden and is stored in another page,
                        // as it gets loaded into the current page, the message to delete cannot be found and the whole thing gets stuck
                        // println!(" process_rejected_hidden - Error deleting rejected message with ID: {}: {}", message_id, e);
                    }
                }
                content.expired = true;
                tx.save_rejected_content(content.clone())?;
                //println!("Permanently removed post from video_info: {}", content.url);
            } else {
                content_info.status = ContentStatus::Rejected { shown: true };

                let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "rejected", content_info);
                let new_message = self.bot.send_video(CHAT_ID, InputFile::url(content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

                let undo_action_text = self.ui_definitions.buttons.get("undo").unwrap();
                let remove_from_view_action_text = self.ui_definitions.buttons.get("remove_from_view").unwrap();
                let undo_action = [
                    InlineKeyboardButton::callback(undo_action_text, format!("undo_{}", new_message.id)),
                    InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id)),
                ];

                message_id = new_message.id;

                let _msg = self.bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
            }

            Ok((message_id, content_info.clone()))
        }
    }

    async fn process_queued(&mut self, message_id: MessageId, content_info: &mut ContentInfo, input_file: &InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
        
        if content_info.status == (ContentStatus::Queued { shown: true }) {
            let span = tracing::span!(tracing::Level::INFO, "process_queued_shown");
            let _enter = span.enter();

            let mut tx = self.database.begin_transaction().unwrap();
            let queued_content = tx.get_queued_content_by_shortcode(content_info.original_shortcode.clone()).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            let now = now_in_my_timezone(user_settings.clone());

            let last_updated_at = DateTime::parse_from_rfc3339(&queued_content.last_updated_at).unwrap();

            let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "queued", content_info);

            if last_updated_at < now - INTERFACE_UPDATE_INTERVAL {
                let remove_from_queue_action_text = self.ui_definitions.buttons.get("remove_from_queue").unwrap();
                let _ = match self.bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                    Ok(_) => {
                        let remove_action = [InlineKeyboardButton::callback(remove_from_queue_action_text, format!("remove_from_queue_{}", message_id))];
                        if full_video_caption.contains("(Posting now...)") {
                            // We don't want to show the remove button if the video is being posted
                        } else {
                            let _msg = self.bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_action])).await?;
                        }
                    }
                    Err(e) => {
                        let error_message = e.to_string();

                        if error_message.contains("message is not modified") {
                            // This doesn't need to be logged
                            //tracing::warn!("Message is not modified: {}", e);
                        } else if error_message.contains("A network error: error sending request") {
                            // This might not need to be logged
                            tracing::warn!("Error sending request: {}", e);
                        } else {
                            tracing::warn!("Error editing message caption: {}", e);
                            let new_message = self.bot.send_video(CHAT_ID, InputFile::url(queued_content.url.clone().parse().unwrap())).caption(full_video_caption.clone()).await?;
                            let undo_action = [InlineKeyboardButton::callback(remove_from_queue_action_text, format!("remove_from_queue_{}", new_message.id))];
                            tx.save_content_mapping(IndexMap::from([(new_message.id, content_info.clone())]))?;
                            if full_video_caption.contains("(Posting now...)") {
                                // We don't want to show the remove button if the video is being posted
                            } else {
                                let _msg = self.bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                            }
                        }
                    }
                };
                update_content_status_if_posted(content_info, &mut tx, queued_content, now)?;
            } else {
                //println!("No need to update the message");
            }
            Ok((message_id, content_info.clone()))
        } else {
            let span = tracing::span!(tracing::Level::INFO, "process_queued_hidden");
            let _enter = span.enter();

            content_info.status = ContentStatus::Queued { shown: true };

            let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "queued", content_info);

            let sent_message_id = self.send_video_and_get_id(input_file.clone()).await?;
            let remove_from_queue_action = self.get_action_buttons(&["remove_from_queue"], sent_message_id);
            self.edit_message_caption_and_markup(CHAT_ID, sent_message_id, full_video_caption, remove_from_queue_action).await?;
            Ok((sent_message_id, content_info.clone()))
        }
    }
    
    async fn process_posted(&mut self, mut message_id: MessageId, video_info: &mut ContentInfo, input_file: &InputFile) -> Result<(MessageId, ContentInfo), Box<dyn Error + Send + Sync>> {
        
        if video_info.status == (ContentStatus::Posted {shown: true}) {
            let span = tracing::span!(tracing::Level::INFO, "process_posted_shown");
            let _enter = span.enter();

            //println!("process_posted_shown - Message ID: {}", message_id);
            let mut tx = self.database.begin_transaction().unwrap();
            let mut posted_content = tx.get_posted_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();
            let posted_at = DateTime::parse_from_rfc3339(&posted_content.posted_at).unwrap();
            //let formatted_datetime = datetime.format("%m-%d %H:%M").to_string();

            let will_expire_at = posted_at.checked_add_signed(chrono::Duration::try_seconds(tx.load_user_settings().unwrap().posted_content_lifespan * 60).unwrap()).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            if will_expire_at < now_in_my_timezone(user_settings.clone()) {
                video_info.status = ContentStatus::RemovedFromView;
                self.bot.delete_message(CHAT_ID, message_id).await?;
                posted_content.expired = true;
                // println!("Posted content has expired");
            } else {
                let now = now_in_my_timezone(user_settings.clone());

                let last_updated_at = DateTime::parse_from_rfc3339(&posted_content.last_updated_at).unwrap();
                if last_updated_at < now - INTERFACE_UPDATE_INTERVAL {
                    let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "posted", video_info);
                    let _ = match self.bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                        Ok(_) => {
                            let remove_from_view_action_text = self.ui_definitions.buttons.get("remove_from_view").unwrap();
                            let remove_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", message_id))];

                            let _msg = self.bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_action])).await?;
                        }
                        Err(_) => {
                            let new_message = self.bot.send_video(CHAT_ID, InputFile::url(posted_content.url.clone().parse().unwrap())).caption(full_video_caption).await?;

                            let remove_from_view_action_text = self.ui_definitions.buttons.get("remove_from_view").unwrap();
                            let undo_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id))];

                            tx.save_content_mapping(IndexMap::from([(new_message.id, video_info.clone())]))?;

                            let _msg = self.bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                        }
                    };
                    posted_content.last_updated_at = now.to_rfc3339();
                    tx.save_posted_content(posted_content.clone())?;
                } else {
                    //println!("No need to update the message");
                }
            }

            Ok((message_id, video_info.clone()))
        } else {
            let span = tracing::span!(tracing::Level::INFO, "process_posted_hidden");
            let _enter = span.enter();
            //println!("process_posted_hidden - Message ID: {}", message_id);
            let mut tx = self.database.begin_transaction().unwrap();
            video_info.status = ContentStatus::Posted { shown: true };

            let mut posted_content = tx.get_posted_content_by_shortcode(video_info.original_shortcode.clone()).unwrap();

            let user_settings = tx.load_user_settings().unwrap();
            let now = now_in_my_timezone(user_settings.clone());

            let full_video_caption = generate_full_video_caption(self.database.clone(), self.ui_definitions.clone(), "posted", video_info);
            match self.bot.edit_message_caption(CHAT_ID, message_id).caption(full_video_caption.clone()).await {
                // If the message is already sent and changes status we can edit the caption
                Ok(_) => {
                    let remove_from_view_action_text = self.ui_definitions.buttons.get("remove_from_view").unwrap();
                    let remove_from_view_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", message_id))];

                    self.bot.edit_message_reply_markup(CHAT_ID, message_id).reply_markup(InlineKeyboardMarkup::new([remove_from_view_action])).await?;

                    let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(message_id, video_info.clone())]);
                    tx.save_content_mapping(content_mapping).unwrap();
                }
                // If the message is not sent, we need to send it and save the new message ID
                Err(_e) => {
                    if _e.to_string().contains("message to edit not found") {
                        let new_message = self.bot.send_video(CHAT_ID, input_file.to_owned()).caption(full_video_caption.clone()).await.unwrap();

                        let remove_from_view_action_text = self.ui_definitions.buttons.get("remove_from_view").unwrap();
                        let undo_action = [InlineKeyboardButton::callback(remove_from_view_action_text, format!("remove_from_view_{}", new_message.id))];

                        message_id = new_message.id;

                        let _msg = self.bot.edit_message_reply_markup(CHAT_ID, new_message.id).reply_markup(InlineKeyboardMarkup::new([undo_action])).await?;
                    } else {
                        tracing::warn!("Error editing message caption: {}", _e);
                    }
                }
            };
            posted_content.last_updated_at = now.to_rfc3339();
            tx.save_posted_content(posted_content.clone())?;

            Ok((message_id, video_info.clone()))
        }
    }

    async fn send_video_and_get_id(&mut self, input_file: InputFile) -> Result<MessageId, Box<dyn Error + Send + Sync>> {
        let span = tracing::span!(tracing::Level::INFO, "send_video_and_get_id");
        let _enter = span.enter();

        let video_message = self.bot.send_video(CHAT_ID, input_file).await?;
        Ok(video_message.id)
    }

    fn get_action_buttons(&mut self, action_keys: &[&str], sent_message_id: MessageId) -> Vec<InlineKeyboardButton> {
        let span = tracing::span!(tracing::Level::INFO, "get_action_buttons");
        let _enter = span.enter();

        action_keys
            .iter()
            .map(|&key| {
                let action_text = self.ui_definitions.buttons.get(key).unwrap();
                InlineKeyboardButton::callback(action_text, format!("{}_{}", key, sent_message_id))
            })
            .collect()
    }
    async fn edit_message_caption_and_markup(&mut self, chat_id: ChatId, message_id: MessageId, caption: String, markup_buttons: Vec<InlineKeyboardButton>) -> Result<(), Box<dyn Error + Send + Sync>> {
        let span = tracing::span!(tracing::Level::INFO, "edit_message_caption_and_markup");
        let _enter = span.enter();

        let caption_edit_result = self.bot.edit_message_caption(chat_id, message_id).caption(caption.clone()).await;
        handle_message_is_not_modified_error(caption_edit_result, caption).await?;

        let markup_edit_result = self.bot.edit_message_reply_markup(chat_id, message_id).reply_markup(InlineKeyboardMarkup::new([markup_buttons])).await;
        handle_message_is_not_modified_error(markup_edit_result, "markup".to_string()).await?;

        Ok(())
    }
}

impl BotManager {
    pub fn new(database: Database, credentials: HashMap<String, String>) -> Self {
        let span = tracing::span!(tracing::Level::INFO, "BotManager::new");
        let _enter = span.enter();
        let inner = InnerBotManager::new(database, credentials);
        let inner = Arc::new(Mutex::new(inner));
        Self { inner }
    }

    pub async fn run_bot(&self, rx: Receiver<(String, String, String, String)>) {
        let span = tracing::span!(tracing::Level::INFO, "BotManager::run_bot");
        let _enter = span.enter();
        let mut inner = self.inner.lock().await;
        inner.run_bot(rx).await;
    }
}
