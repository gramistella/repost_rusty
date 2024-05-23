use std::time::Duration;

use chrono::{DateTime, Utc};
use instagram_scraper_rs::{InstagramScraper, InstagramScraperError};
use rand::prelude::{SliceRandom, StdRng};
use rand::rngs::OsRng;
use rand::{Rng, SeedableRng};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::database::database::{DatabaseTransaction, FailedContent, PublishedContent, QueuedContent};
use crate::discord::state::ContentStatus;
use crate::discord::utils::now_in_my_timezone;
use crate::scraper_poster::scraper::ContentManager;
use crate::scraper_poster::utils::{set_bot_status_halted, set_bot_status_operational};
use crate::{SCRAPER_REFRESH_RATE};

impl ContentManager {
    pub fn poster_loop(&mut self) -> JoinHandle<anyhow::Result<()>> {
        let span = tracing::span!(tracing::Level::INFO, "poster_loop");
        let _enter = span.enter();
        let cloned_self = self.clone();
        tokio::spawn(async move {
            cloned_self.amend_queue().await;
            // Allow the scraper_poster to login

            let sleep_duration = 90.0; // make this a float so we can add a fractional amount to it
            let mut rng = StdRng::from_rng(OsRng).unwrap();
            let variance: f64 = rng.gen_range(0.0..=1.0); // generates a number between 0 and 1
            let sleep_duration = sleep_duration + (sleep_duration * variance * 0.3); // add up to 30% of the original sleep duration
                                                                                     // Convert sleep_duration to milliseconds
            let sleep_duration_millis = (sleep_duration * 1000.0) as u64;
            // Sleep
            sleep(Duration::from_millis(sleep_duration_millis)).await;

            cloned_self.println("Starting poster loop...");

            loop {
                let mut transaction = cloned_self.database.begin_transaction().await;
                let content_mapping = transaction.load_content_mapping().await;
                let user_settings = transaction.load_user_settings().await;

                let queued_posts = transaction.load_content_queue().await;

                'outer: for content_info in content_mapping {
                    if content_info.status.to_string().contains("queued_") {
                        for queued_post in queued_posts.iter() {
                            if DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap() < now_in_my_timezone(&user_settings) {
                                if user_settings.can_post {
                                    if !cloned_self.is_offline {
                                        let full_caption = Self::prepare_caption_for_post(queued_post);

                                        let user_id = cloned_self.credentials.get("instagram_business_account_id").unwrap();
                                        let access_token = cloned_self.credentials.get("fb_access_token").unwrap();

                                        // We want to lock the scraper for the entire duration of the publishing process
                                        let mut scraper_guard = cloned_self.scraper.lock().await;

                                        // Publish the content
                                        let reel_id = match cloned_self.publish_content(&mut scraper_guard, &mut transaction, queued_post, &full_caption, user_id, access_token).await {
                                            Some(value) => value,
                                            None => continue,
                                        };

                                        // Try to comment on the post
                                        cloned_self.comment_on_published_content(&mut scraper_guard, access_token, &reel_id).await;
                                    } else if queued_post.caption.contains("will_fail") {
                                        cloned_self.println(&format!("[!] Failed to upload content offline: {}", queued_post.url));
                                        cloned_self.handle_failed_content(queued_post).await;
                                        continue;
                                    } else {
                                        cloned_self.println(&format!("[!] Uploaded content offline: {}", queued_post.url));
                                    }

                                    let mut content_info = transaction.get_content_info_by_shortcode(&queued_post.original_shortcode).await;
                                    content_info.status = ContentStatus::Published { shown: false };

                                    transaction.save_content_info(&content_info).await;

                                    let published_content = PublishedContent {
                                        username: queued_post.username.clone(),
                                        url: queued_post.url.clone(),
                                        caption: queued_post.caption.clone(),
                                        hashtags: queued_post.hashtags.clone(),
                                        original_author: queued_post.original_author.clone(),
                                        original_shortcode: queued_post.original_shortcode.clone(),
                                        published_at: now_in_my_timezone(&user_settings).to_rfc3339(),
                                    };

                                    transaction.save_published_content(published_content).await;
                                } else {
                                    for content in queued_posts.clone().iter_mut() {
                                        content.will_post_at = (DateTime::parse_from_rfc3339(&content.will_post_at).unwrap() + Duration::from_secs((user_settings.posting_interval * 60) as u64)).to_rfc3339();
                                        transaction.save_queued_content(queued_post).await;
                                        let mut content_info = transaction.get_content_info_by_shortcode(&queued_post.original_shortcode).await;
                                        content_info.last_updated_at = (now_in_my_timezone(&user_settings) - chrono::Duration::milliseconds(user_settings.interface_update_interval)).to_rfc3339();
                                        transaction.save_content_info(&content_info).await;
                                    }
                                    // Since we have just altered the whole queue, and we are also iterating over the queue in the outer loop, we need to break here
                                }
                                // Just break, we need to post just once per iteration anyway
                                break 'outer;
                            }
                        }
                    }
                }
                // Don't remove this sleep, without it the bot becomes completely unresponsive
                sleep(SCRAPER_REFRESH_RATE).await;
            }
        })
    }

    async fn comment_on_published_content(&self, scraper: &mut InstagramScraper, access_token: &str, reel_id: &str) {
        let mut comment_vec = vec![];
        match self.username.as_str() {
            "repostrusty" => {
                let comment_caption_1 = format!("Follow @{} for daily dank memes 😤", self.username);
                let comment_caption_2 = format!("Follow @{} for daily memes, I won't disappoint 😎", self.username);
                let comment_caption_3 = format!("Follow @{} for your daily meme fix 🗿", self.username);
                comment_vec.push(comment_caption_1);
                comment_vec.push(comment_caption_2);
                comment_vec.push(comment_caption_3);
            }
            "cringepostrusty" => {
                let comment_caption_1 = format!("Follow @{} for daily cringe 😤", self.username);
                let comment_caption_2 = format!("Follow @{} for daily cringe, I won't disappoint 😎", self.username);
                let comment_caption_3 = format!("Follow @{} for your daily cringe fix 🗿", self.username);
                comment_vec.push(comment_caption_1);
                comment_vec.push(comment_caption_2);
                comment_vec.push(comment_caption_3);
            }
            "rusty_cat_memes" => {
                let comment_caption_1 = format!("Follow @{} for daily cat memes ⸜(｡˃ ᵕ ˂ )⸝♡", self.username);
                let comment_caption_2 = format!("Follow @{} for daily cat memes, I won't disappoint ദ്ദി(˵ •̀ ᴗ - ˵ ) ✧", self.username);
                let comment_caption_3 = format!("Follow @{} for your daily cat meme needs (˶ᵔ ᵕ ᵔ˶)", self.username);
                comment_vec.push(comment_caption_1);
                comment_vec.push(comment_caption_2);
                comment_vec.push(comment_caption_3);
            }
            _ => {}
        }

        // Choose a random comment
        let mut rng = StdRng::from_entropy();
        let comment_caption = comment_vec.choose(&mut rng).unwrap();
        match scraper.comment(reel_id, access_token, comment_caption).await {
            Ok(_) => {
                self.println("Commented on the post successfully!");
            }
            Err(e) => {
                let e = format!("{}", e);
                self.println(&format!("Error while commenting: {}", e));
            }
        }
    }

    async fn publish_content(&self, scraper: &mut InstagramScraper, transaction: &mut DatabaseTransaction, queued_post: &QueuedContent, full_caption: &str, user_id: &str, access_token: &str) -> Option<String> {
        self.println(&format!("[+] Publishing content to instagram: {}", queued_post.original_shortcode));
        let timer = std::time::Instant::now();
        let reel_id = match scraper.upload_reel(user_id, access_token, &queued_post.url, full_caption).await {
            Ok(reel_id) => {
                let duration = timer.elapsed(); // End timer

                let minutes = duration.as_secs() / 60;
                let seconds = duration.as_secs() % 60;

                self.println(&format!("[+] Published content successfully: {}, took {} minutes and {} seconds", queued_post.original_shortcode, minutes, seconds));
                reel_id
            }
            Err(err) => {
                match err {
                    InstagramScraperError::UploadFailedRecoverable(_) => {
                        if err.to_string().contains("The app user's Instagram Professional account is inactive, checkpointed, or restricted.") {
                            self.println("[!] Couldn't upload content to instagram! The app user's Instagram Professional account is inactive, checkpointed, or restricted.");
                            set_bot_status_halted(transaction).await;
                            loop {
                                let bot_status = transaction.load_bot_status().await;
                                if bot_status.status == 0 {
                                    self.println("Retrying to upload content to instagram...");
                                    let result = self.scraper.lock().await.upload_reel(user_id, access_token, &queued_post.url, full_caption).await;
                                    match result {
                                        Ok(_) => {
                                            self.println(&format!("[+] Published content successfully: {}", queued_post.original_shortcode));
                                            set_bot_status_operational(transaction).await;
                                            break;
                                        }
                                        Err(_e) => {
                                            self.println("[!] Couldn't upload content to instagram! The app user's Instagram Professional account is inactive, checkpointed, or restricted.");
                                            set_bot_status_halted(transaction).await;
                                        }
                                    }
                                } else {
                                    tokio::time::sleep(SCRAPER_REFRESH_RATE).await;
                                }
                            }
                        } else {
                            self.println(&format!("[!] Couldn't upload content to instagram! Trying again later\n [WARNING] {}", err));
                            self.handle_recoverable_failed_content().await;
                        }
                    }
                    InstagramScraperError::UploadFailedNonRecoverable(_) => {
                        self.println(&format!("[!] Couldn't upload content to instagram!\n [ERROR] {}\n{}", err, queued_post.url));

                        self.handle_failed_content(queued_post).await;
                    }
                    InstagramScraperError::UploadSucceededButFailedToRetrieveId(e) => {
                        self.println(&format!("[!] Uploaded content to instagram, but failed to retrieve media id!\n [WARNING] {}\n{}", e, queued_post.url));
                        self.handle_posted_but_failed_content(queued_post).await;
                    }
                    _ => {}
                }
                return None;
            }
        };
        Some(reel_id)
    }

    fn prepare_caption_for_post(queued_post: &QueuedContent) -> String {
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
        if queued_post.caption.is_empty() && queued_post.hashtags.is_empty() {
            full_caption = "".to_string();
        } else if queued_post.caption.is_empty() {
            full_caption = format!("{}", queued_post.hashtags);
        } else if queued_post.hashtags.is_empty() {
            full_caption = format!("{}", queued_post.caption);
        } else {
            full_caption = format!("{}{}{}{}{}", queued_post.caption, big_spacer, disclaimer, small_spacer, queued_post.hashtags);
        }
        full_caption
    }

    async fn handle_failed_content(&self, queued_post: &QueuedContent) {
        let span = tracing::span!(tracing::Level::INFO, "handle_failed_content");
        let _enter = span.enter();

        let mut transaction = self.database.begin_transaction().await;
        let user_settings = transaction.load_user_settings().await;
        let mut video_info = transaction.get_content_info_by_shortcode(&queued_post.original_shortcode).await;
        video_info.status = ContentStatus::Failed { shown: false };

        transaction.save_content_info(&video_info).await;

        let now = now_in_my_timezone(&user_settings).to_rfc3339();
        let failed_content = FailedContent {
            username: queued_post.username.clone(),
            url: queued_post.url.clone(),
            caption: queued_post.caption.clone(),
            hashtags: queued_post.hashtags.clone(),
            original_author: queued_post.original_author.clone(),
            original_shortcode: queued_post.original_shortcode.clone(),
            failed_at: now,
        };

        transaction.save_failed_content(failed_content).await;
    }

    async fn handle_recoverable_failed_content(&self) {
        let span = tracing::span!(tracing::Level::INFO, "handle_recoverable_failed_content");
        let _enter = span.enter();

        let mut transaction = self.database.begin_transaction().await;
        let user_settings = transaction.load_user_settings().await;

        for mut queued_post in transaction.load_content_queue().await {
            let new_will_post_at = DateTime::parse_from_rfc3339(&queued_post.will_post_at).unwrap() + Duration::from_secs((user_settings.posting_interval * 60) as u64);
            queued_post.will_post_at = new_will_post_at.to_rfc3339();
            transaction.save_queued_content(&queued_post).await;
        }
    }

    async fn handle_posted_but_failed_content(&self, queued_post: &QueuedContent) {
        let span = tracing::span!(tracing::Level::INFO, "handle_posted_but_failed_content");
        let _enter = span.enter();

        let mut transaction = self.database.begin_transaction().await;
        let user_settings = transaction.load_user_settings().await;

        let mut content_info = transaction.get_content_info_by_shortcode(&queued_post.original_shortcode).await;
        content_info.status = ContentStatus::Published { shown: false };

        transaction.save_content_info(&content_info).await;

        let published_content = PublishedContent {
            username: queued_post.username.clone(),
            url: queued_post.url.clone(),
            caption: queued_post.caption.clone(),
            hashtags: queued_post.hashtags.clone(),
            original_author: queued_post.original_author.clone(),
            original_shortcode: queued_post.original_shortcode.clone(),
            published_at: now_in_my_timezone(&user_settings).to_rfc3339(),
        };

        transaction.save_published_content(published_content).await;
    }

    /// This function will amend the queue to ensure that only one post is posted at a time,
    /// even if the bot was shut down for a while.
    async fn amend_queue(&self) {
        let mut tx = self.database.begin_transaction().await;
        let content_queue = tx.load_content_queue().await;
        let user_settings = tx.load_user_settings().await;
        let mut content_to_post = 0;
        for queued_post in content_queue.iter().clone() {
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
                tx.save_queued_content(&queued_post).await;
            }
        }
    }
}
