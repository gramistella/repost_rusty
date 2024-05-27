use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use rand::prelude::{SliceRandom, StdRng};
use rand::SeedableRng;
use s3::Bucket;
use serde::{Deserialize, Serialize};
use serenity::all::{Builder, ChannelId, CreateInteractionResponse, CreateMessage, GetMessages, Interaction, MessageId, RatelimitInfo};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::database::database::{Database, DatabaseTransaction, UserSettings};
use crate::discord::interactions::{EditedContent, EditedContentKind};
use crate::discord::state::ContentStatus;
use crate::discord::utils::{clear_all_messages, prune_expired_content};
use crate::{crab, DISCORD_REFRESH_RATE, GUILD_ID, POSTED_CHANNEL_ID, STATUS_CHANNEL_ID};

#[derive(Clone)]
pub struct Handler {
    pub username: String,
    pub database: Database,
    pub credentials: HashMap<String, String>,
    pub bucket: Bucket,
    pub ui_definitions: UiDefinitions,
    pub edited_content: Arc<Mutex<Option<EditedContent>>>,
    pub interaction_mutex: Arc<Mutex<()>>,
    pub global_last_updated_at: Arc<Mutex<DateTime<Utc>>>,
    pub is_first_iteration: Arc<AtomicBool>,
    pub has_started: Arc<AtomicBool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct UiDefinitions {
    pub(crate) buttons: HashMap<String, String>,
    pub(crate) labels: HashMap<String, String>,
}

#[derive(Clone)]
pub struct DiscordBot {
    username: String,
    client: Arc<Mutex<Client>>,
}

pub(crate) struct ChannelIdMap {}

impl TypeMapKey for ChannelIdMap {
    type Value = ChannelId;
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        let channel_id = *ctx.data.read().await.get::<ChannelIdMap>().unwrap();

        if msg.channel_id == channel_id && !msg.author.bot {
            let edited_content = self.edited_content.lock().await;
            if edited_content.is_some() {
                let mut edited_content = edited_content.clone().unwrap();

                let mut received_edit = "".to_string();
                if msg.content != "!" {
                    received_edit.clone_from(&msg.content);
                }

                let mut tx = self.database.begin_transaction().await;
                let user_settings = tx.load_user_settings().await;

                match edited_content.kind {
                    EditedContentKind::Caption => {
                        edited_content.content_info.caption = received_edit;
                    }
                    EditedContentKind::Hashtags => {
                        edited_content.content_info.hashtags = received_edit;
                    }
                }

                tx.save_content_info(&edited_content.content_info).await;

                msg.delete(&ctx.http).await.unwrap();
                ctx.http.delete_message(channel_id, edited_content.message_to_delete.unwrap(), None).await.unwrap();

                self.process_pending(&ctx, &user_settings, &mut tx, &mut edited_content.content_info, Arc::clone(&self.global_last_updated_at)).await;
            }
        }
    }

    async fn ready(&self, ctx: Context, _ready: serenity::model::gateway::Ready) {

        if !self.has_started.swap(true, Ordering::SeqCst) {
            loop {
                let mut tx = self.database.begin_transaction().await;
                let user_settings = tx.load_user_settings().await;
                let mut rng = StdRng::from_entropy();

                let global_last_updated_at = Arc::clone(&self.global_last_updated_at);

                self.ready_loop(&ctx, &user_settings, &mut tx, global_last_updated_at, &mut rng).await;

                if self.is_first_iteration.swap(false, Ordering::SeqCst) {
                    let mut tx = self.database.begin_transaction().await;
                    println!(" [{}] Discord bot finished warming up.", self.username);
                    let mut bot_status = tx.load_bot_status().await;
                    bot_status.is_discord_warmed_up = true;
                    tx.save_bot_status(&bot_status).await;
                }

                sleep(DISCORD_REFRESH_RATE).await;
            }
        }
    }
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let response = CreateInteractionResponse::Acknowledge;

        match response.execute(&ctx.http, (interaction.id(), interaction.token())).await {
            Ok(_) => {}
            Err(e) => {
                let e = format!("{:?}", e);
                if e.contains("Unknown Interaction") {
                } else {
                    tracing::warn!("Failed to acknowledge interaction!");
                }
                return;
            }
        };

        let _is_handling_interaction = self.interaction_mutex.lock().await;

        let original_message_id = interaction.clone().message_component().unwrap().message.id;

        let mut tx = self.database.begin_transaction().await;

        let interaction_message = interaction.clone().message_component().unwrap();
        let interaction_type = interaction_message.clone().data.custom_id;

        let global_last_updated_at = Arc::clone(&self.global_last_updated_at);

        // Check if the original message id is in the content mapping
        let mut found_content = None;
        for content in tx.load_content_mapping().await {
            if content.message_id == original_message_id {
                found_content = Some(content);
            }
        }

        let mut user_settings = tx.load_user_settings().await;
        if found_content.is_none() {
            let mut bot_status = tx.load_bot_status().await;
            if bot_status.message_id == original_message_id {
                match interaction_type.as_str() {
                    "resume_from_halt" => {
                        self.interaction_resume_from_halt(&mut user_settings, &mut bot_status, &mut tx).await;
                    }
                    "enable_manual_mode" => {
                        self.interaction_enable_manual_mode(&user_settings, &mut bot_status, &mut tx).await;
                    }
                    "disable_manual_mode" => {
                        self.interaction_disable_manual_mode(&user_settings, &mut bot_status, &mut tx).await;
                    }
                    _ => {
                        tracing::error!("Unhandled interaction type: {:?}", interaction_type);
                    }
                }
            } else {
                tracing::error!("Content not found for message id: {}", original_message_id);
                return;
            }
        } else {
            let mut content = found_content.clone().unwrap();

            match interaction_type.as_str() {
                "publish_now" => {
                    self.interaction_publish_now(&user_settings, &mut content, &mut tx).await;
                }
                "accept" => {
                    self.interaction_accepted(&ctx, &user_settings, &mut content, &mut tx, global_last_updated_at).await;
                }
                "remove_from_queue" => {
                    self.interaction_remove_from_queue(&ctx, &user_settings, &mut content, &mut tx, global_last_updated_at).await;
                }
                "reject" => {
                    self.interaction_rejected(&ctx, &user_settings, &mut content, &mut tx, global_last_updated_at).await;
                }
                "undo_rejected" => {
                    self.interaction_undo_rejected(&ctx, &user_settings, &mut content, &mut tx, global_last_updated_at).await;
                }
                "remove_from_view" => {
                    self.interaction_remove_from_view(&ctx, &mut content).await;
                }
                "remove_from_view_failed" => {
                    self.interaction_remove_from_view_failed(&ctx, &mut content).await;
                }
                "edit" => {
                    self.interaction_edit(&user_settings, &mut tx, &ctx, &mut content).await;
                }
                "go_back" => {
                    self.interaction_go_back(&user_settings, &mut tx, &ctx, &mut content).await;
                }
                "edit_caption" => {
                    if self.edited_content.lock().await.is_none() {
                        self.interaction_edit_caption(&ctx, &interaction, &mut content).await;
                    }
                }
                "edit_hashtags" => {
                    if self.edited_content.lock().await.is_none() {
                        self.interaction_edit_hashtags(&ctx, &interaction, &mut content).await;
                    }
                }
                _ => {
                    tracing::error!("Unhandled interaction type: {:?}", interaction_type);
                }
            }
            tx.save_content_info(&content).await;
        }
    }

    async fn ratelimit(&self, data: RatelimitInfo) {
        // Disable rate limit logic for the first iteration
        if !self.is_first_iteration.load(Ordering::SeqCst) {
            tracing::warn!(" [{}] Rate limited: {:?}", self.username, data);
            //let timeout = data.timeout.as_millis();
            //let mut tx = self.database.begin_transaction().await;
            //let mut user_settings = tx.load_user_settings().await;
            //user_settings.interface_update_interval += timeout as i64;
            //tx.save_user_settings(&user_settings).await;
        }
    }
}

impl Handler {
    async fn ready_loop(&self, ctx: &Context, user_settings: &UserSettings, tx: &mut DatabaseTransaction, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>, rng: &mut StdRng) {
        if self.is_bot_busy().await {
            return;
        }

        self.process_bot_status(ctx, user_settings, tx, Arc::clone(&global_last_updated_at)).await;
        let content_mapping = if self.is_first_iteration.load(Ordering::SeqCst) {
            tx.load_content_mapping().await
        } else {
            let mut content_mapping = tx.load_content_mapping().await;
            content_mapping.shuffle(rng);
            content_mapping
        };

        if content_mapping.is_empty() {
            sleep(DISCORD_REFRESH_RATE).await;
        }

        for mut content in content_mapping {
            if prune_expired_content(user_settings, tx, &mut content).await {
                continue;
            }

            if self.is_bot_busy().await {
                break;
            }

            match content.status {
                ContentStatus::RemovedFromView => {
                    tx.remove_content_info_with_shortcode(&content.original_shortcode).await;
                    continue;
                }
                ContentStatus::Pending { .. } => self.process_pending(ctx, user_settings, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Queued { .. } => self.process_queued(ctx, user_settings, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Published { .. } => self.process_published(ctx, user_settings, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Rejected { .. } => self.process_rejected(ctx, user_settings, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Failed { .. } => self.process_failed(ctx, user_settings, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
            }

            tx.save_content_info(&content).await;
        }
    }

    async fn is_bot_busy(&self) -> bool {
        // Check if the bot is currently editing a message
        {
            let is_editing = self.edited_content.lock().await;
            if is_editing.is_some() {
                return true;
            }
        }

        // Check if the bot is currently handling an interaction
        {
            let is_handling_interaction = self.interaction_mutex.try_lock();
            match is_handling_interaction {
                Ok(_) => {}
                Err(_) => {
                    // Not actually an error, just means the bot is currently handling an interaction
                    return true;
                }
            }
        }

        false
    }
}

impl DiscordBot {
    pub async fn new(database: Database, bucket: Bucket, credentials: HashMap<String, String>, is_first_run: bool) -> Self {
        let ui_definitions_yaml_data = include_str!("../../config/ui_definitions.yaml");
        let ui_definitions: UiDefinitions = serde_yaml::from_str(ui_definitions_yaml_data).expect("Error parsing config file");

        // Login with a bot token from the environment
        let username = credentials.get("username").expect("No username found in credentials");
        let token = credentials.get("discord_token").expect("No discord token found in credentials");

        // Set gateway intents, which decides what events the bot will be notified about
        let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        // let interaction_shard = Shard::new();
        // Create a new instance of the Client, logging in as a bot.
        let client = Client::builder(token, intents)
            .event_handler(Handler {
                username: username.clone(),
                credentials: credentials.clone(),
                database: database.clone(),
                bucket,
                ui_definitions: ui_definitions.clone(),
                edited_content: Arc::new(Mutex::new(None)),
                interaction_mutex: Arc::new(Mutex::new(())),
                global_last_updated_at: Arc::new(Mutex::new(Utc::now())),
                is_first_iteration: Arc::new(AtomicBool::new(true)),
                has_started: Arc::new(AtomicBool::new(false)),
            })
            .await
            .expect("Err creating client");

        let mut user_channel = None;

        let guild = client.http.get_guild(GUILD_ID).await.unwrap();

        let guild_channels = guild.channels(&client.http).await.unwrap();
        for (_channel_id, channel) in guild_channels {
            if channel.name == *username {
                user_channel = Some(channel);
            }
        }

        let channel_id = match user_channel {
            Some(channel) => channel.id,
            None => {
                let mut map = HashMap::new();
                map.insert("name".to_string(), username.clone());
                let channel = client.http.create_channel(guild.id, &map, None).await.unwrap();
                channel.id
            }
        };

        let mut tx = database.begin_transaction().await;

        clear_all_messages(&mut tx, &client.http, channel_id, true).await;
        
        let welcome_message = format!("Welcome back! {}", crab!("!,!"));
        
        if is_first_run {
            // Set up the posted channel
            let messages = POSTED_CHANNEL_ID.messages(&client.http, GetMessages::new()).await.unwrap();

            let mut is_message_there = false;
            for (i, message) in messages.iter().enumerate() {
                if i == 0 && message.author.bot && message.content.contains(&welcome_message) {
                    is_message_there = true;
                    continue;
                } else {
                    message.delete(&client.http).await.unwrap();
                }
            }

            if !is_message_there {
                let msg = CreateMessage::new().content(&welcome_message);
                let _ = client.http.send_message(POSTED_CHANNEL_ID, vec![], &msg).await;
            }

            // Set up the status channel
            tx.clear_all_other_bot_statuses().await;

            let messages = STATUS_CHANNEL_ID.messages(&client.http, GetMessages::new()).await.unwrap();

            let mut tx = database.begin_transaction().await;
            let mut bot_status = tx.load_bot_status().await;
            let mut is_message_there = false;
            for message in messages {
                if message.author.name == *username && message.author.bot && message.content.contains("Last updated at") {
                    if bot_status.message_id == message.id {
                        is_message_there = true;
                    } else {
                        is_message_there = true;
                        bot_status.message_id = message.id;
                    }
                } else {
                    message.delete(&client.http).await.unwrap();
                }
            }

            // If the message not is there, and the message id is not 1, then reset the message id
            // so that it will be sent by view.rs
            if !is_message_there && bot_status.message_id.get() != 1 {
                bot_status.message_id = MessageId::new(1);
            }

            // Reset the message ids for the alerts to function properly when restarting the bot
            bot_status.halt_alert_message_id = MessageId::new(1);
            bot_status.queue_alert_1_message_id = MessageId::new(1);

            tx.save_bot_status(&bot_status).await;
        }

        let msg = CreateMessage::new().content(welcome_message);
        let _ = client.http.send_message(channel_id, vec![], &msg).await;

        {
            let mut data = client.data.write().await;
            match data.get_mut::<ChannelIdMap>() {
                Some(map) => {
                    *map = channel_id;
                }
                None => {
                    data.insert::<ChannelIdMap>(channel_id);
                }
            }
        }

        let client = Arc::new(Mutex::new(client));
        DiscordBot { username: username.to_string(), client }
    }

    pub async fn start_listener(&mut self) {
        let client = Arc::clone(&self.client);
        let mut client_guard = client.lock().await;
        client_guard.start().await.expect("Error starting client");
    }

    pub async fn run(&mut self) {
        println!("Running discord bot for {}", self.username);
        self.start_listener().await;
    }
}
