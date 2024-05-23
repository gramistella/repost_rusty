use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use instagram_scraper_rs::{InstagramScraper, InstagramScraperError, Post, User};
use rand::prelude::SliceRandom;
use rand::rngs::{OsRng, StdRng};
use rand::{Rng, SeedableRng};
use serenity::all::MessageId;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::Instrument;

use crate::database::database::{ContentInfo, Database, DatabaseTransaction, DuplicateContent};
use crate::discord::state::ContentStatus;
use crate::discord::utils::now_in_my_timezone;
use crate::s3::helper::upload_to_s3;
use crate::scraper_poster::utils::{pause_scraper_if_needed, process_caption, save_cookie_store_to_json, set_bot_status_halted, set_bot_status_operational};
use crate::video::processing::process_video;
use crate::{FETCH_SLEEP_LEN, MAX_CONTENT_PER_ITERATION, SCRAPER_DOWNLOAD_SLEEP_LEN, SCRAPER_LOOP_SLEEP_LEN};
use crate::{MAX_CONTENT_HANDLED, SCRAPER_REFRESH_RATE};

#[derive(Clone)]
pub struct ContentManager {
    pub(crate) username: String,
    pub(crate) scraper: Arc<Mutex<InstagramScraper>>,
    pub(crate) database: Database,
    pub(crate) is_offline: bool,
    cookie_store_path: String,
    pub(crate) credentials: HashMap<String, String>,
    latest_content_mutex: Arc<Mutex<Option<(String, String, String, String)>>>,
}

impl ContentManager {
    pub fn new(database: Database, username: String, credentials: HashMap<String, String>, is_offline: bool) -> Self {
        let cookie_store_path = format!("cookies/cookies_{}.json", username);
        let scraper = Arc::new(Mutex::new(InstagramScraper::with_cookie_store(&cookie_store_path)));

        let latest_content_mutex = Arc::new(Mutex::new(None));

        Self {
            username,
            scraper,
            database,
            is_offline,
            cookie_store_path,
            credentials,
            latest_content_mutex,
        }
    }

    pub async fn run(&mut self) {
        let (sender_loop, scraper_loop) = self.scraper_loop().await;

        let poster_loop = self.poster_loop();

        let sender_span = tracing::span!(tracing::Level::INFO, "sender");
        let scraper_span = tracing::span!(tracing::Level::INFO, "scraper_poster");
        let poster_span = tracing::span!(tracing::Level::INFO, "poster");

        let _ = tokio::try_join!(sender_loop.instrument(sender_span), scraper_loop.instrument(scraper_span), poster_loop.instrument(poster_span));
    }

    async fn scraper_loop(&mut self) -> (JoinHandle<anyhow::Result<()>>, JoinHandle<anyhow::Result<()>>) {
        let span = tracing::span!(tracing::Level::INFO, "outer_scraper_loop");
        let _enter = span.enter();
        let scraper_loop: JoinHandle<anyhow::Result<()>>;
        let mut accounts_to_scrape: HashMap<String, String> = read_accounts_to_scrape("config/accounts_to_scrape.yaml", self.username.as_str()).await;
        let hashtag_mapping: HashMap<String, String> = read_hashtag_mapping("config/hashtags.yaml").await;

        let mut transaction = self.database.begin_transaction().await;
        let username = self.username.clone();
        let credentials = self.credentials.clone();
        let sender_latest_content = Arc::clone(&self.latest_content_mutex);
        let sender_loop = tokio::spawn(async move {
            loop {
                {
                    // Use a scoped block to avoid sleeping while the mutex is locked
                    let content_tuple = {
                        let lock = sender_latest_content.lock().await;
                        lock.clone()
                    };

                    let user_settings = transaction.load_user_settings().await;

                    let bot_status = transaction.load_bot_status().await;

                    if bot_status.status != 0 {
                        tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
                        continue;
                    }

                    if let Some((video_file_name, caption, author, shortcode)) = content_tuple {
                        if !transaction.does_content_exist_with_shortcode(shortcode.clone()).await && shortcode != "halted" {
                            // Process video to check if it already exists
                            let video_exists = process_video(&mut transaction, &video_file_name, author.clone(), shortcode.clone()).await.unwrap();

                            if video_exists {
                                println!("The same video is already in the database with a different shortcode, skipping! :)");

                                let duplicate_content = DuplicateContent {
                                    username: username.clone(),
                                    original_shortcode: shortcode.clone(),
                                };

                                transaction.save_duplicate_content(duplicate_content).await;
                                continue;
                            }

                            // Upload the video to S3
                            let s3_filename = format!("{}/{}", username, video_file_name);
                            let url = upload_to_s3(&credentials, video_file_name, s3_filename, true).await.unwrap();

                            let re = regex::Regex::new(r"#\w+").unwrap();
                            let cloned_caption = caption.clone();
                            let hashtags: Vec<&str> = re.find_iter(&cloned_caption).map(|mat| mat.as_str()).collect();
                            let hashtags = hashtags.join(" ");
                            let caption = re.replace_all(&caption.clone(), "").to_string();
                            let now_string = now_in_my_timezone(&user_settings).to_rfc3339();

                            let message_id = transaction.get_temp_message_id(&user_settings).await;

                            let video = ContentInfo {
                                username: user_settings.username.clone(),
                                message_id: MessageId::new(message_id),
                                url: url.clone(),
                                status: ContentStatus::Pending { shown: false },
                                caption,
                                hashtags,
                                original_author: author.clone(),
                                original_shortcode: shortcode.clone(),
                                last_updated_at: now_string.clone(),
                                added_at: now_string,
                                encountered_errors: 0,
                            };

                            transaction.save_content_info(&video).await;
                        }
                    } else {
                        //tx.send(("".to_string(), "".to_string(), "".to_string(), "ignore".to_string())).await.unwrap();
                    }
                }
                tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
            }
        });

        if self.is_offline {
            let testing_urls = vec![
                "https://tekeye.uk/html/images/Joren_Falls_Izu_Jap.mp4",
                "https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/ForBiggerEscapes.mp4",
                "https://tekeye.uk/html/images/Joren_Falls_Izu_Jap.mp4",
                "https://www.w3schools.com/html/mov_bbb.mp4",
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
                        let caption_string = if inner_loop_iterations == 2 {
                            format!("Video {}, loop {} #meme, will_fail", inner_loop_iterations, loop_iterations)
                        } else {
                            format!("Video {}, loop {} #meme", inner_loop_iterations, loop_iterations)
                        };

                        let path = format!("temp/shortcode{}.mp4", inner_loop_iterations);
                        let response = reqwest::get(url.to_string()).await.unwrap();
                        let bytes = response.bytes().await.unwrap();
                        let mut file = File::create(path.clone()).await.unwrap();
                        file.write_all(&bytes).await.unwrap();

                        let mut latest_content_guard = scraper_latest_content.lock().await;
                        *latest_content_guard = Some((format!("../{path}").to_string(), caption_string.clone(), "local".to_string(), format!("shortcode{}", inner_loop_iterations)));
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

                loop {
                    let content_mapping_len = cloned_self.database.begin_transaction().await.load_content_mapping().await.len();

                    if content_mapping_len >= MAX_CONTENT_HANDLED {
                        cloned_self.println("Reached the maximum amount of handled content");
                        cloned_self.println(&format!("Starting long sleep ({} minutes)", SCRAPER_LOOP_SLEEP_LEN.as_secs() / 60));
                        cloned_self.randomized_sleep(SCRAPER_LOOP_SLEEP_LEN.as_secs()).await;

                        continue;
                    }

                    let mut posts: HashMap<User, Vec<Post>> = HashMap::new();
                    cloned_self.fetch_posts(accounts_being_scraped.clone(), &mut posts).await;

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
                    let mut tx = self.database.begin_transaction().await;
                    set_bot_status_halted(&mut tx).await;

                    loop {
                        let bot_status = tx.load_bot_status().await;
                        if bot_status.status == 0 {
                            self.println("Retrying to log in...");
                            scraper_guard.authenticate_with_login(username.clone(), password.clone());
                            let result = scraper_guard.login().await;
                            match result {
                                Ok(_) => {
                                    self.println("Logged in successfully");
                                    set_bot_status_operational(&mut tx).await;
                                    break;
                                }
                                Err(e) => {
                                    self.println(&format!(" Login failed: {}", e));
                                    set_bot_status_halted(&mut tx).await;
                                }
                            }
                        } else {
                            tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
                        }
                    }
                }
            };

            let cookie_store = Arc::clone(&scraper_guard.session.cookie_store);
            save_cookie_store_to_json(&self.cookie_store_path, cookie_store).await;
        }
    }

    async fn fetch_user_info(&mut self, accounts_to_scrape: &mut HashMap<String, String>, accounts_being_scraped: &mut Vec<User>) {
        let mut tx = self.database.begin_transaction().await;

        pause_scraper_if_needed(&mut tx).await;
        let mut accounts_scraped = 0;
        let accounts_to_scrape_len = accounts_to_scrape.len();
        self.println("Fetching user info...");
        for (profile, _hashtags) in accounts_to_scrape.clone() {
            {
                pause_scraper_if_needed(&mut tx).await;

                accounts_scraped += 1;
                let mut scraper_guard = self.scraper.lock().await;
                let result = scraper_guard.scrape_userinfo(&profile).await;

                match result {
                    Ok(user) => {
                        accounts_being_scraped.push(user);
                        self.println(&format!("{}/{} Fetched user info for {}", accounts_scraped, accounts_to_scrape_len, profile));
                        set_bot_status_operational(&mut tx).await;
                    }
                    Err(e) => {
                        self.println(&format!("{}/{} Error fetching user info for {}: {}", accounts_scraped, accounts_to_scrape_len, profile, e));
                        match e {
                            InstagramScraperError::UserNotFound(profile) => {
                                accounts_to_scrape.remove(&profile);
                            }
                            InstagramScraperError::Http(error) => {
                                let error = format!("{}", error);
                                if error.contains("error sending request for url") {
                                    // Try again
                                    self.println("Automatically retrying to fetch user info...");
                                    let result = scraper_guard.scrape_userinfo(&profile).await;
                                    match result {
                                        Ok(user) => {
                                            accounts_being_scraped.push(user);
                                            self.println(&format!("{}/{} Fetched user info for {}", accounts_scraped, accounts_to_scrape_len, profile));
                                            set_bot_status_operational(&mut tx).await;
                                        }
                                        Err(e) => {
                                            self.println(&format!("{}/{} Error fetching user info for {}: {}", accounts_scraped, accounts_to_scrape_len, profile, e));
                                            set_bot_status_halted(&mut tx).await;
                                            self.fetch_user_info_halted_loop(accounts_being_scraped, &mut tx, &mut accounts_scraped, &accounts_to_scrape_len, &profile, &mut *scraper_guard).await;
                                        }
                                    }
                                }
                            }
                            _ => {
                                set_bot_status_halted(&mut tx).await;
                                self.fetch_user_info_halted_loop(accounts_being_scraped, &mut tx, &mut accounts_scraped, &accounts_to_scrape_len, &profile, &mut *scraper_guard).await;
                            }
                        }
                    }
                };
            }

            self.randomized_sleep(FETCH_SLEEP_LEN.as_secs()).await;
        }
    }

    async fn fetch_user_info_halted_loop(&self, accounts_being_scraped: &mut Vec<User>, mut tx: &mut DatabaseTransaction, accounts_scraped: &mut i32, accounts_to_scrape_len: &usize, profile: &String, scraper_guard: &mut InstagramScraper) {
        loop {
            let bot_status = tx.load_bot_status().await;
            if bot_status.status == 0 {
                self.println("Retrying to fetch user info...");
                let result = scraper_guard.scrape_userinfo(&profile).await;
                match result {
                    Ok(user) => {
                        accounts_being_scraped.push(user);
                        self.println(&format!("{}/{} Fetched user info for {}", accounts_scraped, accounts_to_scrape_len, profile));
                        set_bot_status_operational(&mut tx).await;
                        break;
                    }
                    Err(e) => {
                        self.println(&format!("{}/{} Error fetching user info for {}: {}", accounts_scraped, accounts_to_scrape_len, profile, e));
                        set_bot_status_halted(&mut tx).await;
                    }
                }
            } else {
                tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
            }
        }
    }

    async fn fetch_posts(&mut self, accounts_being_scraped: Vec<User>, posts: &mut HashMap<User, Vec<Post>>) {
        let mut tx = self.database.begin_transaction().await;
        pause_scraper_if_needed(&mut tx).await;
        let mut accounts_scraped = 0;
        let accounts_being_scraped_len = accounts_being_scraped.len();
        self.println("Fetching posts...");
        for user in accounts_being_scraped.iter() {
            // get posts
            {
                pause_scraper_if_needed(&mut tx).await;

                let mut scraper_guard = self.scraper.lock().await;
                accounts_scraped += 1;
                self.println(&format!("{}/{} Retrieving posts from user {}", accounts_scraped, accounts_being_scraped_len, user.username));

                match scraper_guard.scrape_posts(&user.id, 5).await {
                    Ok(scraped_posts) => {
                        set_bot_status_operational(&mut tx).await;
                        posts.insert(user.clone(), scraped_posts);
                    }
                    Err(e) => {
                        self.println(&format!("Error scraping posts: {}", e));
                        let mut bot_status = tx.load_bot_status().await;
                        bot_status.status = 1;
                        tx.save_bot_status(&bot_status).await;
                        loop {
                            let bot_status = tx.load_bot_status().await;
                            if bot_status.status == 0 {
                                self.println("Retrying to fetch posts...");
                                let result = scraper_guard.scrape_posts(&user.id, 5).await;
                                match result {
                                    Ok(scraped_posts) => {
                                        posts.insert(user.clone(), scraped_posts);
                                        set_bot_status_operational(&mut tx).await;
                                        break;
                                    }
                                    Err(e) => {
                                        self.println(&format!("Error scraping posts: {}", e));
                                        set_bot_status_halted(&mut tx).await;
                                    }
                                }
                            } else {
                                tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
                            }
                        }
                        break;
                    }
                };
            }

            self.randomized_sleep(FETCH_SLEEP_LEN.as_secs()).await;
        }
    }

    async fn scrape_posts(&mut self, accounts_to_scrape: &HashMap<String, String>, hashtag_mapping: &HashMap<String, String>, posts: &mut HashMap<User, Vec<Post>>) {
        let mut transaction = self.database.begin_transaction().await;

        pause_scraper_if_needed(&mut transaction).await;
        let mut rng = StdRng::from_entropy();

        self.println("Scraping posts...");

        let mut flattened_posts: Vec<(User, Post)> = Vec::new();
        for (user, user_posts) in posts {
            for post in user_posts {
                flattened_posts.push((user.clone(), post.clone()));
            }
        }

        flattened_posts.shuffle(&mut rng);

        // remove everything that is not a video
        flattened_posts.retain(|(_, post)| post.is_video);

        let mut flattened_posts_processed = 0;
        let flattened_posts_len = flattened_posts.len();

        let mut actually_scraped = 0;
        for (author, post) in flattened_posts {
            pause_scraper_if_needed(&mut transaction).await;

            flattened_posts_processed += 1;

            if actually_scraped >= MAX_CONTENT_PER_ITERATION {
                self.println("Reached the maximum amount of scraped content per iteration");
                set_bot_status_operational(&mut transaction).await;
                break;
            }

            let base_print = format!("{flattened_posts_processed}/{flattened_posts_len} - {actually_scraped}/{MAX_CONTENT_PER_ITERATION}");

            // Send the URL through the channel
            if post.is_video {
                if !transaction.does_content_exist_with_shortcode(post.shortcode.clone()).await {
                    let filename;
                    let caption;
                    {
                        filename = format!("{}.mp4", post.shortcode);
                        let mut scraper_guard = self.scraper.lock().await;
                        caption = match scraper_guard.download_reel(&post.shortcode, &filename).await {
                            Ok(caption) => {
                                actually_scraped += 1;
                                let base_print = format!("{flattened_posts_processed}/{flattened_posts_len} - {actually_scraped}/{MAX_CONTENT_PER_ITERATION}");
                                self.println(&format!("{base_print} Scraped content from {}: {}", author.username, post.shortcode));
                                set_bot_status_operational(&mut transaction).await;
                                caption
                            }
                            Err(e) => {
                                self.println(&format!("Error while downloading reel | {}", e));

                                match e {
                                    InstagramScraperError::MediaNotFound { .. } => continue,
                                    InstagramScraperError::RateLimitExceeded { .. } => break,
                                    _ => {
                                        set_bot_status_halted(&mut transaction).await;
                                        loop {
                                            let bot_status = transaction.load_bot_status().await;
                                            if bot_status.status == 0 {
                                                self.println("Retrying to download reel...");
                                                let result = scraper_guard.download_reel(&post.shortcode, &filename).await;
                                                match result {
                                                    Ok(caption) => {
                                                        actually_scraped += 1;
                                                        let base_print = format!("{flattened_posts_processed}/{flattened_posts_len} - {actually_scraped}/{MAX_CONTENT_PER_ITERATION}");
                                                        self.println(&format!("{base_print} Scraped content from {}: {}", author.username, post.shortcode));
                                                        set_bot_status_operational(&mut transaction).await;
                                                        break caption;
                                                    }
                                                    Err(e) => {
                                                        self.println(&format!("Error while downloading reel | {}", e));
                                                        set_bot_status_halted(&mut transaction).await;
                                                    }
                                                }
                                            } else {
                                                tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
                                            }
                                        }
                                    }
                                }
                            }
                        };

                        let cookie_store = Arc::clone(&scraper_guard.session.cookie_store);
                        save_cookie_store_to_json(&self.cookie_store_path, cookie_store).await;
                    }

                    let caption = process_caption(accounts_to_scrape, hashtag_mapping, &mut rng, &author, caption);

                    // Use a scoped block to immediately drop the lock
                    {
                        // Store the new URL in the shared variable
                        let mut lock = self.latest_content_mutex.lock().await;
                        //println!("Storing URL: {}", url);
                        *lock = Some((filename, caption, author.username.clone(), post.shortcode.clone()));
                    }
                } else {
                    let existing_content_shortcodes: Vec<String> = transaction.load_content_mapping().await.iter().map(|content_info| content_info.original_shortcode.clone()).collect();
                    let existing_posted_shortcodes: Vec<String> = transaction.load_posted_content().await.iter().map(|existing_posted| existing_posted.original_shortcode.clone()).collect();
                    let existing_failed_shortcodes: Vec<String> = transaction.load_failed_content().await.iter().map(|existing_posted| existing_posted.original_shortcode.clone()).collect();
                    let existing_rejected_shortcodes: Vec<String> = transaction.load_rejected_content().await.iter().map(|existing_posted| existing_posted.original_shortcode.clone()).collect();
                    let existing_duplicate_shortcodes: Vec<String> = transaction.load_duplicate_content().await.iter().map(|existing_posted| existing_posted.original_shortcode.clone()).collect();

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
                            } else if existing_duplicate_shortcodes.contains(&post.shortcode) {
                                self.println(&format!("{base_print} Content already scraped (dupe): {}", post.shortcode));
                            } else {
                                let error_message = format!("{base_print} Content not found in any mapping: {}", post.shortcode);
                                tracing::error!(error_message);
                                panic!("{}", error_message);
                            }
                        }
                    };
                }
                self.randomized_sleep(SCRAPER_DOWNLOAD_SLEEP_LEN.as_secs()).await;
            } else {
                self.println(&format!("{base_print} Content is not a video: {}", post.shortcode));
            }
        }
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

    pub(crate) fn println(&self, message: &str) {
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
