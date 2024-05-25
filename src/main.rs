use ::s3::creds::Credentials;
use ::s3::{Bucket, Region};
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serenity::all::{ChannelId, GuildId, UserId};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{layer::SubscriberExt, Layer, Registry};

use crate::database::database::Database;
use crate::discord::bot::DiscordBot;
use crate::scraper_poster::scraper::ContentManager;

mod discord;
mod s3;
mod scraper_poster;
mod video;

mod database;

// Constants that can be changed
pub(crate) const MY_DISCORD_ID: UserId = UserId::new(465494062275756032);
pub(crate) const GUILD_ID: GuildId = GuildId::new(1090413253592612917);
pub(crate) const POSTED_CHANNEL_ID: ChannelId = ChannelId::new(1236328603696762891);
pub(crate) const STATUS_CHANNEL_ID: ChannelId = ChannelId::new(1233547564880498688);

// Internal configuration, don't change the constants below
const IS_OFFLINE: bool = false;

// Internal scraper configuration
pub(crate) const SCRAPER_REFRESH_RATE: Duration = Duration::from_millis(3000);
const MAX_CONTENT_PER_ITERATION: usize = 8;
pub(crate) const MAX_CONTENT_HANDLED: usize = 40;
const FETCH_SLEEP_LEN: Duration = Duration::from_secs(60);
const SCRAPER_DOWNLOAD_SLEEP_LEN: Duration = Duration::from_secs(60 * 20);
const SCRAPER_LOOP_SLEEP_LEN: Duration = Duration::from_secs(60 * 60 * 12);

// Internal S3 configuration
pub const S3_EXPIRATION_TIME: u32 = 60 * 60 * 24 * 7;

// Internal Discord configuration
pub const DELAY_BETWEEN_MESSAGE_UPDATES: chrono::Duration = chrono::Duration::milliseconds(333);
pub(crate) const DISCORD_REFRESH_RATE: Duration = Duration::from_millis(333);
pub(crate) const INITIAL_INTERFACE_UPDATE_INTERVAL: Duration = Duration::from_millis(60_000);

// (V){!,!}(V)

fn main() -> anyhow::Result<()> {
    env::set_var("RUST_BACKTRACE", "full");

    let (_file_guard, _stdout_guard) = init_logging();

    let all_credentials = read_credentials("config/credentials.yaml");
    let mut all_handles = Vec::new();

    let mut is_first_run = true;
    for (username, credentials) in all_credentials {
        if credentials.get("enabled").expect("No enabled field in credentials") == "true" {
            let span = tracing::span!(tracing::Level::INFO, "main", username = username.as_str());
            let _enter = span.enter();
            tracing::info!("Starting bot for user: {}", username);

            let rt = Arc::new(tokio::runtime::Runtime::new().unwrap());
            let rt_clone = Arc::clone(&rt);

            let db = rt.block_on(async { Database::new(username.clone(), credentials.clone()).await.unwrap() });
            let bucket = init_bucket(credentials.clone());

            let mut discord_bot_manager = rt.block_on(async { DiscordBot::new(db.clone(), bucket.clone(), credentials.clone(), is_first_run).await });

            // Run the content_manager and the bot concurrently
            let mut content_manager = ContentManager::new(db, bucket, username, credentials, IS_OFFLINE);
            let scraper = std::thread::spawn(move || rt.block_on(content_manager.run()));

            let discord = std::thread::spawn(move || rt_clone.block_on(async { discord_bot_manager.run().await }));

            all_handles.push(scraper);
            all_handles.push(discord);

            is_first_run = false;
        }
    }

    // Wait for all tasks to complete
    for handle in all_handles {
        handle.join().expect("Thread panicked");
    }

    Ok(())
}

fn init_logging() -> (tracing_appender::non_blocking::WorkerGuard, tracing_appender::non_blocking::WorkerGuard) {
    //let multi = MultiProgress::new();
    let file_appender = tracing_appender::rolling::hourly("logs/", "rolling.log");
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::Layer::new()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE)
        .with_writer(non_blocking)
        .with_filter(LevelFilter::WARN);

    let (non_blocking, stdout_guard) = tracing_appender::non_blocking(std::io::stdout());
    let layer2 = tracing_subscriber::fmt::Layer::new()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(false)
        .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE)
        .with_writer(non_blocking)
        .with_filter(LevelFilter::WARN);

    //let logger = Registry::default().with(file_layer).with(layer2);
    //LogWrapper::new(multi.clone(), logger).try_init().unwrap();
    Registry::default().with(file_layer).with(layer2).init();

    (file_guard, stdout_guard)
}

fn init_bucket(credentials: HashMap<String, String>) -> Bucket {
    let access_key = Some(credentials.get("s3_access_key").unwrap().as_str());
    let secret_key = Some(credentials.get("s3_secret_key").unwrap().as_str());

    let creds = Credentials::new(access_key, secret_key, None, None, None).unwrap();
    let bucket = Bucket::new("repostrusty", Region::EuNorth1, creds).unwrap();

    bucket
}

fn read_credentials(path: &str) -> HashMap<String, HashMap<String, String>> {
    let mut file = File::open(path).expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Unable to read the credentials file");
    let credentials: HashMap<String, HashMap<String, String>> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    credentials
}
