extern crate r2d2;
extern crate r2d2_sqlite;

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::time::Duration;

use teloxide::prelude::ChatId;
use tokio::sync::mpsc;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{layer::SubscriberExt, Layer, Registry};

use crate::database::Database;
use crate::telegram_bot::BotManager;

mod database;
mod scraper;
mod telegram_bot;
mod utils;

const CHAT_ID: ChatId = ChatId(34957918);
const INTERFACE_UPDATE_INTERVAL: Duration = Duration::from_secs(60);
const REFRESH_RATE: Duration = Duration::from_secs(2);
const CONTENT_EXPIRY: Duration = Duration::from_secs(60 * 60 * 24);
const SCRAPER_LOOP_SLEEP_LEN: Duration = Duration::from_secs(60 * 90);
const SCRAPER_DOWNLOAD_SLEEP_LEN: Duration = Duration::from_secs(60 * 5);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (_file_guard, _stdout_guard) = init_logging();

    let is_offline = false;

    // Initialize the database
    let db = Database::new(is_offline)?;
    db.begin_transaction()?.reorder_pages()?;

    let all_credentials = read_credentials("config/credentials.yaml");

    let mut all_handles = Vec::new();

    for (username, credentials) in &all_credentials {
        if credentials.get("enabled").expect("No enabled field in credentials") == "true" {
            tracing::info!("Starting bot for user: {}", username);

            // Create a new channel
            let (tx, rx) = mpsc::channel(100);

            // Run the scraper and the bot concurrently
            let scraper = tokio::spawn(scraper::run_scraper(tx, db.clone(), is_offline, credentials.clone()));
            let bot_manager = BotManager::new(db.clone(), credentials.clone());

            let telegram_bot = tokio::spawn(async move { bot_manager.run_bot(rx).await });

            all_handles.push(scraper);
            all_handles.push(telegram_bot);
        }
    }

    // Wait for both tasks to complete
    let _ = futures::future::join_all(all_handles).await;

    Ok(())
}

fn init_logging() -> (tracing_appender::non_blocking::WorkerGuard, tracing_appender::non_blocking::WorkerGuard) {
    let file_appender = tracing_appender::rolling::hourly("logs/", "rolling.log");
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::Layer::new()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(false)
        .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE)
        .with_writer(non_blocking)
        .with_filter(LevelFilter::INFO);

    let (non_blocking, stdout_guard) = tracing_appender::non_blocking(std::io::stdout());
    let layer2 = tracing_subscriber::fmt::Layer::default()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(false)
        .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE)
        .with_writer(non_blocking)
        .with_filter(LevelFilter::WARN);

    Registry::default().with(file_layer).with(layer2).init();

    (file_guard, stdout_guard)
}

fn read_credentials(path: &str) -> HashMap<String, HashMap<String, String>> {
    let mut file = File::open(path).expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Unable to read the credentials file");
    let credentials: HashMap<String, HashMap<String, String>> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    credentials
}
