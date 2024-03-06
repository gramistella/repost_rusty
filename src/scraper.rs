use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::DateTime;
use indexmap::IndexMap;
use instagram_scraper_rs::{InstagramScraper, Post, User};
use rand::prelude::SliceRandom;
use rand::rngs::{OsRng, StdRng};
use rand::{Rng, SeedableRng};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::database::{Database, FailedContent, PostedContent, QueuedContent};
use crate::utils::now_in_my_timezone;
use crate::REFRESH_RATE;

async fn read_accounts_to_scrape(path: &str, username: &str) -> HashMap<String, String> {
    let mut file = File::open(path).await.expect("Unable to open credentials file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).await.expect("Unable to read the credentials file");
    let accounts: HashMap<String, HashMap<String, String>> = serde_yaml::from_str(&contents).expect("Error parsing credentials file");
    accounts.get(username).unwrap().clone()
}

pub async fn run_scraper(tx: Sender<(String, String, String, String)>, database: Database, is_offline: bool, credentials: HashMap<String, String>) -> anyhow::Result<()> {
    let testing_urls = vec![
        "https://scontent-mxp2-1.cdninstagram.com/v/t50.2886-16/427593823_409517391528628_721852697655626393_n.mp4?_nc_ht=scontent-mxp2-1.cdninstagram.com&_nc_cat=104&_nc_ohc=9pssChekERcAX_XayYY&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfAXS0ConI008uTXkgc-woujGN6BchRo_ofWZkPVrg1JfQ&oe=65D574C7&_nc_sid=2999b8",
        "https://scontent-mxp1-1.cdninstagram.com/v/t50.2886-16/429215690_1444115769649034_2337419377310423138_n.mp4?_nc_ht=scontent-mxp1-1.cdninstagram.com&_nc_cat=102&_nc_ohc=jEzG6M_uCAQAX8oTbjC&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfCGYsCaoUB8qOYTSJJFJZbLMuKCbGZqfXH9ydMu9jKhxQ&oe=65D5D2CE&_nc_sid=2999b8",
        "https://scontent-mxp1-1.cdninstagram.com/v/t66.30100-16/48718206_1450879249116459_8164759842261415987_n.mp4?_nc_ht=scontent-mxp1-1.cdninstagram.com&_nc_cat=103&_nc_ohc=GUN_mDr2OYsAX9fRqgF&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfC78iGTuJdhnNbhO_fuieXvYT5R3pkhMAD-MLNxtxR7TQ&oe=65D572F7&_nc_sid=2999b8",
        "https://scontent-mxp2-1.cdninstagram.com/v/t50.2886-16/428456788_295092509963149_5948637286561662383_n.mp4?_nc_ht=scontent-mxp2-1.cdninstagram.com&_nc_cat=101&_nc_ohc=zkatv7OeKwUAX8YNkBr&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfAQIp78Fu1NCj719FprGjWMV_gZ3s_b8Ux_mWy3Ek1uOA&oe=65D73480&_nc_sid=2999b8",
        "https://scontent-mxp2-1.cdninstagram.com/v/t50.2886-16/428478842_735629148752808_8195281140553080552_n.mp4?_nc_ht=scontent-mxp2-1.cdninstagram.com&_nc_cat=100&_nc_ohc=C_p4c8oZuQAAX-v2g55&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfDFvSm0eBmcCsuO3rFKcdLFdi6HBHTKzAkN8tqdAoUn_w&oe=65D69DF3&_nc_sid=2999b8",
        //"https://scontent-mxp1-1.cdninstagram.com/v/t50.2886-16/428226521_1419744418741015_2882822033667053284_n.mp4?_nc_ht=scontent-mxp1-1.cdninstagram.com&_nc_cat=107&_nc_ohc=Dir0Fhly2oQAX-WOLlD&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfCdg_GUWzIIA6ueap7WZgK1zG1Zq903lp_RxGFMT22xWA&oe=65D6A70B&_nc_sid=2999b8",
        //"https://scontent-mxp1-1.cdninstagram.com/v/t66.30100-16/121970702_1790725214757830_3319359828076371228_n.mp4?_nc_ht=scontent-mxp1-1.cdninstagram.com&_nc_cat=102&_nc_ohc=sdXOm_-HZdYAX8rM2fF&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfCM70VvPF38qW8nyUmlObryDhI643vN5WHjvDqc3NRbcA&oe=65D6EF9B&_nc_sid=2999b8",
        //"https://scontent-mxp1-1.cdninstagram.com/v/t66.30100-16/121970702_1790725214757830_3319359828076371228_n.mp4?_nc_ht=scontent-mxp1-1.cdninstagram.com&_nc_cat=102&_nc_ohc=sdXOm_-HZdYAX8rM2fF&edm=AP_V10EBAAAA&ccb=7-5&oh=00_AfCM70VvPF38qW8nyUmlObryDhI643vN5WHjvDqc3NRbcA&oe=65D6EF9B&_nc_sid=2999b8",
    ];

    let loop_sleep_len = Duration::from_secs(60 * 90);
    let download_sleep_len = Duration::from_secs(60 * 5);
    //let mut unique_post_ids = HashSet::new();

    let scraper_loop: Option<tokio::task::JoinHandle<anyhow::Result<()>>>;
    let scraper = Arc::new(Mutex::new(InstagramScraper::default()));
    let scraper_clone = Arc::clone(&scraper);

    let username = credentials.get("username").unwrap().clone();
    let password = credentials.get("password").unwrap().clone();

    let accounts_to_scrape: HashMap<String, String> = read_accounts_to_scrape("config/accounts_to_scrape.yaml", username.as_str()).await;

    if is_offline {
        println!("Sending offline data");

        scraper_loop = Some(tokio::spawn(async move {
            let mut loop_iterations = 0;
            loop {
                loop_iterations += 1;
                let mut inner_loop_iterations = 0;
                for url in &testing_urls {
                    inner_loop_iterations += 1;
                    let caption_string = format!("Video {}, loop {}", inner_loop_iterations, loop_iterations);
                    tx.send((url.to_string(), caption_string, "local".to_string(), format!("shortcode{}", inner_loop_iterations))).await.unwrap();
                    sleep(Duration::from_secs(3)).await;
                }
            }
        }));
    } else {
        let scraper_loop_database = database.clone();
        scraper_loop = Some(tokio::spawn(async move {
            let cloned_scraper = Arc::clone(&scraper_clone);

            let mut rng = StdRng::from_entropy();

            // Create a shared variable for the latest URL
            let latest_url = Arc::new(Mutex::new(None));

            // Create a clone of the shared variable for the separate task
            let latest_url_for_task = Arc::clone(&latest_url);

            // Create a separate task that sends the URL through the channel
            tokio::spawn(async move {
                loop {
                    let url = {
                        let lock = latest_url_for_task.lock().await;
                        lock.clone()
                    };

                    if let Some((url, caption, author, shortcode)) = url {
                        tx.send((url, caption, author, shortcode)).await.unwrap();
                    } else {
                        tx.send(("".to_string(), "".to_string(), "".to_string(), "C4EXJ4NpQz3".to_string())).await.unwrap();
                    }

                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            });

            {
                let mut scraper_guard = cloned_scraper.lock().await;

                scraper_guard.authenticate_with_login(username.clone(), password);

                println!("Logging in {}...", username.clone());
                match scraper_guard.login().await {
                    Ok(_) => {
                        println!("Logged in {} successfully", username);
                    }
                    Err(e) => {
                        println!("Login failed: {}", e);
                        return Err(anyhow::anyhow!("login failed: {}", e));
                    }
                }
            }

            println!("Fetching user info...");
            let mut accounts_being_scraped = Vec::new();

            for (profile, _hashtags) in &accounts_to_scrape {
                {
                    let mut scraper_guard = cloned_scraper.lock().await;
                    let user = scraper_guard.scrape_userinfo(&profile).await.unwrap();
                    accounts_being_scraped.push(user);
                    println!("Fetched user info for {}", profile);
                }
                let sleep_duration = 20;
                randomized_sleep(sleep_duration).await;
            }

            let inner_scraper_loop_database = scraper_loop_database.clone();
            loop {
                let mut transaction = inner_scraper_loop_database.begin_transaction().unwrap();

                //let random_profiles: Vec<_> = accounts_being_scraped.choose_multiple(&mut rng, 3).collect();
                //println!("Running scraper loop, selected users {}, {}, {}", random_profiles[0].username, random_profiles[1].username, random_profiles[2].username);
                // get user info

                let mut posts: HashMap<User, Vec<Post>> = HashMap::new();
                let mut accounts_scraped = 0;
                let accounts_being_scraped_len = accounts_being_scraped.len();

                for user in accounts_being_scraped.iter() {
                    accounts_scraped += 1;
                    println!("{}/{} Retrieving posts from user {}", accounts_scraped, accounts_being_scraped_len, user.username);

                    // get posts
                    {
                        let mut scraper_guard = cloned_scraper.lock().await;
                        scraper_guard.scrape_posts(&user.id, 5).await.unwrap();
                        posts.insert(user.clone(), scraper_guard.scrape_posts(&user.id, 5).await.unwrap());
                    }

                    randomized_sleep(30).await;
                }

                let mut flattened_posts: Vec<(User, Post)> = Vec::new();
                for (user, user_posts) in &posts {
                    for post in user_posts {
                        flattened_posts.push((user.clone(), post.clone()));
                    }
                }

                flattened_posts.shuffle(&mut rng);

                let mut flattened_posts_processed = 0;
                let flattened_posts_len = flattened_posts.len();

                for (author, post) in flattened_posts {
                    flattened_posts_processed += 1;

                    // Send the URL through the channel
                    if post.is_video {
                        if transaction.does_content_exist_with_shortcode(post.shortcode.clone()) == false {
                            println!("{}/{} Scraping content: {}", flattened_posts_processed, flattened_posts_len, post.shortcode);
                            let mut scraper_guard = cloned_scraper.lock().await;
                            let (url, caption) = scraper_guard.download_reel(&post.shortcode).await.unwrap();

                            // Check if the caption contains any hashtags
                            let mut hashtags = caption.split_whitespace().filter(|s| s.starts_with('#')).collect::<Vec<&str>>().join(" ");
                            let selected_hashtags;
                            if hashtags.len() > 0 {
                                selected_hashtags = hashtags;
                            } else {
                                hashtags = accounts_to_scrape.get(&author.username.clone()).unwrap().clone();
                                // Convert hashtag string from "#hastag, #hashtag2" to vec, and then pick 3 random hashtags
                                // Split the string into a vector and trim each element
                                let mut hashtags: Vec<&str> = hashtags.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

                                // Shuffle the vector and select the first three elements

                                hashtags.shuffle(&mut rng);
                                let random_hashtags = &hashtags[0..3.min(hashtags.len())];

                                // Join the selected hashtags into a single string
                                selected_hashtags = random_hashtags.join(" ");
                            }

                            // Remove the hashtags from the caption
                            let caption = caption.split_whitespace().filter(|s| !s.starts_with('#')).collect::<Vec<&str>>().join(" ");
                            // Rebuild the caption
                            let caption = format!("{} {}", caption, selected_hashtags);
                            // Use a scoped block to immediately drop the lock
                            {
                                // Store the new URL in the shared variable
                                let mut lock = latest_url.lock().await;
                                //println!("Storing URL: {}", url);
                                *lock = Some((url, caption, author.username.clone(), post.shortcode.clone()));
                            }
                        } else {
                            println!("{}/{} Content already scraped: {}", flattened_posts_processed, flattened_posts_len, post.shortcode);
                        }
                    } else {
                        println!("{}/{} Content is not a video: {}", flattened_posts_processed, flattened_posts_len, post.shortcode);
                    }
                    randomized_sleep(download_sleep_len.as_secs()).await;
                }
                // Wait for a while before the next iteration
                println!("Starting long sleep ({} minutes)", loop_sleep_len.as_secs() / 60);
                randomized_sleep(loop_sleep_len.as_secs()).await;
            }

            //debug_account(&profile, &mut scraper).await?;
            /*
            //logout
            scraper
                .logout()
                .await
                .map_err(|e| anyhow::anyhow!("logout failed: {}", e))

             */
        }));
    }

    let poster_loop_database = database.clone();
    let poster_loop = tokio::spawn(async move {
        loop {
            // Load current video mapping
            // Check which video begins with status of "accepted_"

            let mut transaction = poster_loop_database.begin_transaction().unwrap();
            let content_mapping = transaction.load_content_mapping().unwrap();
            let user_settings = transaction.load_user_settings().unwrap();

            //let queued_posts = Vec::new();
            for (message_id, mut content_info) in content_mapping {
                if content_info.status.contains("accepted_") {
                    let last_updated_at = now_in_my_timezone(user_settings.clone()) - REFRESH_RATE;
                    let will_post_at = transaction.get_new_post_time(user_settings.clone()).unwrap();
                    let new_queued_post = QueuedContent {
                        url: content_info.url.clone(),
                        caption: content_info.caption.clone(),
                        hashtags: content_info.hashtags.clone(),
                        original_author: content_info.original_author.clone(),
                        original_shortcode: content_info.original_shortcode.clone(),
                        last_updated_at: last_updated_at.to_rfc3339(),
                        will_post_at,
                    };

                    transaction.save_content_queue(new_queued_post).unwrap();
                    if content_info.status.contains("shown") {
                        content_info.status = "queued_shown".to_string();
                    } else {
                        content_info.status = "queued_hidden".to_string();
                    }
                    let index_map = IndexMap::from([(message_id, content_info.clone())]);
                    transaction.save_content_mapping(index_map).unwrap();
                }
                if content_info.status.contains("queued_") {
                    let queued_posts = transaction.load_content_queue().unwrap();
                    for mut queued_post in queued_posts {
                        if DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap() < now_in_my_timezone(user_settings.clone()) {
                            if user_settings.can_post {
                                let scraper_copy = Arc::clone(&scraper);
                                let mut scraper_guard = scraper_copy.lock().await;
                                if !is_offline {
                                    let full_caption;
                                    let spacer = ".\n.\n.\n.\n.\n.\n.\n";
                                    if queued_post.caption == "" && queued_post.hashtags == "" {
                                        full_caption = "".to_string();
                                    } else if queued_post.caption == "" {
                                        full_caption = format!("{}", queued_post.hashtags);
                                    } else if queued_post.hashtags == "" {
                                        full_caption = format!("{}", queued_post.caption);
                                    } else {
                                        full_caption = format!("{}{}{}", queued_post.caption, spacer, queued_post.hashtags);
                                    }

                                    let user_id = credentials.get("instagram_business_account_id").unwrap();
                                    let access_token = credentials.get("fb_access_token").unwrap();
                                    println!(" [+] Uploading content to instagram: {}", queued_post.original_shortcode);

                                    match scraper_guard.upload_reel(user_id, access_token, &queued_post.url, &full_caption).await {
                                        Ok(_) => {
                                            println!(" [+] Uploaded content successfully: {}", queued_post.original_shortcode);
                                        }
                                        Err(_) => {
                                            println!(" [!] ERROR: couldn't upload content to instagram! {}", queued_post.url);

                                            let (message_id, mut video_info) = transaction.get_content_info_by_shortcode(queued_post.original_shortcode.clone()).unwrap();
                                            if video_info.status.contains("shown") {
                                                video_info.status = "failed_shown".to_string();
                                            } else {
                                                video_info.status = "failed_hidden".to_string();
                                            }

                                            let index_map = IndexMap::from([(message_id, video_info.clone())]);
                                            transaction.save_content_mapping(index_map).unwrap();

                                            let now = now_in_my_timezone(user_settings.clone()).to_rfc3339();
                                            let failed_content = FailedContent {
                                                url: queued_post.url.clone(),
                                                caption: queued_post.caption.clone(),
                                                hashtags: queued_post.hashtags.clone(),
                                                original_author: queued_post.original_author.clone(),
                                                original_shortcode: queued_post.original_shortcode.clone(),
                                                last_updated_at: now.clone(),
                                                failed_at: now,
                                            };

                                            transaction.save_failed_content(failed_content.clone()).unwrap();
                                            continue;
                                        }
                                    }
                                } else {
                                    println!(" [!] Uploaded content offline: {}", queued_post.url);
                                }

                                let (message_id, mut video_info) = transaction.get_content_info_by_shortcode(queued_post.original_shortcode.clone()).unwrap();
                                video_info.status = "posted_hidden".to_string();
                                let index_map = IndexMap::from([(message_id, video_info.clone())]);
                                transaction.save_content_mapping(index_map).unwrap();

                                let posted_content = PostedContent {
                                    url: queued_post.url.clone(),
                                    caption: queued_post.caption.clone(),
                                    hashtags: queued_post.hashtags.clone(),
                                    original_author: queued_post.original_author.clone(),
                                    original_shortcode: queued_post.original_shortcode.clone(),
                                    posted_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
                                    last_updated_at: now_in_my_timezone(user_settings.clone()).to_rfc3339(),
                                    expired: false,
                                };

                                transaction.save_posted_content(posted_content).unwrap();
                                println!(" [+] Saved posted content: {}", queued_post.original_shortcode);
                            } else {
                                let new_will_post_at = transaction.get_new_post_time(user_settings.clone()).unwrap();
                                queued_post.will_post_at = new_will_post_at;
                                transaction.save_content_queue(queued_post.clone()).unwrap();

                                let (message_id, mut video_info) = transaction.get_content_info_by_shortcode(queued_post.original_shortcode.clone()).unwrap();
                                video_info.status = "queued_hidden".to_string();
                                let index_map = IndexMap::from([(message_id, video_info.clone())]);
                                transaction.save_content_mapping(index_map).unwrap();
                            }
                        }
                    }
                }
            }

            sleep(Duration::from_secs(1)).await;
        }
    });

    let _ = tokio::try_join!(scraper_loop.unwrap(), poster_loop);

    Ok(())
}

/// Randomized sleep function, will randomize the sleep duration by up to 20% of the original duration
async fn randomized_sleep(original_duration: u64) {
    let mut rng = StdRng::from_rng(OsRng).unwrap();
    let variance: u64 = rng.gen_range(0..=1); // generates a number between 0 and 1
    let sleep_duration = original_duration + (original_duration * variance / 5); // add up to 20% of the original sleep duration
    sleep(Duration::from_secs(sleep_duration)).await;
}
