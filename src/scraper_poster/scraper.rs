use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use instagram_scraper_rs::{InstagramScraper, InstagramScraperError, Post, User};
use rand::prelude::SliceRandom;
use rand::rngs::{OsRng, StdRng};
use rand::{Rng, SeedableRng};
use serenity::all::MessageId;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::Instrument;

use crate::discord_bot::bot::{INTERFACE_UPDATE_INTERVAL, REFRESH_RATE};
use crate::discord_bot::database::{ContentInfo, Database, FailedContent, PublishedContent, QueuedContent};
use crate::discord_bot::state::ContentStatus;
use crate::discord_bot::utils::now_in_my_timezone;
use crate::{FETCH_SLEEP_LEN, MAX_CONTENT_PER_ITERATION, SCRAPER_DOWNLOAD_SLEEP_LEN, SCRAPER_LOOP_SLEEP_LEN};

#[derive(Clone)]
pub struct ScraperPoster {
    username: String,
    scraper: Arc<Mutex<InstagramScraper>>,
    database: Database,
    is_restricted: bool,
    is_offline: bool,
    cookie_store_path: String,
    credentials: HashMap<String, String>,
    latest_content_mutex: Arc<Mutex<Option<(String, String, String, String)>>>,
}

impl ScraperPoster {
    pub fn new(database: Database, username: String, credentials: HashMap<String, String>, is_offline: bool) -> Self {
        let cookie_store_path = format!("cookies/cookies_{}.json", username);
        let scraper = Arc::new(Mutex::new(InstagramScraper::with_cookie_store(&cookie_store_path)));

        let latest_content_mutex = Arc::new(Mutex::new(None));

        Self {
            username,
            scraper,
            database,
            is_restricted: false,
            is_offline,
            cookie_store_path,
            credentials,
            latest_content_mutex,
        }
    }

    pub async fn run_scraper(&mut self) {
        let (sender_loop, scraper_loop) = self.scraper_loop().await;

        let poster_loop = self.poster_loop();

        let sender_span = tracing::span!(tracing::Level::INFO, "sender");
        let scraper_span = tracing::span!(tracing::Level::INFO, "scraper_poster");
        let poster_span = tracing::span!(tracing::Level::INFO, "poster");

        let _ = tokio::try_join!(sender_loop.instrument(sender_span), scraper_loop.instrument(scraper_span), poster_loop.instrument(poster_span));
    }

    //noinspection RsConstantConditionIf
    async fn scraper_loop(&mut self) -> (JoinHandle<anyhow::Result<()>>, JoinHandle<anyhow::Result<()>>) {
        let span = tracing::span!(tracing::Level::INFO, "outer_scraper_loop");
        let _enter = span.enter();
        let scraper_loop: JoinHandle<anyhow::Result<()>>;
        let mut accounts_to_scrape: HashMap<String, String> = read_accounts_to_scrape("config/accounts_to_scrape.yaml", self.username.as_str()).await;
        let hashtag_mapping: HashMap<String, String> = read_hashtag_mapping("config/hashtags.yaml").await;

        let mut transaction = self.database.begin_transaction().await.unwrap();
        let sender_latest_content = Arc::clone(&self.latest_content_mutex);
        let sender_loop = tokio::spawn(async move {
            loop {
                {
                    // Use a scoped block to avoid sleeping while the mutex is locked
                    let content_tuple = {
                        let lock = sender_latest_content.lock().await;
                        lock.clone()
                    };

                    let user_settings = transaction.load_user_settings().unwrap();

                    if let Some((url, caption, author, shortcode)) = content_tuple {
                        if shortcode == "halted" {
                            tokio::time::sleep(REFRESH_RATE).await;
                            continue;
                        }

                        if !transaction.does_content_exist_with_shortcode(shortcode.clone()) {
                            let re = regex::Regex::new(r"#\w+").unwrap();
                            let cloned_caption = caption.clone();
                            let hashtags: Vec<&str> = re.find_iter(&cloned_caption).map(|mat| mat.as_str()).collect();
                            let hashtags = hashtags.join(" ");
                            let caption = re.replace_all(&caption.clone(), "").to_string();
                            let now_string = now_in_my_timezone(&user_settings).to_rfc3339();

                            let video = ContentInfo {
                                username: user_settings.username.clone(),
                                url: url.clone(),
                                status: ContentStatus::Pending { shown: false },
                                caption,
                                hashtags,
                                original_author: author.clone(),
                                original_shortcode: shortcode.clone(),
                                last_updated_at: now_string.clone(),
                                url_last_updated_at: now_string.clone(),
                                added_at: now_string,
                                page_num: 1,
                                encountered_errors: 0,
                            };

                            let message_id = transaction.get_temp_message_id(user_settings);
                            let content_mapping: IndexMap<MessageId, ContentInfo> = IndexMap::from([(MessageId::new(message_id as u64), video.clone())]);

                            transaction.save_content_mapping(content_mapping).unwrap();
                        }
                    } else {
                        //tx.send(("".to_string(), "".to_string(), "".to_string(), "ignore".to_string())).await.unwrap();
                    }
                }
                tokio::time::sleep(REFRESH_RATE).await;
            }
        });

        if self.is_offline {
            let testing_urls = vec![
                "https://tekeye.uk/html/images/Joren_Falls_Izu_Jap.mp4",
                "https://www.w3schools.com/html/mov_bbb.mp4",
                "https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/ForBiggerEscapes.mp4",
                "https://tekeye.uk/html/images/Joren_Falls_Izu_Jap.mp4",
                "https://www.w3schools.com/html/mov_bbb.mp4",
                "https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/ForBiggerEscapes.mp4",
                "https://tekeye.uk/html/images/Joren_Falls_Izu_Jap.mp4",
                "https://www.w3schools.com/html/mov_bbb.mp4",
                "https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/ForBiggerEscapes.mp4",
                "https://tekeye.uk/html/images/Joren_Falls_Izu_Jap.mp4",
                "https://www.w3schools.com/html/mov_bbb.mp4",
                "https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/ForBiggerEscapes.mp4",
            ];

            println!("Sending offline data");

            let scraper_latest_content = Arc::clone(&self.latest_content_mutex);
            scraper_loop = tokio::spawn(async move {
                let mut loop_iterations = 0;
                loop {
                    loop_iterations += 1;
                    let mut inner_loop_iterations = 0;
                    for url in &testing_urls {
                        inner_loop_iterations += 1;
                        let caption_string;
                        if inner_loop_iterations == 2 {
                            caption_string = format!("Video {}, loop {} #meme, will_fail", inner_loop_iterations, loop_iterations);
                        } else {
                            caption_string = format!("Video {}, loop {} #meme", inner_loop_iterations, loop_iterations);
                        }

                        let mut latest_content_guard = scraper_latest_content.lock().await;
                        *latest_content_guard = Some((url.to_string(), caption_string.clone(), "local".to_string(), format!("shortcode{}", inner_loop_iterations)));
                        sleep(Duration::from_secs(10)).await;
                    }
                }
            });
        } else {
            let mut cloned_self = self.clone();

            scraper_loop = tokio::spawn(async move {
                let span = tracing::span!(tracing::Level::INFO, "online_scraper_loop");
                let _enter = span.enter();

                cloned_self.login_scraper().await;

                let mut accounts_being_scraped = Vec::new();

                cloned_self.fetch_user_info(&mut accounts_to_scrape, &mut accounts_being_scraped).await;

                // Since this is not within the loop, we need to handle it separately
                while cloned_self.is_restricted {
                    cloned_self.should_continue_due_to_restriction().await;
                    cloned_self.fetch_user_info(&mut accounts_to_scrape, &mut accounts_being_scraped).await;
                }

                loop {
                    let mut posts: HashMap<User, Vec<Post>> = HashMap::new();
                    cloned_self.fetch_posts(&accounts_being_scraped, &mut posts).await;

                    if cloned_self.should_continue_due_to_restriction().await {
                        continue;
                    }

                    // Scrape the posts
                    cloned_self.scrape_posts(&accounts_to_scrape, &hashtag_mapping, &mut posts).await;

                    // Wait for a while before the next iteration
                    cloned_self.println(&format!("Starting long sleep ({} minutes)", SCRAPER_LOOP_SLEEP_LEN.as_secs() / 60));
                    cloned_self.randomized_sleep(SCRAPER_LOOP_SLEEP_LEN.as_secs()).await;
                }
            });
        }
        (sender_loop, scraper_loop)
    }

    async fn scrape_posts(&mut self, accounts_to_scrape: &HashMap<String, String>, hashtag_mapping: &HashMap<String, String>, posts: &mut HashMap<User, Vec<Post>>) {
        let mut rng = StdRng::from_entropy();

        self.println("Scraping posts...");

        let mut flattened_posts: Vec<(User, Post)> = Vec::new();
        for (user, user_posts) in posts {
            for post in user_posts {
                flattened_posts.push((user.clone(), post.clone()));
            }
        }

        flattened_posts.shuffle(&mut rng);

        let mut flattened_posts_processed = 0;
        let flattened_posts_len = flattened_posts.len();

        let mut actually_scraped = 0;
        for (author, post) in flattened_posts {
            flattened_posts_processed += 1;

            if actually_scraped >= MAX_CONTENT_PER_ITERATION {
                self.println("Reached the maximum amount of scraped content per iteration");
                break;
            }
            
            let base_print = format!("{flattened_posts_processed}/{flattened_posts_len} - {actually_scraped}/{MAX_CONTENT_PER_ITERATION}");
            
            // Send the URL through the channel
            if post.is_video {
                let mut transaction = self.database.begin_transaction().await.unwrap();
                if transaction.does_content_exist_with_shortcode(post.shortcode.clone()) == false {
                    let url;
                    let caption;
                    {
                        let mut scraper_guard = self.scraper.lock().await;
                        (url, caption) = match scraper_guard.download_reel(&post.shortcode).await {
                            Ok((url, caption)) => {
                                actually_scraped += 1;
                                self.is_restricted = false;
                                let base_print = format!("{flattened_posts_processed}/{flattened_posts_len} - {actually_scraped}/{MAX_CONTENT_PER_ITERATION}");
                                self.println(&format!("{base_print} Scraped content from {}: {}", author.username, post.shortcode));
                                (url, caption)
                            }
                            Err(e) => {
                                self.println(&format!("Error while downloading reel | {}", e));
                                self.is_restricted = true;
                                let mut lock = self.latest_content_mutex.lock().await;
                                *lock = Some((e.to_string(), "caption".to_string(), author.username.clone(), "halted".to_string()));
                                break;
                            }
                        };
                    }

                    let caption = Self::process_caption(accounts_to_scrape, hashtag_mapping, &mut rng, &author, caption);

                    // Use a scoped block to immediately drop the lock
                    {
                        // Store the new URL in the shared variable
                        let mut lock = self.latest_content_mutex.lock().await;
                        //println!("Storing URL: {}", url);
                        *lock = Some((url, caption, author.username.clone(), post.shortcode.clone()));
                    }

                    self.save_cookie_store_to_json().await;
                } else {

                    let existing_content_shortcodes: Vec<String> = transaction.load_content_mapping().unwrap().values().map(|content_info| content_info.original_shortcode.clone()).collect();
                    let existing_posted_shortcodes: Vec<String> = transaction.load_posted_content().unwrap().iter().map(|existing_posted| existing_posted.original_shortcode.clone()).collect();
                    let existing_failed_shortcodes: Vec<String> = transaction.load_failed_content().unwrap().iter().map(|existing_posted| existing_posted.original_shortcode.clone()).collect();
                    let existing_rejected_shortcodes: Vec<String> = transaction.load_rejected_content().unwrap().iter().map(|existing_posted| existing_posted.original_shortcode.clone()).collect();

                    match existing_content_shortcodes.iter().position(|x| x == &post.shortcode) {
                        Some(_) => {
                            self.println(&format!("{base_print} Content already scraped: {}", post.shortcode));
                        }
                        None => {
                            // Check if the shortcode is in the posted, failed or rejected content
                            if existing_posted_shortcodes.contains(&post.shortcode) {
                                self.println(&format!("{base_print} Content already posted: {}", post.shortcode));
                            } else if existing_failed_shortcodes.contains(&post.shortcode) {
                                self.println(&format!("{base_print} Content already failed: {}", post.shortcode));
                            } else if existing_rejected_shortcodes.contains(&post.shortcode) {
                                self.println(&format!("{base_print} Content already rejected: {}", post.shortcode));
                            } else {
                                let error_message = format!("{base_print} Content not found in any mapping: {}", post.shortcode);
                                tracing::error!(error_message);
                                panic!("{}", error_message);
                            }
                        }
                    };
                }
            } else {

                self.println(&format!("{base_print} Content is not a video: {}", post.shortcode));
            }
            self.randomized_sleep(SCRAPER_DOWNLOAD_SLEEP_LEN.as_secs()).await;
        }
    }

    fn process_caption(accounts_to_scrape: &HashMap<String, String>, hashtag_mapping: &HashMap<String, String>, mut rng: &mut StdRng, author: &User, caption: String) -> String {
        // Check if the caption contains any hashtags

        // Sadasscats
        let caption = caption.replace(
            "\n-\n-\n-\n- credit: unknown (We do not claim ownership of this video, all rights are reserved and belong to their respective owners, no copyright infringement intended. Please DM us for credit/removal) tags:",
            "",
        );
        let caption = caption.replace("-", "");
        let caption = caption.replace("credit: unknown", "");
        let caption = caption.replace("(We do not claim ownership of this video, all rights are reserved and belong to their respective owners, no copyright infringement intended. Please DM us for credit/removal)", "");
        let caption = caption.replace("tags:", "");

        let caption = caption.replace("#softcatmemes", "");

        // Catvibenow
        let caption = caption.replace("•", "");
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
        let caption = caption.replace("-", "");

        let mut hashtags = caption.split_whitespace().filter(|s| s.starts_with('#')).collect::<Vec<&str>>();
        let selected_hashtags;
        if hashtags.len() > 0 {
            hashtags.shuffle(&mut rng);
            let hashtags = hashtags.join(" ");
            selected_hashtags = hashtags;
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
            let random_specific_hashtags: Vec<&str> = specific_hashtags.choose_multiple(&mut rng, 3).map(|s| *s).collect();

            // Join the selected hashtags into a single string
            selected_hashtags = format!("{} {} {} {}", random_general_hashtag, random_specific_hashtags.get(0).unwrap(), random_specific_hashtags.get(1).unwrap(), random_specific_hashtags.get(2).unwrap());
        }

        // Remove the hashtags from the caption
        let caption = caption.split_whitespace().filter(|s| !s.starts_with('#')).collect::<Vec<&str>>().join(" ");
        // Rebuild the caption
        let caption = format!("{} {}", caption, selected_hashtags);
        caption
    }

    /// This function checks if the account is restricted and sleeps for 30 minutes if it is.
    /// It updates the latest content shortcode to indicate that the scraper_poster has been halted.
    async fn should_continue_due_to_restriction(&mut self) -> bool {
        if self.is_restricted {
            self.println("Account awaits manual intervention, sleeping for 30 min");
            {
                let mut lock = self.latest_content_mutex.lock().await;
                *lock = Some(("Account awaits manual intervention".to_string(), "caption".to_string(), "author".to_string(), "halted".to_string()));
            }
            self.randomized_sleep(60 * 30).await;
            return true;
        }
        false
    }

    async fn fetch_posts(&mut self, accounts_being_scraped: &Vec<User>, posts: &mut HashMap<User, Vec<Post>>) {
        let mut accounts_scraped = 0;
        let accounts_being_scraped_len = accounts_being_scraped.len();
        self.println("Fetching posts...");
        for user in accounts_being_scraped.iter() {
            accounts_scraped += 1;
            self.println(&format!("{}/{} Retrieving posts from user {}", accounts_scraped, accounts_being_scraped_len, user.username));

            // get posts
            {
                let mut scraper_guard = self.scraper.lock().await;
                match scraper_guard.scrape_posts(&user.id, 5).await {
                    Ok(scraped_posts) => {
                        self.is_restricted = false;
                        posts.insert(user.clone(), scraped_posts);
                    }
                    Err(e) => {
                        self.println(&format!("Error scraping posts: {}", e));
                        self.is_restricted = true;
                        break;
                    }
                };
            }

            self.randomized_sleep(FETCH_SLEEP_LEN.as_secs()).await;
        }
    }

    async fn fetch_user_info(&mut self, accounts_to_scrape: &mut HashMap<String, String>, accounts_being_scraped: &mut Vec<User>) {
        let mut accounts_scraped = 0;
        let accounts_to_scrape_len = accounts_to_scrape.len();
        self.println("Fetching user info...");
        for (profile, _hashtags) in accounts_to_scrape.clone() {
            {
                accounts_scraped += 1;
                let mut scraper_guard = self.scraper.lock().await;
                let result = scraper_guard.scrape_userinfo(&profile).await;
                match result {
                    Ok(user) => {
                        accounts_being_scraped.push(user);
                        self.println(&format!("{}/{} Fetched user info for {}", accounts_scraped, accounts_to_scrape_len, profile));
                        self.is_restricted = false;
                    }
                    Err(e) => {
                        self.println(&format!("{}/{} Error fetching user info for {}: {}", accounts_scraped, accounts_to_scrape_len, profile, e));
                        match e {
                            InstagramScraperError::UserNotFound(profile) => {
                                accounts_to_scrape.remove(&profile);
                            }
                            _ => {
                                let mut lock = self.latest_content_mutex.lock().await;
                                *lock = Some((e.to_string(), "caption".to_string(), "author".to_string(), "halted".to_string()));
                                self.is_restricted = true;
                                break;
                            }
                        }
                    }
                };
            }

            self.randomized_sleep(FETCH_SLEEP_LEN.as_secs()).await;
        }
    }

    async fn login_scraper(&mut self) {
        let username = self.credentials.get("username").unwrap().clone();
        let password = self.credentials.get("password").unwrap().clone();

        {
            // Lock the scraper_poster
            let mut scraper_guard = self.scraper.lock().await;
            scraper_guard.authenticate_with_login(username.clone(), password.clone());
            self.println("Logging in...");
            let result = scraper_guard.login().await;
            match result {
                Ok(_) => {
                    self.println("Logged in successfully");
                }
                Err(e) => {
                    self.println(&format!(" Login failed: {}", e));
                }
            };
        }
        self.save_cookie_store_to_json().await;
    }

    fn poster_loop(&mut self) -> JoinHandle<anyhow::Result<()>> {
        let span = tracing::span!(tracing::Level::INFO, "poster_loop");
        let _enter = span.enter();
        let mut cloned_self = self.clone();
        let poster_loop = tokio::spawn(async move {
            cloned_self.amend_queue().await;
            // Allow the scraper_poster to login
            sleep(Duration::from_secs(30)).await;

            loop {
                let mut transaction = cloned_self.database.begin_transaction().await.unwrap();
                let content_mapping = transaction.load_content_mapping().unwrap();
                let user_settings = transaction.load_user_settings().unwrap();

                for (_message_id, content_info) in content_mapping {
                    if content_info.status.to_string().contains("queued_") {
                        let queued_posts = transaction.load_content_queue().unwrap();
                        for mut queued_post in queued_posts {
                            if DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap() < now_in_my_timezone(&user_settings) {
                                if user_settings.can_post {
                                    let mut scraper_guard = cloned_self.scraper.lock().await;

                                    if !cloned_self.is_offline {
                                        // Example of a caption:
                                        // "This is a cool caption!"
                                        // "•"
                                        // "•"
                                        // "•"
                                        // "•"
                                        // "•"
                                        // "(We don’t own this reel. All rights are reserved & belong to their respective owners, no copyright infringement intended. DM for credit/removal.)"
                                        // "•"
                                        // "#cool #caption #hashtags"

                                        let full_caption;
                                        let big_spacer = "\n\n\n•\n•\n•\n•\n•\n";
                                        let small_spacer = "\n•\n";
                                        let disclaimer = "(We don’t own this content. All rights are reserved & belong to their respective owners, no copyright infringement intended. DM for credit/removal.)";
                                        if queued_post.caption == "" && queued_post.hashtags == "" {
                                            full_caption = "".to_string();
                                        } else if queued_post.caption == "" {
                                            full_caption = format!("{}", queued_post.hashtags);
                                        } else if queued_post.hashtags == "" {
                                            full_caption = format!("{}", queued_post.caption);
                                        } else {
                                            full_caption = format!("{}{}{}{}{}", queued_post.caption, big_spacer, disclaimer, small_spacer, queued_post.hashtags);
                                        }

                                        let user_id = cloned_self.credentials.get("instagram_business_account_id").unwrap();
                                        let access_token = cloned_self.credentials.get("fb_access_token").unwrap();

                                        let (_message_id, content_info) = transaction.get_content_info_by_shortcode(queued_post.original_shortcode.clone()).unwrap();

                                        if DateTime::parse_from_rfc3339(&content_info.url_last_updated_at).unwrap() < now_in_my_timezone(&user_settings) - Duration::from_secs(60 * 60 * 12) {
                                            //cloned_self.println(&format!("[+] Updating content url before posting: {}", queued_post.original_shortcode));
                                            match scraper_guard.download_reel(&queued_post.original_shortcode).await {
                                                Ok((url, _caption)) => {
                                                    queued_post.url = url;
                                                }
                                                Err(err) => {
                                                    cloned_self.println(&format!("[!] couldn't update reel url!\n [ERROR] {}", err.to_string()));
                                                    match err {
                                                        InstagramScraperError::MediaNotFound(_) => {
                                                            drop(scraper_guard);
                                                            cloned_self.handle_failed_content(&mut queued_post).await;
                                                            continue;
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                            }
                                        }

                                        cloned_self.println(&format!("[+] Publishing content to instagram: {}", queued_post.original_shortcode));
                                        let timer = std::time::Instant::now();
                                        match scraper_guard.upload_reel(user_id, access_token, &queued_post.url, &full_caption).await {
                                            Ok(_) => {
                                                let duration = timer.elapsed(); // End timer

                                                let minutes = duration.as_secs() / 60;
                                                let seconds = duration.as_secs() % 60;

                                                cloned_self.println(&format!("[+] Published content successfully: {}, took {} minutes and {} seconds", queued_post.original_shortcode, minutes, seconds));
                                            }
                                            Err(err) => {
                                                match err {
                                                    InstagramScraperError::UploadFailedRecoverable(_) => {
                                                        cloned_self.println(&format!("[!] Couldn't upload content to instagram! Trying again later\n [WARNING] {}", err.to_string()));
                                                        drop(scraper_guard);
                                                        cloned_self.handle_recoverable_failed_content().await;
                                                    }
                                                    InstagramScraperError::UploadFailedNonRecoverable(_) => {
                                                        cloned_self.println(&format!("[!] Couldn't upload content to instagram!\n [ERROR] {}\n{}", err.to_string(), queued_post.url));
                                                        drop(scraper_guard);
                                                        cloned_self.handle_failed_content(&mut queued_post).await;
                                                    }
                                                    _ => {}
                                                }
                                                continue;
                                            }
                                        }
                                    } else {
                                        if queued_post.caption.contains("will_fail") {
                                            cloned_self.println(&format!("[!] Failed to upload content offline: {}", queued_post.url));
                                            drop(scraper_guard);
                                            cloned_self.handle_failed_content(&mut queued_post).await;
                                            continue;
                                        } else {
                                            cloned_self.println(&format!("[!] Uploaded content offline: {}", queued_post.url));
                                        }
                                    }

                                    let (message_id, mut video_info) = transaction.get_content_info_by_shortcode(queued_post.original_shortcode.clone()).unwrap();
                                    video_info.status = ContentStatus::Published { shown: false };
                                    let index_map = IndexMap::from([(message_id, video_info.clone())]);
                                    transaction.save_content_mapping(index_map).unwrap();

                                    let published_content = PublishedContent {
                                        username: queued_post.username.clone(),
                                        url: queued_post.url.clone(),
                                        caption: queued_post.caption.clone(),
                                        hashtags: queued_post.hashtags.clone(),
                                        original_author: queued_post.original_author.clone(),
                                        original_shortcode: queued_post.original_shortcode.clone(),
                                        published_at: now_in_my_timezone(&user_settings).to_rfc3339(),
                                        last_updated_at: now_in_my_timezone(&user_settings).to_rfc3339(),
                                        expired: false,
                                    };

                                    transaction.save_published_content(published_content).unwrap();
                                } else {
                                    let new_will_post_at = transaction.get_new_post_time().unwrap();
                                    queued_post.will_post_at = new_will_post_at;
                                    transaction.save_queued_content(queued_post.clone()).unwrap();

                                    let (message_id, mut video_info) = transaction.get_content_info_by_shortcode(queued_post.original_shortcode.clone()).unwrap();
                                    video_info.status = ContentStatus::Queued { shown: false };
                                    let index_map = IndexMap::from([(message_id, video_info.clone())]);
                                    transaction.save_content_mapping(index_map).unwrap();
                                }
                            }
                        }
                    }
                }

                // Don't remove this sleep, without it the bot becomes completely unresponsive
                sleep(REFRESH_RATE).await;
            }
        });
        poster_loop
    }

    async fn handle_failed_content(&mut self, queued_post: &mut QueuedContent) {
        let span = tracing::span!(tracing::Level::INFO, "handle_failed_content");
        let _enter = span.enter();

        let mut transaction = self.database.begin_transaction().await.unwrap();
        let user_settings = transaction.load_user_settings().unwrap();
        let (message_id, mut video_info) = transaction.get_content_info_by_shortcode(queued_post.original_shortcode.clone()).unwrap();
        video_info.status = ContentStatus::Failed { shown: false };

        let index_map = IndexMap::from([(message_id, video_info.clone())]);
        transaction.save_content_mapping(index_map).unwrap();

        let now = now_in_my_timezone(&user_settings).to_rfc3339();
        let last_updated_at = (now_in_my_timezone(&user_settings) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
        let failed_content = FailedContent {
            username: queued_post.username.clone(),
            url: queued_post.url.clone(),
            caption: queued_post.caption.clone(),
            hashtags: queued_post.hashtags.clone(),
            original_author: queued_post.original_author.clone(),
            original_shortcode: queued_post.original_shortcode.clone(),
            last_updated_at,
            failed_at: now,
            expired: false,
        };

        transaction.save_failed_content(failed_content.clone()).unwrap();
    }

    // Move all the queued content
    async fn handle_recoverable_failed_content(&mut self) {
        let span = tracing::span!(tracing::Level::INFO, "handle_recoverable_failed_content");
        let _enter = span.enter();

        let mut transaction = self.database.begin_transaction().await.unwrap();
        let user_settings = transaction.load_user_settings().unwrap();

        for mut queued_post in transaction.load_content_queue().unwrap() {
            // Add 1/2 of the posting interval to the will_post_at time
            let new_will_post_at = DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap() + Duration::from_secs((user_settings.posting_interval * 30) as u64);
            queued_post.last_updated_at = (now_in_my_timezone(&user_settings) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
            queued_post.will_post_at = new_will_post_at.to_rfc3339();
            transaction.save_queued_content(queued_post.clone()).unwrap();
        }
    }

    async fn save_cookie_store_to_json(&mut self) {
        let span = tracing::span!(tracing::Level::INFO, "save_cookie_store_to_json");
        let _enter = span.enter();
        let mut writer = std::fs::File::create(self.cookie_store_path.clone()).map(std::io::BufWriter::new).unwrap();

        let scraper_guard = self.scraper.lock().await;
        let cookie_store = Arc::clone(&scraper_guard.session.cookie_store);
        cookie_store.lock().unwrap().save_json(&mut writer).expect("ERROR in scraper_poster.rs, failed to save cookie_store!");
    }

    /// Randomized sleep function, will randomize the sleep duration by up to 30% of the original duration
    async fn randomized_sleep(&mut self, original_duration: u64) {
        let span = tracing::span!(tracing::Level::INFO, "randomized_sleep");
        let mut rng = StdRng::from_rng(OsRng).unwrap();
        let variance: u64 = rng.gen_range(0..=1); // generates a number between 0 and 1
        let sleep_duration = original_duration + (original_duration * variance * 3 / 10); // add up to 30% of the original sleep duration
        span.in_scope(|| {
            tracing::info!(" [{}] - Sleeping for {} seconds", self.username, sleep_duration);
        });

        sleep(Duration::from_secs(sleep_duration)).await;
    }

    /// This function will amend the queue to ensure that only one post is posted at a time,
    /// even if the bot was shut down for a while.
    async fn amend_queue(&self) {
        let mut tx = self.database.begin_transaction().await.unwrap();
        let content_queue = tx.load_content_queue().unwrap();
        let user_settings = tx.load_user_settings().unwrap();
        let mut content_to_post = 0;
        for queued_post in content_queue.clone() {
            if DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap() < now_in_my_timezone(&user_settings) {
                self.println(&format!("Amending queue: {}", queued_post.original_shortcode));
                content_to_post += 1;
            }
        }

        if content_to_post > 1 {
            // Determine difference between the current time and the time the first post will be posted
            let first_post_time = DateTime::parse_from_rfc3339(&content_queue.first().unwrap().will_post_at).unwrap();

            // Calculate the time difference between the first post and now
            let time_difference = now_in_my_timezone(&user_settings) - first_post_time.with_timezone(&Utc);

            // Add the time difference to all the posts
            for mut queued_post in content_queue {
                let new_will_post_at = DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap() + time_difference;
                self.println(&format!("Changing post time for {}: {}", queued_post.original_shortcode, new_will_post_at));
                queued_post.will_post_at = new_will_post_at.to_rfc3339();
                tx.save_queued_content(queued_post.clone()).unwrap();
            }
        }
    }

    fn println(&self, message: &str) {
        println!(" [{}] - {}", self.username, message);
    }
}

async fn read_accounts_to_scrape(path: &str, username: &str) -> HashMap<String, String> {
    let mut file = File::open(path).await.expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).await.expect("Unable to read the credentials file");
    let accounts: HashMap<String, HashMap<String, String>> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    accounts.get(username).unwrap().clone()
}

async fn read_hashtag_mapping(path: &str) -> HashMap<String, String> {
    let mut file = File::open(path).await.expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).await.expect("Unable to read the credentials file");
    let hashtags: HashMap<String, String> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    hashtags
}
