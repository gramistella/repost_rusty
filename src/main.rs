extern crate r2d2;
extern crate r2d2_sqlite;

use std::{backtrace, env};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{layer::SubscriberExt, Layer, Registry};

use crate::database::Database;
use crate::discord_bot::bot::DiscordBot;
use crate::scraper_poster::scraper::ScraperPoster;

mod database;
mod discord_bot;
mod s3;
mod scraper_poster;
mod video_processing;

// Main configuration
const IS_OFFLINE: bool = false;

// Scraper configuration
const MAX_CONTENT_PER_ITERATION: usize = 8;
const FETCH_SLEEP_LEN: Duration = Duration::from_secs(60);
const SCRAPER_DOWNLOAD_SLEEP_LEN: Duration = Duration::from_secs(60 * 20);
const SCRAPER_LOOP_SLEEP_LEN: Duration = Duration::from_secs(60 * 60 * 12);

fn main() -> anyhow::Result<()> {
    let (_file_guard, _stdout_guard) = init_logging();

    env::set_var("RUST_BACKTRACE", "1");

    let all_credentials = read_credentials("config/credentials.yaml");
    let mut all_handles = Vec::new();

    let mut is_first_run = true;
    for (username, credentials) in all_credentials {
        if credentials.get("enabled").expect("No enabled field in credentials") == "true" {
            let span = tracing::span!(tracing::Level::INFO, "main", username = username.as_str());
            let _enter = span.enter();
            tracing::info!("Starting bot for user: {}", username);

            let rt_scraper_poster = Arc::new(tokio::runtime::Runtime::new().unwrap());
            let rt_discord_bot = Arc::new(tokio::runtime::Runtime::new().unwrap());

            let db = Database::new(username.clone()).unwrap();
            match rt_discord_bot.block_on(async { db.begin_transaction().await.unwrap().reorder_pages() }) {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("Error reordering pages: {:?}", e);
                }
            }

            let mut discord_bot_manager = rt_discord_bot.block_on(async { DiscordBot::new(db.clone(), credentials.clone(), is_first_run).await });

            // Run the scraper_poster and the bot concurrently
            let mut scraper_poster = ScraperPoster::new(db.clone(), username.clone(), credentials.clone(), IS_OFFLINE);
            let scraper = std::thread::spawn(move || rt_scraper_poster.block_on(scraper_poster.run_scraper()));

            let discord = std::thread::spawn(move || rt_discord_bot.block_on(async { discord_bot_manager.run_bot().await }));

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

fn read_credentials(path: &str) -> HashMap<String, HashMap<String, String>> {
    let mut file = File::open(path).expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Unable to read the credentials file");
    let credentials: HashMap<String, HashMap<String, String>> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    credentials
}
