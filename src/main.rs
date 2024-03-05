extern crate r2d2;
extern crate r2d2_sqlite;

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

use tokio::sync::mpsc;

use crate::database::Database;

mod database;
mod scraper;
mod telegram_bot;
mod utils;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let is_offline = false;

    // Initialize the database
    let db = Database::new(is_offline)?;

    // Create a new channel
    let (tx, rx) = mpsc::channel(100);

    let credentials = read_credentials("config/credentials.yaml");

    // Run the scraper and the bot concurrently
    let scraper = tokio::spawn(scraper::run_scraper(tx, db.clone(), is_offline, credentials.clone()));
    let telegram_bot = tokio::spawn(telegram_bot::run_bot(rx, db, credentials));

    // Wait for both tasks to complete
    let _ = tokio::try_join!(scraper, telegram_bot)?;

    Ok(())
}

fn read_credentials(path: &str) -> HashMap<String, String> {
    let mut file = File::open(path).expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Unable to read the credentials file");
    let credentials: HashMap<String, String> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    credentials
}
