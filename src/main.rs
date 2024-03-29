extern crate r2d2;
extern crate r2d2_sqlite;

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::ChatId;
use tokio::sync::mpsc;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{layer::SubscriberExt, Layer, Registry};

use crate::database::Database;
use crate::scraper::ScraperPoster;
use crate::telegram_bot::BotManager;

mod database;
mod scraper;
mod telegram_bot;
mod utils;

const CHAT_ID: ChatId = ChatId(34957918);
const INTERFACE_UPDATE_INTERVAL: Duration = Duration::from_secs(90);
const REFRESH_RATE: Duration = Duration::from_secs(6);
const SCRAPER_LOOP_SLEEP_LEN: Duration = Duration::from_secs(60 * 120);
const SCRAPER_DOWNLOAD_SLEEP_LEN: Duration = Duration::from_secs(60 * 10);
const IS_OFFLINE: bool = true;

fn main() -> anyhow::Result<()> {
    let (_file_guard, _stdout_guard) = init_logging();

    let all_credentials = read_credentials("config/credentials.yaml");
    let mut all_handles = Vec::new();

    for (username, credentials) in all_credentials {
        if credentials.get("enabled").expect("No enabled field in credentials") == "true" {
            let span = tracing::span!(tracing::Level::INFO, "main", username = username.as_str());
            let _enter = span.enter();
            tracing::info!("Starting bot for user: {}", username);

            // Create a single runtime for all threads
            let rt = Arc::new(tokio::runtime::Runtime::new().unwrap());

            let db = Database::new(username.clone()).unwrap();
            match rt.block_on(async { db.begin_transaction().await.unwrap().reorder_pages() }) {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("Error reordering pages: {:?}", e);
                }
            }

            // Create a new channel
            let (tx, rx) = mpsc::channel(100);

            let bot_manager = rt.block_on(async { BotManager::new(db.clone(), credentials.clone()) });
            let rt_clone_bot = Arc::clone(&rt);

            // Run the scraper and the bot concurrently
            let mut scraper_poster = rt.block_on(async { ScraperPoster::new(db.clone(), username.clone(), credentials.clone()) });
            let scraper = std::thread::spawn(move || rt.block_on(scraper_poster.run_scraper(tx)));
            let telegram_bot = std::thread::spawn(move || rt_clone_bot.block_on(async move { bot_manager.run_bot(rx, username).await }));

            all_handles.push(scraper);
            all_handles.push(telegram_bot);
        }
    }

    // Wait for both tasks to complete
    for handle in all_handles {
        handle.join().expect("Thread panicked");
    }

    Ok(())
}

fn init_logging() -> (tracing_appender::non_blocking::WorkerGuard, tracing_appender::non_blocking::WorkerGuard) {
    let file_appender = tracing_appender::rolling::hourly("logs/", "rolling.log");
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::Layer::new()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE)
        .with_writer(non_blocking)
        .with_filter(LevelFilter::INFO);

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
