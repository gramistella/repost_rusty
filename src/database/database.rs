use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, Timelike, Utc};
use image_hasher::ImageHash;
use rand::Rng;
use serenity::all::MessageId;
use sqlx::pool::PoolConnection;
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlx_macros::*;
use sqlx::{query, query_as, Error, Pool, Postgres};

use crate::discord::state::ContentStatus;
use crate::discord::utils::now_in_my_timezone;
use crate::INITIAL_INTERFACE_UPDATE_INTERVAL;
use crate::IS_OFFLINE;

pub const DEFAULT_FAILURE_EXPIRATION: core::time::Duration = core::time::Duration::from_secs(60 * 60 * 24);
pub const DEFAULT_POSTED_EXPIRATION: core::time::Duration = core::time::Duration::from_secs(60 * 60 * 24);

#[derive(FromRow)]
pub struct UserSettings {
    pub username: String,
    pub can_post: bool,
    pub posting_interval: i32,
    pub interface_update_interval: i64,
    pub random_interval_variance: i32,
    pub rejected_content_lifespan: i32,
    pub timezone_offset: i32,
}

#[derive(Debug, Clone)]
pub struct QueuedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub will_post_at: String,
}

#[derive(Debug, Clone)]
pub struct PublishedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub published_at: String,
}

#[derive(Debug, Clone)]
pub struct RejectedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub rejected_at: String,
}

#[derive(Debug, Clone)]
pub struct FailedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub failed_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ContentInfo {
    pub username: String,
    pub message_id: MessageId,
    pub url: String,
    pub status: ContentStatus,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub added_at: String,
    pub encountered_errors: i32,
}

struct InnerContentInfo {
    pub username: String,
    pub message_id: i64,
    pub url: String,
    pub status: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub added_at: String,
    pub encountered_errors: i32,
}

#[derive(Debug, Clone)]
pub struct HashedVideo {
    pub username: String,
    pub duration: f64,
    pub original_shortcode: String,
    pub hash_frame_1: ImageHash,
    pub hash_frame_2: ImageHash,
    pub hash_frame_3: ImageHash,
    pub hash_frame_4: ImageHash,
}

struct InnerHashedVideo {
    pub username: String,
    pub duration: String,
    pub original_shortcode: String,
    pub hash_frame_1: String,
    pub hash_frame_2: String,
    pub hash_frame_3: String,
    pub hash_frame_4: String,
}

#[derive(Debug, Clone)]
pub struct BotStatus {
    pub username: String,
    pub message_id: MessageId,
    /// 0 = all good, 1 = account awaiting manual intervention, 2 = Other
    pub status: i32,
    pub status_message: String,
    pub is_discord_warmed_up: bool,
    pub manual_mode: bool,
    pub last_updated_at: String,
    pub queue_alert_1_message_id: MessageId,
    pub queue_alert_2_message_id: MessageId,
    pub queue_alert_3_message_id: MessageId,
    pub prev_content_queue_len: i32,
    pub halt_alert_message_id: MessageId,
}

struct InnerBotStatus {
    pub username: String,
    pub message_id: i64,
    /// 0 = all good, 1 = account awaiting manual intervention, 2 = Other
    pub status: i32,
    pub status_message: String,
    pub is_discord_warmed_up: bool,
    pub manual_mode: bool,
    pub last_updated_at: String,
    pub queue_alert_1_message_id: i64,
    pub queue_alert_2_message_id: i64,
    pub queue_alert_3_message_id: i64,
    pub prev_content_queue_len: i32,
    pub halt_alert_message_id: i64,
}

pub struct DuplicateContent {
    pub username: String,
    pub original_shortcode: String,
}

pub(crate) struct Database {
    pool: Pool<Postgres>,
    username: String,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This would have been nice, but it doesn't work
        // let pool = f.debug_struct("Pool")
        //    .field("state", &self.pool.state());
        //    //.field("config", &self.pool.config()) // These fields don't need to be printed
        //    //.field("manager", &self.pool.manager()) // These fields don't need to be printed

        f.debug_struct("Database")
            //.field("bot", &redacted_bot_debug_string) // Use the redacted string
            //.field("pool_state", &self.pool.state())
            .finish()
    }
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Database { pool: self.pool.clone(), username: self.username.clone() }
    }
}

impl Database {
    //noinspection RsConstantConditionIf
    pub async fn new(username: String, credentials: HashMap<String, String>) -> Result<Self, Error> {
        let db_username = credentials.get("db_username").expect("No db_username field in credentials");
        let db_password = credentials.get("db_password").expect("No db_password field in credentials");
        let database_url = if IS_OFFLINE {
            format!("postgres://{db_username}:{db_password}@192.168.1.101/dev")
        } else {
            format!("postgres://{db_username}:{db_password}@192.168.1.101/prod")
        };

        let pool = PgPoolOptions::new().max_connections(5).connect(&database_url).await?;

        query!(
            "CREATE TABLE IF NOT EXISTS user_settings (
            username TEXT PRIMARY KEY,
            can_post BOOLEAN NOT NULL,
            posting_interval INTEGER NOT NULL,
            interface_update_interval BIGINT NOT NULL,
            random_interval_variance INTEGER NOT NULL,
            rejected_content_lifespan INTEGER NOT NULL,
            timezone_offset INTEGER NOT NULL
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        let user_exists = query_as!(UserSettings, "SELECT * FROM user_settings WHERE username = $1", &username).fetch_optional(&pool).await.unwrap().is_some();

        if !user_exists {
            if IS_OFFLINE {
                let user_settings = UserSettings {
                    username: username.clone(),
                    can_post: true,
                    posting_interval: 2,
                    interface_update_interval: INITIAL_INTERFACE_UPDATE_INTERVAL.as_millis() as i64,
                    random_interval_variance: 0,
                    rejected_content_lifespan: 2,
                    timezone_offset: 2,
                };

                query!(
                    "INSERT INTO user_settings (username, can_post, posting_interval, interface_update_interval, random_interval_variance, rejected_content_lifespan, timezone_offset) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    user_settings.username,
                    user_settings.can_post,
                    user_settings.posting_interval,
                    user_settings.interface_update_interval,
                    user_settings.random_interval_variance,
                    user_settings.rejected_content_lifespan,
                    user_settings.timezone_offset
                )
                .execute(&pool)
                .await
                .unwrap();
            } else {
                let user_settings = UserSettings {
                    username: username.clone(),
                    can_post: true,
                    posting_interval: 150,
                    interface_update_interval: INITIAL_INTERFACE_UPDATE_INTERVAL.as_millis() as i64,
                    random_interval_variance: 30,
                    rejected_content_lifespan: 180,
                    timezone_offset: 2,
                };

                query!(
                    "INSERT INTO user_settings (username, can_post, posting_interval, interface_update_interval, random_interval_variance, rejected_content_lifespan, timezone_offset) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    user_settings.username,
                    user_settings.can_post,
                    user_settings.posting_interval,
                    user_settings.interface_update_interval,
                    user_settings.random_interval_variance,
                    user_settings.rejected_content_lifespan,
                    user_settings.timezone_offset
                )
                .execute(&pool)
                .await
                .unwrap();
            }
        }

        query!(
            "CREATE TABLE IF NOT EXISTS content_info (
            username TEXT NOT NULL,
            message_id BIGINT NOT NULL,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            last_updated_at TEXT NOT NULL,
            added_at TEXT NOT NULL,
            encountered_errors INTEGER NOT NULL,
            PRIMARY KEY (username, original_shortcode))
            "
        )
        .execute(&pool)
        .await
        .unwrap();

        query!(
            "CREATE TABLE IF NOT EXISTS queued_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            will_post_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        query!(
            "CREATE TABLE IF NOT EXISTS published_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            published_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        query!(
            "CREATE TABLE IF NOT EXISTS rejected_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            rejected_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        query!(
            "CREATE TABLE IF NOT EXISTS failed_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            failed_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        query!(
            "CREATE TABLE IF NOT EXISTS video_hashes (
            username TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            duration TEXT NOT NULL,
            hash_frame_1 TEXT NOT NULL,
            hash_frame_2 TEXT NOT NULL,
            hash_frame_3 TEXT NOT NULL,
            hash_frame_4 TEXT NOT NULL,
            PRIMARY KEY (original_shortcode)
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        query!(
            "CREATE TABLE IF NOT EXISTS duplicate_content (
            username TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            PRIMARY KEY (original_shortcode)
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        query!(
            "CREATE TABLE IF NOT EXISTS bot_status (
            username TEXT PRIMARY KEY,
            message_id BIGINT NOT NULL,
            status INTEGER NOT NULL,
            status_message TEXT NOT NULL,
            is_discord_warmed_up BOOLEAN NOT NULL,
            manual_mode BOOLEAN NOT NULL,
            last_updated_at TEXT NOT NULL,
            queue_alert_1_message_id BIGINT NOT NULL,
            queue_alert_2_message_id BIGINT NOT NULL,
            queue_alert_3_message_id BIGINT NOT NULL,
            prev_content_queue_len INTEGER NOT NULL,
            halt_alert_message_id BIGINT NOT NULL
        )"
        )
        .execute(&pool)
        .await
        .unwrap();

        let bot_status_exists = query_as!(InnerBotStatus, "SELECT * FROM bot_status WHERE username = $1", &username).fetch_one(&pool).await.is_ok();
        if !bot_status_exists {
            let bot_status = InnerBotStatus {
                username: username.clone(),
                message_id: 1,
                status: 0,
                status_message: "operational  🟢".to_string(),
                is_discord_warmed_up: false,
                manual_mode: false,
                last_updated_at: Utc::now().to_rfc3339(),
                queue_alert_1_message_id: 1,
                queue_alert_2_message_id: 1,
                queue_alert_3_message_id: 1,
                prev_content_queue_len: 0,
                halt_alert_message_id: 1,
            };
            query!("INSERT INTO bot_status (username, message_id, status, status_message, is_discord_warmed_up, manual_mode, last_updated_at, queue_alert_1_message_id, queue_alert_2_message_id, queue_alert_3_message_id, prev_content_queue_len, halt_alert_message_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
                bot_status.username,
                bot_status.message_id,
                bot_status.status,
                bot_status.status_message,
                bot_status.is_discord_warmed_up,
                bot_status.manual_mode,
                bot_status.last_updated_at,
                bot_status.queue_alert_1_message_id,
                bot_status.queue_alert_2_message_id,
                bot_status.queue_alert_3_message_id,
                bot_status.prev_content_queue_len,
                bot_status.halt_alert_message_id
            ).execute(&pool).await.unwrap();
        }

        Ok(Database { pool, username })
    }
    pub async fn begin_transaction(&self) -> DatabaseTransaction {
        let conn = self.pool.acquire().await.unwrap();
        DatabaseTransaction { conn, username: self.username.clone() }
    }
}

pub struct DatabaseTransaction {
    conn: PoolConnection<Postgres>,
    username: String,
}

impl DatabaseTransaction {
    pub async fn load_user_settings(&mut self) -> UserSettings {
        let user_settings = query_as!(UserSettings, "SELECT * FROM user_settings WHERE username = $1", &self.username).fetch_one(self.conn.as_mut()).await.unwrap();
        user_settings
    }

    pub async fn save_user_settings(&mut self, user_settings: &UserSettings) {
        query!(
            "UPDATE user_settings SET can_post = $1, posting_interval = $2, interface_update_interval = $3, random_interval_variance = $4, rejected_content_lifespan = $5, timezone_offset = $6 WHERE username = $7",
            user_settings.can_post,
            user_settings.posting_interval,
            user_settings.interface_update_interval,
            user_settings.random_interval_variance,
            user_settings.rejected_content_lifespan,
            user_settings.timezone_offset,
            user_settings.username
        )
        .execute(self.conn.as_mut())
        .await
        .unwrap();
    }

    pub async fn load_bot_status(&mut self) -> BotStatus {
        let bot_status = query_as!(InnerBotStatus, "SELECT * FROM bot_status WHERE username = $1", &self.username).fetch_one(self.conn.as_mut()).await.unwrap();

        BotStatus {
            username: bot_status.username,
            message_id: MessageId::new(bot_status.message_id as u64),
            status: bot_status.status,
            status_message: bot_status.status_message,
            is_discord_warmed_up: bot_status.is_discord_warmed_up,
            manual_mode: bot_status.manual_mode,
            last_updated_at: bot_status.last_updated_at,
            queue_alert_1_message_id: MessageId::new(bot_status.queue_alert_1_message_id as u64),
            queue_alert_2_message_id: MessageId::new(bot_status.queue_alert_2_message_id as u64),
            queue_alert_3_message_id: MessageId::new(bot_status.queue_alert_3_message_id as u64),
            prev_content_queue_len: bot_status.prev_content_queue_len,
            halt_alert_message_id: MessageId::new(bot_status.halt_alert_message_id as u64),
        }
    }

    pub async fn save_bot_status(&mut self, bot_status: &BotStatus) {
        let inner_bot_status = InnerBotStatus {
            username: bot_status.username.clone(),
            message_id: bot_status.message_id.get() as i64,
            status: bot_status.status,
            status_message: bot_status.status_message.clone(),
            is_discord_warmed_up: bot_status.is_discord_warmed_up,
            manual_mode: bot_status.manual_mode,
            last_updated_at: bot_status.last_updated_at.clone(),
            queue_alert_1_message_id: bot_status.queue_alert_1_message_id.get() as i64,
            queue_alert_2_message_id: bot_status.queue_alert_2_message_id.get() as i64,
            queue_alert_3_message_id: bot_status.queue_alert_3_message_id.get() as i64,
            prev_content_queue_len: bot_status.prev_content_queue_len,
            halt_alert_message_id: bot_status.halt_alert_message_id.get() as i64,
        };

        query!("UPDATE bot_status SET message_id = $1, status = $2, status_message = $3, is_discord_warmed_up = $4, manual_mode = $5, last_updated_at = $6, queue_alert_1_message_id = $7, queue_alert_2_message_id = $8, queue_alert_3_message_id = $9, prev_content_queue_len = $10, halt_alert_message_id = $11 WHERE username = $12",
            inner_bot_status.message_id,
            inner_bot_status.status,
            inner_bot_status.status_message,
            inner_bot_status.is_discord_warmed_up,
            inner_bot_status.manual_mode,
            inner_bot_status.last_updated_at,
            inner_bot_status.queue_alert_1_message_id,
            inner_bot_status.queue_alert_2_message_id,
            inner_bot_status.queue_alert_3_message_id,
            inner_bot_status.prev_content_queue_len,
            inner_bot_status.halt_alert_message_id,
            inner_bot_status.username
        ).execute(self.conn.as_mut()).await.unwrap();
    }

    pub async fn save_duplicate_content(&mut self, duplicate_content: &DuplicateContent) {
        query!("INSERT INTO duplicate_content (username, original_shortcode) VALUES ($1, $2)", duplicate_content.username, duplicate_content.original_shortcode)
            .execute(self.conn.as_mut())
            .await
            .unwrap();
    }

    pub async fn load_duplicate_content(&mut self) -> Vec<DuplicateContent> {
        query_as!(DuplicateContent, "SELECT * FROM duplicate_content WHERE username = $1", &self.username).fetch_all(self.conn.as_mut()).await.unwrap()
    }

    pub async fn get_content_info_by_shortcode(&mut self, shortcode: &String) -> ContentInfo {
        let found_content = query_as!(InnerContentInfo, "SELECT * FROM content_info WHERE username = $1 AND original_shortcode = $2", &self.username, shortcode).fetch_one(self.conn.as_mut()).await.unwrap();

        ContentInfo {
            username: found_content.username,
            message_id: MessageId::new(found_content.message_id as u64),
            url: found_content.url,
            status: ContentStatus::from_str(&found_content.status).unwrap(),
            caption: found_content.caption,
            hashtags: found_content.hashtags,
            original_author: found_content.original_author,
            original_shortcode: found_content.original_shortcode,
            last_updated_at: found_content.last_updated_at,
            added_at: found_content.added_at,
            encountered_errors: found_content.encountered_errors,
        }
    }

    pub async fn remove_content_info_with_shortcode(&mut self, shortcode: &String) {
        query!("DELETE FROM content_info WHERE username = $1 AND original_shortcode = $2", &self.username, shortcode).execute(self.conn.as_mut()).await.unwrap();

        if self.does_content_exist_with_shortcode_in_queue(shortcode).await {
            self.remove_post_from_queue_with_shortcode(shortcode).await;
        }
    }

    pub async fn save_content_info(&mut self, content_info: &ContentInfo) {
        let span = tracing::span!(tracing::Level::INFO, "save_content_mapping");
        let _enter = span.enter();

        let inner_content_info = InnerContentInfo {
            username: content_info.username.clone(),
            message_id: content_info.message_id.get() as i64,
            url: content_info.url.clone(),
            status: content_info.status.to_string(),
            caption: content_info.caption.clone(),
            hashtags: content_info.hashtags.clone(),
            original_author: content_info.original_author.clone(),
            original_shortcode: content_info.original_shortcode.clone(),
            last_updated_at: content_info.last_updated_at.clone(),
            added_at: content_info.added_at.clone(),
            encountered_errors: content_info.encountered_errors,
        };

        query!("INSERT INTO content_info (username, message_id, url, status, caption, hashtags, original_author, original_shortcode, last_updated_at, added_at, encountered_errors) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) ON CONFLICT (username, original_shortcode) DO UPDATE SET message_id = $2, url = $3, status = $4, caption = $5, hashtags = $6, original_author = $7, last_updated_at = $9, added_at = $10, encountered_errors = $11",
            inner_content_info.username,
            inner_content_info.message_id,
            inner_content_info.url,
            inner_content_info.status,
            inner_content_info.caption,
            inner_content_info.hashtags,
            inner_content_info.original_author,
            inner_content_info.original_shortcode,
            inner_content_info.last_updated_at,
            inner_content_info.added_at,
            inner_content_info.encountered_errors
        ).execute(self.conn.as_mut()).await.unwrap();
    }

    pub async fn load_content_mapping(&mut self) -> Vec<ContentInfo> {
        let content_list = query_as!(InnerContentInfo, "SELECT * FROM content_info WHERE username = $1 ORDER BY added_at", &self.username).fetch_all(self.conn.as_mut()).await.unwrap();

        let content_list = content_list
            .iter()
            .map(|content| ContentInfo {
                username: content.username.clone(),
                message_id: MessageId::new(content.message_id as u64),
                url: content.url.clone(),
                status: ContentStatus::from_str(&content.status).unwrap(),
                caption: content.caption.clone(),
                hashtags: content.hashtags.clone(),
                original_author: content.original_author.clone(),
                original_shortcode: content.original_shortcode.clone(),
                last_updated_at: content.last_updated_at.clone(),
                added_at: content.added_at.clone(),
                encountered_errors: content.encountered_errors,
            })
            .collect::<Vec<ContentInfo>>();

        content_list
    }

    pub async fn get_temp_message_id(&mut self, user_settings: &UserSettings) -> u64 {
        let record_list = query!("SELECT message_id FROM content_info WHERE username = $1", &self.username).fetch_all(self.conn.as_mut()).await.unwrap();

        let mut message_id_vec = Vec::new();
        for record in record_list {
            message_id_vec.push(record.message_id);
        }
        let max_message_id = message_id_vec.iter().max().cloned();
        let msg_id = match max_message_id {
            Some(max) => max + 1000,
            None => now_in_my_timezone(user_settings).num_seconds_from_midnight() as i64,
        };

        msg_id as u64
    }

    pub async fn remove_post_from_queue_with_shortcode(&mut self, shortcode: &String) {
        let deleted_rows = query!("DELETE FROM queued_content WHERE original_shortcode = $1 AND username = $2", shortcode, &self.username).execute(self.conn.as_mut()).await.unwrap().rows_affected();

        if deleted_rows > 0 {
            let user_settings = self.load_user_settings().await;
            let mut queued_content_list = self.load_content_queue().await;

            if let Some(removed_post_index) = queued_content_list.iter().position(|content| content.original_shortcode == *shortcode) {
                queued_content_list.remove(removed_post_index);

                for post in queued_content_list.iter_mut().skip(removed_post_index) {
                    post.will_post_at = self.get_new_post_time().await;

                    let mut content_info = self.get_content_info_by_shortcode(&post.original_shortcode).await;
                    content_info.last_updated_at = (now_in_my_timezone(&user_settings) - Duration::milliseconds(user_settings.interface_update_interval)).to_rfc3339();
                    content_info.status = if content_info.status.to_string().contains("shown") { ContentStatus::Queued { shown: true } } else { ContentStatus::Queued { shown: false } };
                    self.save_content_info(&content_info).await;
                }
            }
        }
    }

    pub async fn save_queued_content(&mut self, queued_content: &QueuedContent) {
        query!(
            "INSERT INTO queued_content (username, url, caption, hashtags, original_author, original_shortcode, will_post_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (username, original_shortcode) DO UPDATE SET url = $2, caption = $3, hashtags = $4, original_author = $5, will_post_at = $7",
            queued_content.username,
            queued_content.url,
            queued_content.caption,
            queued_content.hashtags,
            queued_content.original_author,
            queued_content.original_shortcode,
            queued_content.will_post_at
        )
        .execute(self.conn.as_mut())
        .await
        .unwrap();
    }

    pub async fn load_content_queue(&mut self) -> Vec<QueuedContent> {
        query_as!(QueuedContent, "SELECT * FROM queued_content WHERE username = $1 ORDER BY will_post_at", &self.username).fetch_all(self.conn.as_mut()).await.unwrap()
    }

    pub async fn get_queued_content_by_shortcode(&mut self, shortcode: &String) -> Option<QueuedContent> {
        let content_queue = self.load_content_queue().await;
        content_queue.iter().find(|&content| content.original_shortcode == *shortcode).cloned()
    }

    pub async fn get_rejected_content_by_shortcode(&mut self, shortcode: &String) -> Option<RejectedContent> {
        let rejected_content = self.load_rejected_content().await;

        rejected_content.iter().find(|&content| content.original_shortcode == *shortcode).cloned()
    }

    pub async fn get_failed_content_by_shortcode(&mut self, shortcode: &String) -> Option<FailedContent> {
        let failed_content = self.load_failed_content().await;

        failed_content.iter().find(|&content| content.original_shortcode == *shortcode).cloned()
    }

    pub async fn get_published_content_by_shortcode(&mut self, shortcode: &String) -> Option<PublishedContent> {
        let published_content = self.load_posted_content().await;

        published_content.iter().find(|&content| content.original_shortcode == *shortcode).cloned()
    }

    pub async fn remove_rejected_content_with_shortcode(&mut self, shortcode: &String) {
        query!("DELETE FROM rejected_content WHERE original_shortcode = $1 AND username = $2", shortcode, &self.username).execute(self.conn.as_mut()).await.unwrap();
    }

    pub async fn save_rejected_content(&mut self, rejected_content: &RejectedContent) {
        query!(
            "INSERT INTO rejected_content (username, url, caption, hashtags, original_author, original_shortcode, rejected_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (username, original_shortcode) DO UPDATE SET url = $2, caption = $3, hashtags = $4, original_author = $5, rejected_at = $7",
            rejected_content.username,
            rejected_content.url,
            rejected_content.caption,
            rejected_content.hashtags,
            rejected_content.original_author,
            rejected_content.original_shortcode,
            rejected_content.rejected_at
        )
        .execute(self.conn.as_mut())
        .await
        .unwrap();
    }

    pub async fn load_rejected_content(&mut self) -> Vec<RejectedContent> {
        query_as!(RejectedContent, "SELECT * FROM rejected_content WHERE username = $1", &self.username).fetch_all(self.conn.as_mut()).await.unwrap()
    }

    /// Save a posted content to the database
    ///
    /// Will automatically remove the content from the content_queue
    pub async fn save_published_content(&mut self, published_content: &PublishedContent) {
        let queued_content = self.get_queued_content_by_shortcode(&published_content.original_shortcode).await;
        let mut removed = false;

        if let Some(queued_content) = queued_content {
            let user_settings = self.load_user_settings().await;
            let posting_interval = Duration::try_seconds((user_settings.posting_interval * 60) as i64).unwrap();
            if DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap() < now_in_my_timezone(&user_settings) - posting_interval {
                // If so, we remove the post from the queue using this function, since it also recalculates the will_post_at for the remaining posts
                // And will avoid content being posted all at once
                self.remove_post_from_queue_with_shortcode(&published_content.original_shortcode).await;
                removed = true;
            }
        }

        // Otherwise we remove the post from the queue to avoid recalculating the will_post_at of the other posts
        // This is what the "normal" behavior should be, the above will only happen if the bot was offline for a long time
        if !removed {
            // Firstly we remove the published_content from the content_queue
            query!("DELETE FROM queued_content WHERE original_shortcode = $1 AND username = $2", published_content.original_shortcode, &self.username).execute(self.conn.as_mut()).await.unwrap();
        }

        query!("DELETE FROM published_content WHERE original_shortcode = $1 AND username = $2", published_content.original_shortcode, &self.username).execute(self.conn.as_mut()).await.unwrap();

        query!(
            "INSERT INTO published_content (username, url, caption, hashtags, original_author, original_shortcode, published_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            published_content.username,
            published_content.url,
            published_content.caption,
            published_content.hashtags,
            published_content.original_author,
            published_content.original_shortcode,
            published_content.published_at
        )
        .execute(self.conn.as_mut())
        .await
        .unwrap();
    }

    pub async fn load_posted_content(&mut self) -> Vec<PublishedContent> {
        query_as!(PublishedContent, "SELECT * FROM published_content WHERE username = $1", &self.username).fetch_all(self.conn.as_mut()).await.unwrap()
    }

    /// Save a content that failed to upload to the database
    ///
    /// Will automatically remove the content from the content_queue
    pub async fn save_failed_content(&mut self, failed_content: &FailedContent) {
        // First we check if the content is actually in the content_queue
        let exists = query!("SELECT * FROM queued_content WHERE username = $1 AND original_shortcode = $2", &self.username, failed_content.original_shortcode).fetch_all(self.conn.as_mut()).await.unwrap().len();

        if exists > 0 {
            // we remove the failed_content from the content_queue using this function
            // Since it also automatically recalculates the will_post_at for the remaining posts
            self.remove_post_from_queue_with_shortcode(&failed_content.original_shortcode.clone()).await;
        }

        // Then we add the failed_content to the failed_content table
        query!(
            "INSERT INTO failed_content (username, url, caption, hashtags, original_author, original_shortcode, failed_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            failed_content.username,
            failed_content.url,
            failed_content.caption,
            failed_content.hashtags,
            failed_content.original_author,
            failed_content.original_shortcode,
            failed_content.failed_at
        )
        .execute(self.conn.as_mut())
        .await
        .unwrap();
    }

    pub async fn load_failed_content(&mut self) -> Vec<FailedContent> {
        query_as!(FailedContent, "SELECT * FROM failed_content WHERE username = $1", &self.username).fetch_all(self.conn.as_mut()).await.unwrap()
    }

    pub async fn get_new_post_time(&mut self) -> String {
        let user_settings = self.load_user_settings().await;

        let posted_content = self.load_posted_content().await;
        let queued_content = self.load_content_queue().await;

        let current_time = now_in_my_timezone(&user_settings);

        // Get all the post times
        let mut post_times = Vec::new();
        for post in &posted_content {
            let post_time = DateTime::parse_from_rfc3339(&post.published_at).unwrap().with_timezone(&Utc);
            post_times.push(post_time);
        }
        for post in &queued_content {
            let post_time = DateTime::parse_from_rfc3339(&post.will_post_at).unwrap().with_timezone(&Utc);
            post_times.push(post_time);
        }

        post_times.sort();

        let posting_interval = Duration::try_seconds((user_settings.posting_interval * 60) as i64).unwrap();
        // Filter out the post times that are before the current time
        post_times.retain(|time| *time >= current_time - posting_interval);

        let random_interval = user_settings.random_interval_variance * 60;
        let mut rng = rand::thread_rng();
        let random_variance = rng.gen_range(-random_interval..=random_interval);

        let randomized_posting_interval = Duration::try_seconds((user_settings.posting_interval * 60 + random_variance) as i64).unwrap();

        // Find the first gap in the post times
        for windows in post_times.windows(2) {
            let gap = windows[1] - windows[0];
            if gap > posting_interval + Duration::try_seconds(random_interval as i64).unwrap() {
                let new_post_time = windows[0] + randomized_posting_interval;
                tracing::info!("Gap found, new post time: {}", new_post_time.to_rfc3339());
                return new_post_time.to_rfc3339();
            }
        }

        // If no gap is found, we return the latest post time + posting interval
        let new_post_time = match post_times.last() {
            None => {
                let new_post_time = current_time + Duration::try_seconds(60).unwrap();
                tracing::info!("No recent posts found, posting in 1 minute: {}", new_post_time.to_rfc3339());
                new_post_time
            }
            Some(&last_post_time) => {
                let new_post_time = last_post_time + randomized_posting_interval;
                tracing::info!("No gap found, new post time: {}", new_post_time.to_rfc3339());
                new_post_time
            }
        };

        new_post_time.to_rfc3339()
    }

    pub async fn load_hashed_videos(&mut self) -> Vec<HashedVideo> {
        let hashed_videos = query_as!(InnerHashedVideo, "SELECT * FROM video_hashes WHERE username = $1", &self.username).fetch_all(self.conn.as_mut()).await.unwrap();

        let outer_hashed_video = hashed_videos
            .iter()
            .map(|hashed_video| HashedVideo {
                username: hashed_video.username.clone(),
                duration: hashed_video.duration.parse::<f64>().unwrap(),
                original_shortcode: hashed_video.original_shortcode.clone(),
                hash_frame_1: ImageHash::from_base64(&hashed_video.hash_frame_1).unwrap(),
                hash_frame_2: ImageHash::from_base64(&hashed_video.hash_frame_2).unwrap(),
                hash_frame_3: ImageHash::from_base64(&hashed_video.hash_frame_3).unwrap(),
                hash_frame_4: ImageHash::from_base64(&hashed_video.hash_frame_4).unwrap(),
            })
            .collect::<Vec<HashedVideo>>();

        outer_hashed_video
    }

    pub async fn save_hashed_video(&mut self, hashed_video: &HashedVideo) {
        let inner_hashed_video = InnerHashedVideo {
            username: hashed_video.username.clone(),
            duration: hashed_video.duration.to_string(),
            original_shortcode: hashed_video.original_shortcode.clone(),
            hash_frame_1: hashed_video.hash_frame_1.to_base64(),
            hash_frame_2: hashed_video.hash_frame_2.to_base64(),
            hash_frame_3: hashed_video.hash_frame_3.to_base64(),
            hash_frame_4: hashed_video.hash_frame_4.to_base64(),
        };

        query!(
            "INSERT INTO video_hashes (username, original_shortcode, duration, hash_frame_1, hash_frame_2, hash_frame_3, hash_frame_4) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (original_shortcode) DO UPDATE SET duration = $3, hash_frame_1 = $4, hash_frame_2 = $5, hash_frame_3 = $6, hash_frame_4 = $7",
            inner_hashed_video.username,
            inner_hashed_video.original_shortcode,
            inner_hashed_video.duration,
            inner_hashed_video.hash_frame_1,
            inner_hashed_video.hash_frame_2,
            inner_hashed_video.hash_frame_3,
            inner_hashed_video.hash_frame_4
        )
        .execute(self.conn.as_mut())
        .await
        .unwrap();
    }

    pub async fn does_content_exist_with_shortcode(&mut self, shortcode: &String) -> bool {
        // Execute each statement and check if the URL exists
        let tables = ["content_info", "posted_content", "content_queue", "rejected_content", "failed_content", "duplicate_content"];
        for table in tables {
            let exists = self.shortcode_exists_in_table(table, &shortcode).await;
            if exists {
                return true;
            }
        }
        false
    }

    pub async fn does_content_exist_with_shortcode_in_queue(&mut self, shortcode: &String) -> bool {
        // Execute each statement and check if the URL exists
        let tables = ["content_queue"];
        for table in tables {
            let exists = self.shortcode_exists_in_table(table, &shortcode).await;
            if exists {
                return true;
            }
        }
        false
    }

    async fn shortcode_exists_in_table(&mut self, table_name: &str, shortcode: &str) -> bool {
        match table_name {
            "content_info" => query!("SELECT EXISTS(SELECT 1 FROM content_info WHERE original_shortcode = $1 AND username = $2)", shortcode, &self.username).fetch_one(self.conn.as_mut()).await.unwrap().exists.unwrap(),
            "published_content" => query!("SELECT EXISTS(SELECT 1 FROM published_content WHERE original_shortcode = $1 AND username = $2)", shortcode, &self.username).fetch_one(self.conn.as_mut()).await.unwrap().exists.unwrap(),
            "queued_content" => query!("SELECT EXISTS(SELECT 1 FROM queued_content WHERE original_shortcode = $1 AND username = $2)", shortcode, &self.username).fetch_one(self.conn.as_mut()).await.unwrap().exists.unwrap(),
            "rejected_content" => query!("SELECT EXISTS(SELECT 1 FROM rejected_content WHERE original_shortcode = $1 AND username = $2)", shortcode, &self.username).fetch_one(self.conn.as_mut()).await.unwrap().exists.unwrap(),
            "failed_content" => query!("SELECT EXISTS(SELECT 1 FROM failed_content WHERE original_shortcode = $1 AND username = $2)", shortcode, &self.username).fetch_one(self.conn.as_mut()).await.unwrap().exists.unwrap(),
            "duplicate_content" => query!("SELECT EXISTS(SELECT 1 FROM duplicate_content WHERE original_shortcode = $1 AND username = $2)", shortcode, &self.username).fetch_one(self.conn.as_mut()).await.unwrap().exists.unwrap(),
            _ => false,
        }
    }

    pub async fn clear_all_other_bot_statuses(&mut self) {
        query!("DELETE FROM bot_status WHERE username != $1", &self.username).execute(self.conn.as_mut()).await.unwrap();
    }
}
