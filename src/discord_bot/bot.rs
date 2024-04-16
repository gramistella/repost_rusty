use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serenity::all::{Builder, ChannelId, CreateCommand, CreateInteractionResponse, CreateMessage, GetMessages, GuildId, Interaction, RatelimitInfo};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;
use tokio::time::sleep;

use crate::discord_bot::commands::{edit_caption, Data};
use crate::discord_bot::database::{Database, DatabaseTransaction};
use crate::discord_bot::interactions::{EditedContentKind, InnerEventHandler};
use crate::discord_bot::state::ContentStatus;
use crate::discord_bot::utils::clear_all_messages;

pub(crate) const REFRESH_RATE: Duration = Duration::from_millis(500);

pub(crate) const INTERFACE_UPDATE_INTERVAL: Duration = Duration::from_secs(90);

pub(crate) const GUILD_ID: GuildId = GuildId::new(1090413253592612917);
pub(crate) const POSTED_CHANNEL_ID: ChannelId = ChannelId::new(1228041627898216469);

#[derive(Clone)]
struct Handler {
    database: Database,
    inner_event_handler: Arc<Mutex<InnerEventHandler>>,
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

pub(crate) struct ChannelIdMap {
    channel_id: ChannelId,
}

impl TypeMapKey for ChannelIdMap {
    type Value = ChannelId;
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        let channel_id = ctx.data.read().await.get::<ChannelIdMap>().unwrap().clone();

        if msg.channel_id == channel_id && msg.author.bot == false {
            let inner_event_handler = self.inner_event_handler.lock().await;
            let edited_content = inner_event_handler.edited_content.lock().await;
            if edited_content.is_some() {
                let mut edited_content = edited_content.clone().unwrap();

                let received_edit;
                if msg.content == "!" {
                    received_edit = "".to_string();
                } else {
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

                let content_mapping = IndexMap::from([(edited_content.content_id, edited_content.content_info.clone())]);
                tx.save_content_mapping(content_mapping).unwrap();
                msg.delete(&ctx.http).await.unwrap();
                ctx.http.delete_message(channel_id, edited_content.message_to_delete.unwrap(), None).await.unwrap();
                inner_event_handler.clone().process_pending(&ctx, &mut tx, &mut edited_content.content_id, &mut edited_content.content_info).await;
            }
            //println!("Message received: {}", msg.content);
            /*
            if msg.content == "!ping" {
                if let Err(why) = msg.channel_id.say(&ctx.http, "Pong!").await {
                    println!("Error sending message: {why:?}");
                }
            }
            */
        }
    }

    async fn ratelimit(&self, data: RatelimitInfo) {
        println!("Ratelimited: {:?}", data);
    }
    async fn ready(&self, ctx: Context, _ready: serenity::model::gateway::Ready) {
        let mut tx = self.database.begin_transaction().await.unwrap();
        let ctx = Arc::new(Mutex::new(ctx));
        let self_clone = Arc::new(Mutex::new(self.clone()));
        // Assuming `self` is an instance of `DiscordBot`

        let task = tokio::spawn(async move {
            loop {
                {
                    let ctx = ctx.lock().await;
                    let self_clone = self_clone.lock().await;
                    self_clone.ready_loop(&ctx, &mut tx).await;
                }
                sleep(REFRESH_RATE).await;
            }
        });

        task.await.unwrap();
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        //let _guard = self.inner_event_handler.execution_mutex.lock().await;
        //println!("Interaction received: {:?}", interaction);

        let response = CreateInteractionResponse::Acknowledge;
        response.execute(&ctx.http, (interaction.id(), interaction.token())).await.unwrap();

        let inner_event_handler = self.inner_event_handler.lock().await;
        let _is_handling_interaction = inner_event_handler.interaction_mutex.lock().await;

        let mut original_message_id = interaction.clone().message_component().unwrap().message.id;

        let mut tx = self.database.begin_transaction().await.unwrap();

        // Check if the original message id is in the content mapping
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

            let mut inner_event_handler = inner_event_handler.clone();

            match interaction_type.as_str() {
                "publish_now" => {
                    inner_event_handler.interaction_publish_now(&mut content).await;
                }
                "accept" => {
                    inner_event_handler.interaction_accepted(&mut content).await;
                }
                "remove_from_queue" => {
                    inner_event_handler.interaction_remove_from_queue(&mut content).await;
                }
                "reject" => {
                    inner_event_handler.interaction_rejected(&mut content).await;
                }
                "undo_rejected" => {
                    inner_event_handler.interaction_undo_rejected(&mut content).await;
                }
                "remove_from_view" => {
                    inner_event_handler.interaction_remove_from_view(&ctx, original_message_id, &mut content).await;
                }
                "remove_from_view_failed" => {
                    inner_event_handler.interaction_remove_from_view_failed(&ctx, original_message_id, &mut content).await;
                }
                "edit" => {
                    inner_event_handler.interaction_edit(&ctx, &mut original_message_id, &mut content).await;
                }
                "go_back" => {
                    inner_event_handler.interaction_go_back(&ctx, original_message_id, &mut content).await;
                }
                "edit_caption" => {
                    if inner_event_handler.edited_content.lock().await.is_none() {
                        inner_event_handler.interaction_edit_caption(&ctx, &interaction, &mut original_message_id, &mut content).await;
                    }
                }
                "edit_hashtags" => {
                    if inner_event_handler.edited_content.lock().await.is_none() {
                        inner_event_handler.interaction_edit_hashtags(&ctx, &interaction, &mut original_message_id, &mut content).await;
                    }
                }
                _ => {
                    tracing::error!("Unhandled interaction type: {:?}", interaction_type);
                }
            }
            tx.save_content_mapping(IndexMap::from([(original_message_id, content)])).unwrap();
        }
    }
}

impl Handler {
    async fn ready_loop(&self, ctx: &Context, tx: &mut DatabaseTransaction) {
        // Check if the bot is currently editing a message
        {
            let inner_event_handler = self.inner_event_handler.lock().await;
            let is_editing = inner_event_handler.edited_content.lock().await;

            if is_editing.is_some() {
                return;
            }
        }

        let content_mapping = tx.load_page().unwrap();

        for (mut content_id, mut content) in content_mapping {
            // Check if the bot is currently handling an interaction
            let inner_event_handler = self.inner_event_handler.lock().await;
            let mut tx = self.database.begin_transaction().await.unwrap();
            let is_handling_interaction = inner_event_handler.interaction_mutex.try_lock();
            match is_handling_interaction {
                Ok(_) => {}
                Err(_e) => {
                    // Not actually an error, just means the bot is currently handling an interaction
                    // tracing::error!("Error locking interaction mutex: {:?}", _e);
                    break;
                }
            }

            //println!("Processing content: {}", content_id);
            match content.status {
                ContentStatus::Waiting => {}
                ContentStatus::RemovedFromView => tx.remove_content_info_with_shortcode(content.original_shortcode).unwrap(),
                ContentStatus::Pending { .. } => inner_event_handler.process_pending(&ctx, &mut tx, &mut content_id, &mut content).await,
                ContentStatus::Queued { .. } => inner_event_handler.process_queued(&ctx, &mut tx, &mut content_id, &mut content).await,
                ContentStatus::Published { .. } => inner_event_handler.process_published(&ctx, &mut tx, &mut content_id, &mut content).await,
                ContentStatus::Rejected { .. } => inner_event_handler.process_rejected(&ctx, &mut tx, &mut content_id, &mut content).await,
                ContentStatus::Failed { .. } => inner_event_handler.process_failed(&ctx, &mut tx, &mut content_id, &mut content).await,
            }
        }
    }
}

impl DiscordBot {
    pub async fn new(database: Database, credentials: HashMap<String, String>, setup_posted_channel: bool) -> Self {
        let ui_definitions_yaml_data = include_str!("../../config/ui_definitions.yaml");
        let ui_definitions: UiDefinitions = serde_yaml::from_str(&ui_definitions_yaml_data).expect("Error parsing config file");

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
        let client = Client::builder(&token, intents)
            .event_handler(Handler {
                database: database.clone(),
                inner_event_handler: Arc::new(Mutex::new(InnerEventHandler {
                    database: database.clone(),
                    ui_definitions: ui_definitions.clone(),
                    edited_content: Arc::new(Mutex::new(None)),
                    interaction_mutex: Arc::new(Mutex::new(())),
                })),
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

        if setup_posted_channel {
            let messages = POSTED_CHANNEL_ID.messages(&client.http, GetMessages::new()).await.unwrap();

            for (i, message) in messages.iter().enumerate() {
                if i == 0 && message.author.bot && message.content.contains("Welcome back! 🦀") {
                    continue;
                } else {
                    message.delete(&client.http).await.unwrap();
                }
            }

            let msg = CreateMessage::new().content("Welcome back! 🦀");
            let _ = client.http.send_message(POSTED_CHANNEL_ID, vec![], &msg).await;
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

        let command = CreateCommand::new("/edit_caption").description("Edit the caption of a post");

        let guild_id = GuildId::new(1090413253592612917);

        self.client.lock().await.http.create_guild_command(guild_id, &command).await.unwrap();
    }
}
