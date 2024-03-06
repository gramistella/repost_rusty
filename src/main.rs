extern crate r2d2;
extern crate r2d2_sqlite;

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::database::Database;

mod database;
mod scraper;
mod telegram_bot;
mod utils;

const REFRESH_RATE: Duration = Duration::from_secs(90);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let is_offline = false;

    // Initialize the database
    let db = Database::new(is_offline)?;

    let all_credentials = read_credentials("config/credentials.yaml");

    let mut all_handles = Vec::new();

    for (username, credentials) in &all_credentials {
        if credentials.get("enabled").expect("No enabled field in credentials") == "true"{

            println!("Starting bot for user: {}", username);

            // Create a new channel
            let (tx, rx) = mpsc::channel(100);

            // Run the scraper and the bot concurrently
            let scraper = tokio::spawn(scraper::run_scraper(tx, db.clone(), is_offline, credentials.clone()));
            let telegram_bot = tokio::spawn(telegram_bot::run_bot(rx, db.clone(), credentials.clone()));

            all_handles.push(scraper);
            all_handles.push(telegram_bot);
        }
    }



    // Wait for both tasks to complete
    let _ = futures::future::join_all(all_handles).await;

    Ok(())
}

fn read_credentials(path: &str) -> HashMap<String, HashMap<String, String>> {
    let mut file = File::open(path).expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Unable to read the credentials file");
    let credentials: HashMap<String, HashMap<String, String>> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    credentials
}
