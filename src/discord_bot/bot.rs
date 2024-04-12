use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serenity::all::{ChannelId, CreateCommand, CreateMessage, GuildId, Interaction};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;
use tokio::time::sleep;

use crate::discord_bot::commands::{edit_caption, Data};
use crate::discord_bot::database::Database;
use crate::discord_bot::interactions::{EditedContentKind, InnerEventHandler};
use crate::discord_bot::state::ContentStatus;
use crate::discord_bot::utils::clear_all_messages;

pub(crate) const REFRESH_RATE: Duration = Duration::from_secs(5);

pub(crate) const INTERFACE_UPDATE_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) const GUILD_ID: GuildId = GuildId::new(1090413253592612917);
pub(crate) const POSTED_CHANNEL_ID: ChannelId = ChannelId::new(1228041627898216469);

struct Handler {
    database: Database,
    inner_event_handler: InnerEventHandler,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct UiDefinitions {
    pub(crate) buttons: HashMap<String, String>,
    pub(crate) labels: HashMap<String, String>,
}

#[derive(Clone)]
pub struct DiscordBot {
    username: String,
    database: Database,
    ui_definitions: UiDefinitions,
    client: Arc<Mutex<Client>>,
    channel_id: ChannelId,
}

pub(crate) struct ChannelIdMap {
    channel_id: ChannelId,
}

pub(crate) struct IsEditingMap {
    is_editing: bool,
}
impl TypeMapKey for ChannelIdMap {
    type Value = ChannelId;
}

impl TypeMapKey for IsEditingMap {
    type Value = bool;
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        if msg.channel_id == channel_id && msg.author.bot == false {
            let edited_content = self.inner_event_handler.edited_content.lock().await;
            if edited_content.is_some() {
                let mut edited_content = edited_content.clone().unwrap();

                let mut tx = self.database.begin_transaction().await.unwrap();
                match edited_content.kind {
                    EditedContentKind::Caption => {
                        edited_content.content_info.caption = msg.content.clone();
                    }
                    EditedContentKind::Hashtags => {
                        edited_content.content_info.hashtags = msg.content.clone();
                    }
                }

                let content_mapping = IndexMap::from([(edited_content.content_id, edited_content.content_info.clone())]);
                tx.save_content_mapping(content_mapping).unwrap();
                msg.delete(&ctx.http).await.unwrap();
                ctx.http.delete_message(channel_id, edited_content.message_to_delete.unwrap(), None).await.unwrap();
                self.inner_event_handler.clone().process_pending(&ctx, &mut edited_content.content_id, &mut edited_content.content_info).await;
            }
            println!("Message received: {}", msg.content);
            if msg.content == "!ping" {
                if let Err(why) = msg.channel_id.say(&ctx.http, "Pong!").await {
                    println!("Error sending message: {why:?}");
                }
            }
        }
    }
    async fn ready(&self, ctx: Context, _ready: serenity::model::gateway::Ready) {
        let mut tx = self.database.begin_transaction().await.unwrap();

        loop {
            // Check if the bot is currently editing a message
            {
                let is_editing = self.inner_event_handler.edited_content.lock().await;

                if is_editing.is_some() {
                    sleep(REFRESH_RATE).await;
                    continue;
                }
            }

            let content_mapping = tx.load_content_mapping().unwrap();

            for (mut content_id, mut content) in content_mapping {
                // Check if the bot is currently handling an interaction
                let is_handling_interaction = self.inner_event_handler.interaction_mutex.try_lock();
                match is_handling_interaction {
                    Ok(_) => {}
                    Err(_e) => {
                        // Not actually an error, just means the bot is currently handling an interaction
                        // tracing::error!("Error locking interaction mutex: {:?}", _e);
                        sleep(REFRESH_RATE).await;
                        break;
                    }
                }

                //println!("Processing content: {}", content_id);
                match content.status {
                    ContentStatus::Waiting => {}
                    ContentStatus::RemovedFromView => {}
                    ContentStatus::Pending { .. } => self.inner_event_handler.clone().process_pending(&ctx, &mut content_id, &mut content).await,
                    ContentStatus::Queued { .. } => self.inner_event_handler.clone().process_queued(&ctx, &mut content_id, &mut content).await,
                    ContentStatus::Posted { .. } => self.inner_event_handler.clone().process_posted(&ctx, &mut content_id, &mut content).await,
                    ContentStatus::Rejected { .. } => self.inner_event_handler.clone().process_rejected(&ctx, &mut content_id, &mut content).await,
                    ContentStatus::Failed { .. } => self.inner_event_handler.clone().process_failed(&ctx, &mut content_id, &mut content).await,
                }
            }
            sleep(REFRESH_RATE).await;
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        //let _guard = self.inner_event_handler.execution_mutex.lock().await;
        //println!("Interaction received: {:?}", interaction);

        let _is_handling_interaction = self.inner_event_handler.interaction_mutex.lock().await;

        let mut original_message_id = interaction.clone().message_component().unwrap().message.id;
        //let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();
        let mut tx = self.database.begin_transaction().await.unwrap();
        let mut found_content = None;
        for (id, content) in tx.load_content_mapping().unwrap() {
            if id == original_message_id {
                found_content = Some(content);
            }
        }

        if found_content.is_none() {
            tracing::error!("Content not found for message id: {}", original_message_id);
            return;
        } else {
            let mut content = found_content.clone().unwrap();

            let interaction_message = interaction.clone().message_component().unwrap();
            let interaction_type = interaction_message.clone().data.custom_id;

            let mut inner_event_handler = self.inner_event_handler.clone();
            println!("Interaction received, {:?}", interaction_type);
            match interaction_type.as_str() {
                "post_now" => {
                    inner_event_handler.interaction_post_now(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "accept" => {
                    inner_event_handler.interaction_accepted(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "remove_from_queue" => {
                    inner_event_handler.interaction_remove_from_queue(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "reject" => {
                    inner_event_handler.interaction_rejected(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "undo_rejected" => {
                    inner_event_handler.interaction_undo_rejected(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "remove_from_view" => {
                    inner_event_handler.interaction_remove_from_view(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "remove_from_view_failed" => {
                    inner_event_handler.interaction_remove_from_view_failed(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "edit" => {
                    inner_event_handler.interaction_edit(ctx.clone(), interaction, &mut original_message_id, &mut content).await;
                }
                "go_back" => {
                    inner_event_handler.interaction_go_back(ctx.clone(), interaction, original_message_id, &mut content).await;
                }
                "edit_caption" => {
                    if inner_event_handler.edited_content.lock().await.is_none() {
                        inner_event_handler.interaction_edit_caption(ctx.clone(), interaction, &mut original_message_id, &mut content).await;
                    }
                }
                "edit_hashtags" => {
                    if inner_event_handler.edited_content.lock().await.is_none() {
                        inner_event_handler.interaction_edit_hashtags(ctx.clone(), interaction, &mut original_message_id, &mut content).await;
                    }
                }
                _ => {}
            }

            tx.save_content_mapping(IndexMap::from([(original_message_id, content)])).unwrap();
        }
    }
}

impl DiscordBot {
    pub async fn new(database: Database, credentials: HashMap<String, String>) -> Self {
        let ui_definitions_yaml_data = include_str!("../../config/ui_definitions.yaml");
        let ui_definitions: UiDefinitions = serde_yaml::from_str(&ui_definitions_yaml_data).expect("Error parsing config file");

        println!("Creating discord bot");

        // Login with a bot token from the environment
        let username = credentials.get("username").expect("No username found in credentials");
        let token = credentials.get("discord_token").expect("No discord token found in credentials");

        // Set gateway intents, which decides what events the bot will be notified about
        let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT | GatewayIntents::non_privileged();

        let framework = poise::Framework::builder()
            .options(poise::FrameworkOptions { commands: vec![edit_caption()], ..Default::default() })
            .setup(|ctx, _ready, framework| {
                Box::pin(async move {
                    poise::builtins::register_in_guild(ctx, &framework.options().commands, GUILD_ID).await?;
                    Ok(Data {})
                })
            })
            .build();

        // Create a new instance of the Client, logging in as a bot.
        let mut client = Client::builder(&token, intents)
            .event_handler(Handler {
                database: database.clone(),
                inner_event_handler: InnerEventHandler {
                    database: database.clone(),
                    ui_definitions: ui_definitions.clone(),
                    edited_content: Arc::new(Mutex::new(None)),
                    interaction_mutex: Arc::new(Mutex::new(())),
                },
            })
            .framework(framework)
            .await
            .expect("Err creating client");

        let mut user_channel = None;

        let guild = client.http.get_guild(GUILD_ID).await.unwrap();

        let guild_channels = guild.channels(&client.http).await.unwrap();
        for (_channel_id, mut channel) in guild_channels {
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

        DiscordBot {
            username: username.to_string(),
            database,
            ui_definitions,
            client,
            channel_id,
        }
    }

    pub async fn start_listener(&mut self) {
        let client = Arc::clone(&self.client);
        let mut client_guard = client.lock().await;

        client_guard.start().await.expect("Error starting client");
    }

    pub async fn run_bot(&mut self) {
        println!("Running discord bot for {}", self.username);

        self.start_listener().await;

        let command = CreateCommand::new("/edit_caption").description("Edit the caption of a post");

        let guild_id = GuildId::new(1090413253592612917);

        self.client.lock().await.http.create_guild_command(guild_id, &command).await.unwrap();
    }
}
