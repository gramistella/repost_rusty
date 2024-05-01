use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use instagram_scraper_rs::{InstagramScraper, InstagramScraperError, Post, User};
use rand::{Rng, SeedableRng};
use rand::prelude::SliceRandom;
use rand::rngs::{OsRng, StdRng};
use serenity::all::MessageId;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::Instrument;

use crate::{FETCH_SLEEP_LEN, MAX_CONTENT_PER_ITERATION, SCRAPER_DOWNLOAD_SLEEP_LEN, SCRAPER_LOOP_SLEEP_LEN};
use crate::database::{ContentInfo, Database, DatabaseTransaction, FailedContent, PublishedContent, QueuedContent};
use crate::discord_bot::bot::{INTERFACE_UPDATE_INTERVAL, REFRESH_RATE};
use crate::discord_bot::state::ContentStatus;
use crate::discord_bot::utils::now_in_my_timezone;
use crate::s3::s3_helper::upload_to_s3;
use crate::video_processing::video_processing::process_video;

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
        let username = self.username.clone();
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

                    let bot_status = transaction.load_bot_status().unwrap();

                    if bot_status.status != 0 {
                        tokio::time::sleep(REFRESH_RATE).await;
                        continue;
                    }

                    if let Some((video_file_name, caption, author, shortcode)) = content_tuple {
                        if !transaction.does_content_exist_with_shortcode(shortcode.clone()) && shortcode != "halted" {
                            // Process video to check if it already exists
                            let video_exists = process_video(&mut transaction, &video_file_name).await.unwrap();

                            if video_exists {
                                println!("The same video is already in the database with a different shortcode, skipping");
                                continue;
                            }

                            let s3_filename = format!("{}/{}", username, video_file_name);
                            let url = upload_to_s3(video_file_name, s3_filename, true).await.unwrap();
                            //println!("Uploaded video to S3: {}", url);

                            // https://repostrusty.s3.eu-north-1.amazonaws.com/repostrusty/l87JT2wtDw8l8rJT2wtDw8l8rJT2wsLw8l8rJT24sLw8.mp4?response-content-disposition=inline&X-Amz-Security-Token=IQoJb3JpZ2luX2VjEDkaCmV1LW5vcnRoLTEiSDBGAiEAoisNDhlUd45arOMYG58kGHfWajpfkfXoGUR7QoSYGxMCIQDUrRFzx%2BzFTs%2F1QpOYEmVEscC%2BkLcX9EXGX%2BPiAbJK2irtAgiC%2F%2F%2F%2F%2F%2F%2F%2F%2F%2F8BEAAaDDU5MDE4MzY1MzAzNyIMDaI7c6TWpw%2FK2hUgKsECKJGDTT2I6ienaogLn9rDW%2FW%2Fsfaan9Dlcf7ppwzcr2xR1XbcBQV5LpNnJBn7rVQVpuxPxzjqUIKBK1zelw3OgPCi0PVmrdv01uoaubPUL2bB3bUlMDzj%2F84uYTkUCOiktD8Z5Pp1TCCOWiXr3kCunJxnhXeUqA%2BnZh3pASqTsXGCX8VykiGhxEkHr%2BUtN%2FxkNtjNNq1x5N6dMouDp2wUgWW6iQ%2FbhUAtZc3qESLkmm%2F5u8nvyG2XwK%2Fx%2BP9K%2Bxs%2BiNqvyX0GqZoZxA%2FsM%2BdhWO6Mc27o6qenzzlcdxnKknG8PoHungl3tCBug3jQosk91YuoHq5qGTifFdnVkFvk%2BzY6fWErPVwk1twnZfbUKpndgPbqS7%2F%2FFUh%2BxoO0eBQTAPn8bnfr6xO9XPZxgJBsiSr3QJ8Pq4bQGLawt%2B21NpHcMMa4obEGOrICv8ZFDEh%2BjCIrKtaZ1HdqpljtfhGz0z3qz1Gm7y8PiTq%2BagF%2BWvGDk8GulTAxy%2F4CUxA5gEKWOdXyc8GHw5RHOxwvhq0QMhLveIrdCe1owdByMGSp9xhj3aWky9USwtsPi0Wv9UxdrVGNWnzUWxUXIAYjx7e298D9eFIKFQ2P%2FkdwYklMCBetHC%2F7jerHIHbVLAK4t3R%2BVy8SPc3KezQN3Qpuomv70%2Fphn8%2FG%2B14gJ1zX730fJOgOHBhUPfJGIXBGotXqKejLFRdTZoovSUHIkWMM7yc9d%2BAmFh606aYZttrwa73v4NkvdzPeaqzynS94nPzkqMW%2FhX0qwMVzFLUWcgt06aiJ8KwYZ3r80QaT9gyxsH7jDGXYx6v9sGDuEjLWEYCX%2BPI12GigFHmEd3TNB5RW&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Date=20240424T014309Z&X-Amz-SignedHeaders=host&X-Amz-Expires=300&X-Amz-Credential=ASIAYS2NP32W6KBMYG53%2F20240424%2Feu-north-1%2Fs3%2Faws4_request&X-Amz-Signature=4eb4188047093c768d97ef242328307843f7a4551f1791cbc9c5a63b328b75da
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

                loop {
                    let mut posts: HashMap<User, Vec<Post>> = HashMap::new();
                    cloned_self.fetch_posts(&accounts_being_scraped, &mut posts).await;

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

        // remove everything that is not a video
        flattened_posts.retain(|(_, post)| post.is_video);

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
                                Self::set_bot_status_operational(&mut transaction);
                                caption
                            }
                            Err(e) => {
                                self.println(&format!("Error while downloading reel | {}", e));

                                match e {
                                    InstagramScraperError::MediaNotFound { .. } => continue,
                                    _ => {
                                        Self::set_bot_status_halted(&mut transaction);
                                        loop {
                                            let bot_status = transaction.load_bot_status().unwrap();
                                            if bot_status.status == 0 {
                                                self.println("Reattempting to download reel...");
                                                let result = scraper_guard.download_reel(&post.shortcode, &filename).await;
                                                match result {
                                                    Ok(caption) => {
                                                        actually_scraped += 1;
                                                        let base_print = format!("{flattened_posts_processed}/{flattened_posts_len} - {actually_scraped}/{MAX_CONTENT_PER_ITERATION}");
                                                        self.println(&format!("{base_print} Scraped content from {}: {}", author.username, post.shortcode));
                                                        Self::set_bot_status_operational(&mut transaction);
                                                        break caption;
                                                    }
                                                    Err(e) => {
                                                        self.println(&format!("Error while downloading reel | {}", e));
                                                        Self::set_bot_status_halted(&mut transaction);
                                                    }
                                                }
                                            } else {
                                                tokio::time::sleep(REFRESH_RATE).await;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let caption = Self::process_caption(accounts_to_scrape, hashtag_mapping, &mut rng, &author, caption);

                    // Use a scoped block to immediately drop the lock
                    {
                        // Store the new URL in the shared variable
                        let mut lock = self.latest_content_mutex.lock().await;
                        //println!("Storing URL: {}", url);
                        *lock = Some((filename, caption, author.username.clone(), post.shortcode.clone()));
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
                self.randomized_sleep(SCRAPER_DOWNLOAD_SLEEP_LEN.as_secs()).await;
            } else {
                self.println(&format!("{base_print} Content is not a video: {}", post.shortcode));
            }
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
                        let mut tx = self.database.begin_transaction().await.unwrap();
                        let mut bot_status = tx.load_bot_status().unwrap();
                        bot_status.status = 1;
                        tx.save_bot_status(&bot_status).unwrap();
                        loop {
                            let bot_status = tx.load_bot_status().unwrap();
                            if bot_status.status == 0 {
                                self.println("Reattempting to fetch posts...");
                                let result = scraper_guard.scrape_posts(&user.id, 5).await;
                                match result {
                                    Ok(scraped_posts) => {
                                        posts.insert(user.clone(), scraped_posts);
                                        Self::set_bot_status_operational(&mut tx);
                                        break;
                                    }
                                    Err(e) => {
                                        self.println(&format!("Error scraping posts: {}", e));
                                        Self::set_bot_status_halted(&mut tx);
                                    }
                                }
                            } else {
                                tokio::time::sleep(REFRESH_RATE).await;
                            }
                        }
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
                let mut tx = self.database.begin_transaction().await.unwrap();
                match result {
                    Ok(user) => {
                        accounts_being_scraped.push(user);
                        self.println(&format!("{}/{} Fetched user info for {}", accounts_scraped, accounts_to_scrape_len, profile));
                        Self::set_bot_status_operational(&mut tx);
                    }
                    Err(e) => {
                        self.println(&format!("{}/{} Error fetching user info for {}: {}", accounts_scraped, accounts_to_scrape_len, profile, e));
                        match e {
                            InstagramScraperError::UserNotFound(profile) => {
                                accounts_to_scrape.remove(&profile);
                            }
                            _ => {
                                Self::set_bot_status_halted(&mut tx);
                                loop {
                                    let bot_status = tx.load_bot_status().unwrap();
                                    if bot_status.status == 0 {
                                        self.println("Reattempting to fetch user info...");
                                        let result = scraper_guard.scrape_userinfo(&profile).await;
                                        match result {
                                            Ok(user) => {
                                                accounts_being_scraped.push(user);
                                                self.println(&format!("{}/{} Fetched user info for {}", accounts_scraped, accounts_to_scrape_len, profile));
                                                Self::set_bot_status_operational(&mut tx);
                                                break;
                                            }
                                            Err(e) => {
                                                self.println(&format!("{}/{} Error fetching user info for {}: {}", accounts_scraped, accounts_to_scrape_len, profile, e));
                                                Self::set_bot_status_halted(&mut tx);
                                            }
                                        }
                                    } else {
                                        tokio::time::sleep(REFRESH_RATE).await;
                                    }
                                }
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
                    let mut tx = self.database.begin_transaction().await.unwrap();
                    Self::set_bot_status_halted(&mut tx);

                    loop {
                        let bot_status = tx.load_bot_status().unwrap();
                        if bot_status.status == 0 {
                            self.println("Reattempting to log in...");
                            scraper_guard.authenticate_with_login(username.clone(), password.clone());
                            let result = scraper_guard.login().await;
                            match result {
                                Ok(_) => {
                                    self.println("Logged in successfully");
                                    Self::set_bot_status_operational(&mut tx);
                                    break;
                                }
                                Err(e) => {
                                    self.println(&format!(" Login failed: {}", e));
                                    Self::set_bot_status_halted(&mut tx);
                                }
                            }
                        } else {
                            tokio::time::sleep(REFRESH_RATE).await;
                        }
                    }
                }
            };
        }
        self.save_cookie_store_to_json().await;
    }

    fn set_bot_status_halted(tx: &mut DatabaseTransaction) {
        let mut bot_status = tx.load_bot_status().unwrap();
        bot_status.status = 1;
        bot_status.status_message = "halted  ⚠️".to_string();
        bot_status.last_updated_at = (now_in_my_timezone(&tx.load_user_settings().unwrap()) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
        tx.save_bot_status(&bot_status).unwrap();
    }

    fn set_bot_status_operational(tx: &mut DatabaseTransaction) {
        let mut bot_status = tx.load_bot_status().unwrap();
        bot_status.status = 0;
        bot_status.status_message = "operational  🟢".to_string();
        bot_status.last_updated_at = (now_in_my_timezone(&tx.load_user_settings().unwrap()) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
        tx.save_bot_status(&bot_status).unwrap();
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
                                                        drop(scraper_guard);
                                                        if err.to_string().contains("The app user's Instagram Professional account is inactive, checkpointed, or restricted.") {
                                                            cloned_self.println(&format!("[!] Couldn't upload content to instagram! The app user's Instagram Professional account is inactive, checkpointed, or restricted."));
                                                            Self::set_bot_status_halted(&mut transaction);
                                                            loop {
                                                                let bot_status = transaction.load_bot_status().unwrap();
                                                                if bot_status.status == 0 {
                                                                    cloned_self.println("Reattempting to upload content to instagram...");
                                                                    let result = cloned_self.scraper.lock().await.upload_reel(user_id, access_token, &queued_post.url, &full_caption).await;
                                                                    match result {
                                                                        Ok(_) => {
                                                                            cloned_self.println(&format!("[+] Published content successfully: {}", queued_post.original_shortcode));
                                                                            Self::set_bot_status_operational(&mut transaction);
                                                                            break;
                                                                        }
                                                                        Err(_e) => {
                                                                            cloned_self.println(&format!("[!] Couldn't upload content to instagram! The app user's Instagram Professional account is inactive, checkpointed, or restricted."));
                                                                            Self::set_bot_status_halted(&mut transaction);
                                                                        }
                                                                    }
                                                                } else {
                                                                    tokio::time::sleep(REFRESH_RATE).await;
                                                                }
                                                            }
                                                        } else {
                                                            cloned_self.println(&format!("[!] Couldn't upload content to instagram! Trying again later\n [WARNING] {}", err.to_string()));
                                                            cloned_self.handle_recoverable_failed_content().await;
                                                        }
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
