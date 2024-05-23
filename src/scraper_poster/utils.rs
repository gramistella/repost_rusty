use std::collections::HashMap;
use std::sync::Arc;
use chrono::Duration;

use instagram_scraper_rs::User;
use rand::prelude::{SliceRandom, StdRng};
use reqwest_cookie_store::CookieStoreMutex;

use crate::database::database::DatabaseTransaction;
use crate::discord::utils::now_in_my_timezone;
use crate::{SCRAPER_REFRESH_RATE};

pub async fn save_cookie_store_to_json(cookie_store_path: &String, cookie_store_mutex: Arc<CookieStoreMutex>) {
    let span = tracing::span!(tracing::Level::INFO, "save_cookie_store_to_json");
    let _enter = span.enter();
    let mut writer = std::fs::File::create(cookie_store_path).map(std::io::BufWriter::new).unwrap();

    cookie_store_mutex.lock().unwrap().save_json(&mut writer).expect("ERROR in scraper utils, failed to save cookie_store!");
}

pub async fn pause_scraper_if_needed(tx: &mut DatabaseTransaction) {
    loop {
        let bot_status = tx.load_bot_status().await;
        if bot_status.manual_mode || bot_status.status != 0 {
            tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
        } else {
            break;
        }
    }
}

pub async fn set_bot_status_halted(tx: &mut DatabaseTransaction) {
    let mut bot_status = tx.load_bot_status().await;
    let mut user_settings = tx.load_user_settings().await;
    user_settings.can_post = false;
    bot_status.status = 1;
    bot_status.status_message = "halted  ⚠️".to_string();
    bot_status.last_updated_at = (now_in_my_timezone(&user_settings) - Duration::milliseconds(user_settings.interface_update_interval)).to_rfc3339();
    tx.save_bot_status(&bot_status).await;
    tx.save_user_settings(&user_settings).await;
}

pub async fn set_bot_status_operational(tx: &mut DatabaseTransaction) {
    let mut bot_status = tx.load_bot_status().await;
    let mut user_settings = tx.load_user_settings().await;
    user_settings.can_post = true;
    bot_status.status = 0;
    bot_status.status_message = "operational  🟢".to_string();
    bot_status.last_updated_at = (now_in_my_timezone(&user_settings) - Duration::milliseconds(user_settings.interface_update_interval)).to_rfc3339();
    tx.save_bot_status(&bot_status).await;
    tx.save_user_settings(&user_settings).await;
}

pub fn process_caption(accounts_to_scrape: &HashMap<String, String>, hashtag_mapping: &HashMap<String, String>, mut rng: &mut StdRng, author: &User, caption: String) -> String {
    // Check if the caption contains any hashtags

    // Sadasscats
    let caption = caption.replace(
        "\n-\n-\n-\n- credit: unknown (We do not claim ownership of this video, all rights are reserved and belong to their respective owners, no copyright infringement intended. Please DM us for credit/removal) tags:",
        "",
    );
    let caption = caption.replace('-', "");
    let caption = caption.replace("credit: unknown", "");
    let caption = caption.replace("(We do not claim ownership of this video, all rights are reserved and belong to their respective owners, no copyright infringement intended. Please DM us for credit/removal)", "");
    let caption = caption.replace("tags:", "");

    let caption = caption.replace("#softcatmemes", "");

    // Catvibenow
    let caption = caption.replace('•', "");
    let caption = caption.replace("Follow @catvibenow for your cuteness update 🧐", "");
    let caption = caption.replace("Credit📸:", "");
    let caption = caption.replace("(We don’t own this picture/photo. All rights are reserved & belong to their respective owners, no copyright infringement intended. DM for removal.)", "");

    // purrfectfelinevids
    let caption = caption.replace("\n\n.\n.\n.\n.\n.", "");
    let caption = caption.replace("\n.\n.\n.\n.\n.", "");
    let caption = caption.replace("\n.\n.\n.\n.", "");
    let caption = caption.replace("\n.\n.\n.", "");
    let caption = caption.replace(" (we do not claim ownership of this video, all rights are reserved and belong to their respective owners, no copyright infringement intended. please dm us for credit/removal)", "");

    // instantgatos
    let caption = caption.replace("Follow @instantgatos for more", "");
    let caption = caption.replace("Follow @gatosforyou for more", "");

    // kingcattos
    let caption = caption.replace('-', "");
    let caption = caption.replace("∧,,,∧", "");
    let caption = caption.replace("( · )", "");
    let caption = caption.replace("づ♡", "");
    let caption = caption.replace("\\", "");
    let caption = caption.replace("Follow @rartcattos @kingcattos", "");
    let caption = caption.replace("Follow @kingcattos", "");
    let caption = caption.replace("please DM for credit/removal", "");

    let mut hashtags = caption.split_whitespace().filter(|s| s.starts_with('#')).collect::<Vec<&str>>();
    let selected_hashtags = if !hashtags.is_empty() {
        hashtags.shuffle(&mut rng);
        hashtags.join(" ")
    } else {
        let hashtag_type = accounts_to_scrape.get(&author.username.clone()).unwrap().clone();
        let specific_hashtags = hashtag_mapping.get(&hashtag_type).unwrap().clone();
        let general_hashtags = hashtag_mapping.get("general").unwrap().clone();

        // Convert hashtag string from "#hastag, #hashtag2" to vec, and then pick 3 random hashtags
        // Split the string into a vector and trim each element
        fn split_hashtags(hashtags: &str) -> Vec<&str> {
            hashtags.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect()
        }

        let specific_hashtags = split_hashtags(&specific_hashtags);
        let general_hashtags = split_hashtags(&general_hashtags);

        // Select one random general hashtag
        let random_general_hashtag = general_hashtags.choose(&mut rng).unwrap().to_string();

        // Select three random specific hashtags
        let random_specific_hashtags: Vec<&str> = specific_hashtags.choose_multiple(&mut rng, 3).copied().collect();

        // Join the selected hashtags into a single string
        format!("{} {} {} {}", random_general_hashtag, random_specific_hashtags.first().unwrap(), random_specific_hashtags.get(1).unwrap(), random_specific_hashtags.get(2).unwrap())
    };

    // Remove the hashtags from the caption
    let caption = caption.split_whitespace().filter(|s| !s.starts_with('#')).collect::<Vec<&str>>().join(" ");
    // Rebuild the caption
    let caption = format!("{} {}", caption, selected_hashtags);
    caption
}
