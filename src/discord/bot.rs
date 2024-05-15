use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serenity::all::{Builder, ChannelId, CreateInteractionResponse, CreateMessage, GetMessages, Interaction, MessageId, RatelimitInfo};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;
use tokio::time::sleep;


use crate::discord::commands::{edit_caption, Data};
use crate::discord::interactions::{EditedContent, EditedContentKind};
use crate::discord::state::ContentStatus;
use crate::discord::utils::{clear_all_messages, prune_expired_content};
use crate::{GUILD_ID, POSTED_CHANNEL_ID, STATUS_CHANNEL_ID};
use crate::database::database::{Database, DatabaseTransaction};

pub(crate) const DISCORD_REFRESH_RATE: Duration = Duration::from_millis(333);

#[derive(Clone)]
pub struct Handler {
    pub username: String,
    pub database: Database,
    pub credentials: HashMap<String, String>,
    pub ui_definitions: UiDefinitions,
    pub edited_content: Arc<Mutex<Option<EditedContent>>>,
    pub interaction_mutex: Arc<Mutex<()>>,
    pub global_last_updated_at: Arc<Mutex<DateTime<Utc>>>,
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

lazy_static! {
    static ref IS_FIRST_ITERATION: Arc<Mutex<bool>> = Arc::new(Mutex::new(true));
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
                    received_edit = msg.content.clone();
                }

                let mut tx = self.database.begin_transaction().await.unwrap();
                match edited_content.kind {
                    EditedContentKind::Caption => {
                        edited_content.content_info.caption = received_edit;
                    }
                    EditedContentKind::Hashtags => {
                        edited_content.content_info.hashtags = received_edit;
                    }
                }

                tx.save_content_info(&edited_content.content_info.clone()).unwrap();
                msg.delete(&ctx.http).await.unwrap();
                ctx.http.delete_message(channel_id, edited_content.message_to_delete.unwrap(), None).await.unwrap();
                
                self.process_pending(&ctx, &mut tx, &mut edited_content.content_info, Arc::clone(&self.global_last_updated_at)).await;
            }
        }
    }

    async fn ready(&self, ctx: Context, _ready: serenity::model::gateway::Ready) {
        loop {
            let mut tx = self.database.begin_transaction().await.unwrap();

            let self_clone = self.clone();
            let ctx_clone = ctx.clone();
            let global_last_updated_at = Arc::clone(&self.global_last_updated_at);

            let task = tokio::spawn(async move {
                self_clone.ready_loop(&ctx_clone, &mut tx, global_last_updated_at).await;
            });

            task.await.unwrap();

            let mut is_first_iteration = IS_FIRST_ITERATION.lock().await;
            if is_first_iteration.clone() {
                let mut tx = self.database.begin_transaction().await.unwrap();
                println!(" [{}] Discord bot finished warming up.", self.username);
                let mut bot_status = tx.load_bot_status().unwrap();
                bot_status.is_discord_warmed_up = true;
                tx.save_bot_status(&bot_status).unwrap();
                *is_first_iteration = false;
            }

            sleep(DISCORD_REFRESH_RATE).await;
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

        let mut tx = self.database.begin_transaction().await.unwrap();

        let interaction_message = interaction.clone().message_component().unwrap();
        let interaction_type = interaction_message.clone().data.custom_id;

        // Check if the original message id is in the content mapping
        let mut found_content = None;
        for content in tx.load_content_mapping().unwrap() {
            if content.message_id == original_message_id {
                found_content = Some(content);
            }
        }

        if found_content.is_none() {
            let mut bot_status = tx.load_bot_status().unwrap();
            if bot_status.message_id == original_message_id {
                match interaction_type.as_str() {
                    "resume_from_halt" => {
                        self.interaction_resume_from_halt(&mut bot_status, &mut tx).await;
                    }
                    "enable_manual_mode" => {
                        self.interaction_enable_manual_mode(&mut bot_status, &mut tx).await;
                    }
                    "disable_manual_mode" => {
                        self.interaction_disable_manual_mode(&mut bot_status, &mut tx).await;
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
                    self.interaction_publish_now(&mut content, &mut tx).await;
                }
                "accept" => {
                    self.interaction_accepted(&mut content, &mut tx).await;
                }
                "remove_from_queue" => {
                    self.interaction_remove_from_queue(&mut content, &mut tx).await;
                }
                "reject" => {
                    self.interaction_rejected(&mut content, &mut tx).await;
                }
                "undo_rejected" => {
                    self.interaction_undo_rejected(&mut content, &mut tx).await;
                }
                "remove_from_view" => {
                    self.interaction_remove_from_view(&ctx, &mut content).await;
                }
                "remove_from_view_failed" => {
                    self.interaction_remove_from_view_failed(&ctx, &mut content).await;
                }
                "edit" => {
                    self.interaction_edit(&ctx, &mut content).await;
                }
                "go_back" => {
                    self.interaction_go_back(&ctx, &mut content).await;
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
            tx.save_content_info(&content).unwrap();
        }
    }

    async fn ratelimit(&self, data: RatelimitInfo) {
        tracing::warn!(" [{}] Rate limited: {:?}", self.username, data);
    }
}

impl Handler {
    async fn ready_loop(&self, ctx: &Context, tx: &mut DatabaseTransaction, global_last_updated_at: Arc<Mutex<DateTime<Utc>>>) {
        // Check if the bot is currently editing a message
        {
            let is_editing = self.edited_content.lock().await;
            if is_editing.is_some() {
                return;
            }
        }

        self.process_bot_status(ctx, tx).await;
        let content_mapping = tx.load_content_mapping().unwrap();

        if content_mapping.is_empty() {
            sleep(DISCORD_REFRESH_RATE).await;
        }

        for mut content in content_mapping {
            if prune_expired_content(tx, &mut content) {
                continue;
            }

            // Check if the bot is currently handling an interaction
            let is_handling_interaction = self.interaction_mutex.try_lock();
            match is_handling_interaction {
                Ok(_) => {}
                Err(_e) => {
                    // Not actually an error, just means the bot is currently handling an interaction
                    // tracing::error!("Error locking interaction mutex: {:?}", _e);
                    break;
                }
            }

            match content.status {
                ContentStatus::Waiting => {}
                ContentStatus::RemovedFromView => tx.remove_content_info_with_shortcode(&content.original_shortcode).unwrap(),
                ContentStatus::Pending { .. } => self.process_pending(ctx, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Queued { .. } => self.process_queued(ctx, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Published { .. } => self.process_published(ctx, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Rejected { .. } => self.process_rejected(ctx, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
                ContentStatus::Failed { .. } => self.process_failed(ctx, tx, &mut content, Arc::clone(&global_last_updated_at)).await,
            }

            tx.save_content_info(&content).unwrap();
        }
    }
}

impl DiscordBot {
    pub async fn new(database: Database, credentials: HashMap<String, String>, is_first_run: bool) -> Self {
        let ui_definitions_yaml_data = include_str!("../../config/ui_definitions.yaml");
        let ui_definitions: UiDefinitions = serde_yaml::from_str(ui_definitions_yaml_data).expect("Error parsing config file");

        // Login with a bot token from the environment
        let username = credentials.get("username").expect("No username found in credentials");
        let token = credentials.get("discord_token").expect("No discord token found in credentials");

        // Set gateway intents, which decides what events the bot will be notified about
        let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        let framework = poise::Framework::builder()
            .options(poise::FrameworkOptions { commands: vec![edit_caption()], ..Default::default() })
            .setup(|ctx, _ready, framework| {
                Box::pin(async move {
                    poise::builtins::register_in_guild(ctx, &framework.options().commands, GUILD_ID).await?;
                    Ok(Data {})
                })
            })
            .build();

        // let interaction_shard = Shard::new();
        // Create a new instance of the Client, logging in as a bot.
        let client = Client::builder(token, intents)
            .event_handler(Handler {
                username: username.clone(),
                credentials: credentials.clone(),
                database: database.clone(),
                ui_definitions: ui_definitions.clone(),
                edited_content: Arc::new(Mutex::new(None)),
                interaction_mutex: Arc::new(Mutex::new(())),
                global_last_updated_at: Arc::new(Mutex::new(Utc::now())),
            })
            .framework(framework)
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

        clear_all_messages(&database, &client.http, channel_id, true).await;

        if is_first_run {
            // Set up the posted channel
            let messages = POSTED_CHANNEL_ID.messages(&client.http, GetMessages::new()).await.unwrap();

            let mut is_message_there = false;
            for (i, message) in messages.iter().enumerate() {
                if i == 0 && message.author.bot && message.content.contains("Welcome back! 🦀") {
                    is_message_there = true;
                    continue;
                } else {
                    message.delete(&client.http).await.unwrap();
                }
            }

            if !is_message_there {
                let msg = CreateMessage::new().content("Welcome back! 🦀");
                let _ = client.http.send_message(POSTED_CHANNEL_ID, vec![], &msg).await;
            }

            // Set up the status channel
            let mut tx = database.begin_transaction().await.unwrap();
            tx.clear_all_other_bot_statuses().await.unwrap();

            let messages = STATUS_CHANNEL_ID.messages(&client.http, GetMessages::new()).await.unwrap();

            let mut tx = database.begin_transaction().await.unwrap();
            let mut bot_status = tx.load_bot_status().unwrap();
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
            bot_status.queue_alert_message_id = MessageId::new(1);

            tx.save_bot_status(&bot_status).unwrap();
        }

        let msg = CreateMessage::new().content("Welcome back! 🦀");
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

    pub async fn run_bot(&mut self) {
        println!("Running discord bot for {}", self.username);
        self.start_listener().await;
    }
}
