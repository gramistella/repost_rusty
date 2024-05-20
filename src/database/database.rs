use std::collections::HashMap;
use std::fmt;
use std::ops::DerefMut;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Duration, Timelike, Utc};
use diesel::dsl::exists;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool, PooledConnection};
use diesel::result::Error;
use diesel::{delete, select, table, PgConnection, RunQueryDsl};
use image_hasher::ImageHash;
use rand::Rng;
use serenity::all::MessageId;
use tokio::sync::Mutex;

use crate::discord::state::ContentStatus;
use crate::discord::utils::now_in_my_timezone;
use crate::INTERFACE_UPDATE_INTERVAL;
use crate::IS_OFFLINE;

pub const DEFAULT_FAILURE_EXPIRATION: core::time::Duration = core::time::Duration::from_secs(60 * 60 * 24);
pub const DEFAULT_POSTED_EXPIRATION: core::time::Duration = core::time::Duration::from_secs(60 * 60 * 24);

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = user_settings)]
pub struct UserSettings {
    pub username: String,
    pub can_post: bool,
    pub posting_interval: i32,
    pub random_interval_variance: i32,
    pub rejected_content_lifespan: i32,
    pub timezone_offset: i32,
}

table! {
    user_settings (username) {
        username -> Text,
        can_post -> Bool,
        posting_interval -> Integer,
        random_interval_variance -> Integer,
        rejected_content_lifespan -> Integer,
        timezone_offset -> Integer,
    }
}

#[derive(Queryable, Insertable, Selectable, AsChangeset, Clone)]
#[diesel(table_name = queued_content)]
pub struct QueuedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub will_post_at: String,
}

table! {
    queued_content (username, original_shortcode) {
        username -> Text,
        url -> Text,
        caption -> Text,
        hashtags -> Text,
        original_author -> Text,
        original_shortcode -> Text,
        will_post_at -> Text,
    }
}

#[derive(Queryable, Insertable, Selectable, AsChangeset, Clone)]
#[diesel(table_name = published_content)]
pub struct PublishedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub published_at: String,
}

table! {
    published_content (username, original_shortcode) {
        username -> Text,
        url -> Text,
        caption -> Text,
        hashtags -> Text,
        original_author -> Text,
        original_shortcode -> Text,
        published_at -> Text,
    }
}

#[derive(Queryable, Insertable, Selectable, AsChangeset, Clone)]
#[diesel(table_name = rejected_content)]
pub struct RejectedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub rejected_at: String,
}

table! {
    rejected_content (username, original_shortcode) {
        username -> Text,
        url -> Text,
        caption -> Text,
        hashtags -> Text,
        original_author -> Text,
        original_shortcode -> Text,
        rejected_at -> Text,
    }
}

#[derive(Queryable, Insertable, AsChangeset, Clone)]
#[diesel(table_name = failed_content)]
pub struct FailedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub failed_at: String,
}

table! {
    failed_content (username, original_shortcode) {
        username -> Text,
        url -> Text,
        caption -> Text,
        hashtags -> Text,
        original_author -> Text,
        original_shortcode -> Text,
        failed_at -> Text,
    }
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

#[derive(Queryable, Insertable, Selectable, AsChangeset, Debug, Clone)]
#[diesel(table_name = content_info)]
pub(crate) struct InnerContentInfo {
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

table! {
    content_info (username, original_shortcode) {
        username -> Text,
        message_id -> BigInt,
        url -> Text,
        status -> Text,
        caption -> Text,
        hashtags -> Text,
        original_author -> Text,
        original_shortcode -> Text,
        last_updated_at -> Text,
        added_at -> Text,
        encountered_errors -> Integer,
    }
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

#[derive(Queryable, Insertable, Selectable, AsChangeset, PartialEq)]
#[diesel(table_name = video_hashes)]
pub struct InnerHashedVideo {
    pub username: String,
    pub duration: String,
    pub original_shortcode: String,
    pub hash_frame_1: String,
    pub hash_frame_2: String,
    pub hash_frame_3: String,
    pub hash_frame_4: String,
}

table! {
    video_hashes (original_shortcode) {
        username -> Text,
        original_shortcode -> Text,
        duration -> Text,
        hash_frame_1 -> Text,
        hash_frame_2 -> Text,
        hash_frame_3 -> Text,
        hash_frame_4 -> Text,
    }
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

#[derive(Queryable, QueryableByName, Insertable, Selectable, AsChangeset)]
#[diesel(table_name = bot_status)]
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

table! {
    use diesel::sql_types::*;
    bot_status (username) {
        username -> Text,
        message_id -> BigInt,
        status -> Integer,
        status_message -> Text,
        is_discord_warmed_up -> Bool,
        manual_mode -> Bool,
        last_updated_at -> Text,
        queue_alert_1_message_id -> BigInt,
        queue_alert_2_message_id -> BigInt,
        queue_alert_3_message_id -> BigInt,
        prev_content_queue_len -> Integer,
        halt_alert_message_id -> BigInt,
    }
}

#[derive(Queryable, QueryableByName, Insertable, Selectable)]
#[diesel(table_name = duplicate_content)]
pub struct DuplicateContent {
    pub username: String,
    pub original_shortcode: String,
}

table! {
    duplicate_content (original_shortcode) {
        username -> Text,
        original_shortcode -> Text,
    }
}

pub(crate) struct Database {
    pool: Arc<Mutex<Pool<ConnectionManager<PgConnection>>>>,
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
    pub fn new(username: String, credentials: HashMap<String, String>) -> Result<Self, Error> {
        let db_username = credentials.get("db_username").expect("No db_username field in credentials");
        let db_password = credentials.get("db_password").expect("No db_password field in credentials");
        let database_url = if IS_OFFLINE {
            format!("postgres://{db_username}:{db_password}@192.168.1.101/dev")
        } else {
            format!("postgres://{db_username}:{db_password}@192.168.1.101/prod")
        };

        let manager = ConnectionManager::<PgConnection>::new(database_url);

        let pool = Pool::new(manager).expect("Failed to create pool.");

        let mut pooled_conn = pool.get().unwrap();

        let conn = pooled_conn.deref_mut();

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS user_settings (
            username TEXT PRIMARY KEY,
            can_post BOOLEAN NOT NULL,
            posting_interval INTEGER NOT NULL,
            random_interval_variance INTEGER NOT NULL,
            rejected_content_lifespan INTEGER NOT NULL,
            timezone_offset INTEGER NOT NULL
        )",
        )
        .execute(conn)
        .unwrap();

        let user_exists = user_settings::table.filter(user_settings::username.eq(&username)).first::<UserSettings>(conn).is_ok();

        if !user_exists {
            if IS_OFFLINE {
                let user_settings = UserSettings {
                    username: username.clone(),
                    can_post: true,
                    posting_interval: 2,
                    random_interval_variance: 0,
                    rejected_content_lifespan: 2,
                    timezone_offset: 2,
                };

                diesel::insert_into(user_settings::table).values(&user_settings).execute(conn).unwrap();
            } else {
                let user_settings = UserSettings {
                    username: username.clone(),
                    can_post: true,
                    posting_interval: 150,
                    random_interval_variance: 30,
                    rejected_content_lifespan: 180,
                    timezone_offset: 2,
                };

                diesel::insert_into(user_settings::table).values(&user_settings).execute(conn).unwrap();
            }
        }

        diesel::sql_query(
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
            PRIMARY KEY (username, original_shortcode)
        )",
        )
        .execute(conn)
        .unwrap();

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS queued_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            will_post_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )",
        )
        .execute(conn)
        .unwrap();

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS published_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            published_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )",
        )
        .execute(conn)
        .unwrap();

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS rejected_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            rejected_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )",
        )
        .execute(conn)
        .unwrap();

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS failed_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            failed_at TEXT NOT NULL,
            PRIMARY KEY (username, original_shortcode)
        )",
        )
        .execute(conn)
        .unwrap();

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS video_hashes (
            username TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            duration TEXT NOT NULL,
            hash_frame_1 TEXT NOT NULL,
            hash_frame_2 TEXT NOT NULL,
            hash_frame_3 TEXT NOT NULL,
            hash_frame_4 TEXT NOT NULL,
            PRIMARY KEY (original_shortcode)
        )",
        )
        .execute(conn)
        .unwrap();

        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS duplicate_content (
            username TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            PRIMARY KEY (original_shortcode)
        )",
        )
        .execute(conn)
        .unwrap();

        diesel::sql_query(
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
            halt_alert_message_id BIGINT NOT NULL
        )",
        )
        .execute(conn)
        .unwrap();

        let bot_status_exists = bot_status::table.filter(bot_status::username.eq(&username)).first::<InnerBotStatus>(conn).is_ok();

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
            diesel::insert_into(bot_status::table).values(&bot_status).execute(conn).unwrap();
        }

        let pool = Arc::new(Mutex::new(pool));

        Ok(Database { pool, username })
    }
    pub async fn begin_transaction(&self) -> DatabaseTransaction {
        let pool_guard = self.pool.lock().await;
        let conn = pool_guard.get().unwrap();
        DatabaseTransaction { conn, username: self.username.clone() }
    }
}

pub struct DatabaseTransaction {
    conn: PooledConnection<ConnectionManager<PgConnection>>,
    username: String,
}

impl DatabaseTransaction {
    pub fn load_user_settings(&mut self) -> UserSettings {
        user_settings::table.filter(user_settings::username.eq(&self.username)).first::<UserSettings>(&mut self.conn).unwrap()
    }

    pub fn save_user_settings(&mut self, user_settings: &UserSettings) {
        diesel::update(user_settings::table).filter(user_settings::username.eq(&self.username)).set(user_settings).execute(&mut self.conn).unwrap();
    }

    pub fn load_bot_status(&mut self) -> BotStatus {
        let bot_status = bot_status::table.filter(bot_status::username.eq(&self.username)).first::<InnerBotStatus>(&mut self.conn).unwrap();

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

    pub fn save_bot_status(&mut self, bot_status: &BotStatus) {
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

        diesel::update(bot_status::table).filter(bot_status::username.eq(&self.username)).set(inner_bot_status).execute(&mut self.conn).unwrap();
    }

    pub fn save_duplicate_content(&mut self, duplicate_content: DuplicateContent) {
        diesel::insert_into(duplicate_content::table).values(&duplicate_content).execute(&mut self.conn).unwrap();
    }

    pub fn load_duplicate_content(&mut self) -> Vec<DuplicateContent> {
        duplicate_content::table.filter(duplicate_content::username.eq(&self.username)).load::<DuplicateContent>(&mut self.conn).unwrap()
    }

    pub fn get_content_info_by_shortcode(&mut self, shortcode: &String) -> ContentInfo {
        let found_content = content_info::table.filter(content_info::username.eq(&self.username)).filter(content_info::original_shortcode.eq(&shortcode)).first::<InnerContentInfo>(&mut self.conn).unwrap();

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

    pub fn remove_content_info_with_shortcode(&mut self, shortcode: &String) {
        delete(content_info::table).filter(content_info::username.eq(&self.username)).filter(content_info::original_shortcode.eq(&shortcode)).execute(&mut self.conn).unwrap();
    }

    pub fn save_content_info(&mut self, content_info: &ContentInfo) {
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

        diesel::insert_into(content_info::table)
            .values(&inner_content_info)
            .on_conflict((content_info::username, content_info::original_shortcode))
            .do_update()
            .set(&inner_content_info)
            .execute(&mut self.conn)
            .unwrap();
    }

    pub fn load_content_mapping(&mut self) -> Vec<ContentInfo> {
        let content_list = content_info::table.filter(content_info::username.eq(&self.username)).order(content_info::added_at).load::<InnerContentInfo>(&mut self.conn).unwrap();

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

    pub fn get_temp_message_id(&mut self, user_settings: &UserSettings) -> u64 {
        let message_id_vec = content_info::table.select(content_info::message_id).filter(content_info::username.eq(&self.username)).load::<i64>(&mut self.conn).unwrap();

        let max_message_id = message_id_vec.iter().max().cloned();
        let msg_id = match max_message_id {
            Some(max) => max + 1000,
            None => now_in_my_timezone(user_settings).num_seconds_from_midnight() as i64,
        };

        msg_id as u64
    }

    pub fn remove_post_from_queue_with_shortcode(&mut self, shortcode: String) {
        use crate::database::database::queued_content::dsl::*;
        use diesel::prelude::*;

        let deleted_rows = delete(queued_content).filter(original_shortcode.eq(&shortcode)).filter(username.eq(&self.username)).execute(&mut self.conn).unwrap();

        if deleted_rows > 0 {
            let user_settings = self.load_user_settings();
            let mut queued_content_list = self.load_content_queue();

            if let Some(removed_post_index) = queued_content_list.iter().position(|content| content.original_shortcode == shortcode) {
                queued_content_list.remove(removed_post_index);

                for post in queued_content_list.iter_mut().skip(removed_post_index) {
                    post.will_post_at = self.get_new_post_time();

                    let mut content_info = self.get_content_info_by_shortcode(&post.original_shortcode);
                    content_info.last_updated_at = (now_in_my_timezone(&user_settings) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();
                    content_info.status = if content_info.status.to_string().contains("shown") { ContentStatus::Queued { shown: true } } else { ContentStatus::Queued { shown: false } };
                    self.save_content_info(&content_info);
                }
            }
        }
    }

    pub fn save_queued_content(&mut self, queued_post: &QueuedContent) {
        diesel::insert_into(queued_content::table)
            .values(queued_post)
            .on_conflict((queued_content::username, queued_content::original_shortcode))
            .do_update()
            .set(queued_post)
            .execute(&mut self.conn)
            .unwrap();
    }

    pub fn load_content_queue(&mut self) -> Vec<QueuedContent> {
        queued_content::table.filter(queued_content::username.eq(&self.username)).order(queued_content::will_post_at).load::<QueuedContent>(&mut self.conn).unwrap()
    }

    pub fn get_queued_content_by_shortcode(&mut self, shortcode: String) -> Option<QueuedContent> {
        let content_queue = self.load_content_queue();

        content_queue.iter().find(|&content| content.original_shortcode == shortcode).cloned()
    }

    pub fn get_rejected_content_by_shortcode(&mut self, shortcode: String) -> Option<RejectedContent> {
        let rejected_content = self.load_rejected_content();

        rejected_content.iter().find(|&content| content.original_shortcode == shortcode).cloned()
    }

    pub fn get_failed_content_by_shortcode(&mut self, shortcode: String) -> Option<FailedContent> {
        let failed_content = self.load_failed_content();

        failed_content.iter().find(|&content| content.original_shortcode == shortcode).cloned()
    }

    pub fn get_published_content_by_shortcode(&mut self, shortcode: String) -> Option<PublishedContent> {
        let published_content = self.load_posted_content();

        published_content.iter().find(|&content| content.original_shortcode == shortcode).cloned()
    }

    pub fn remove_rejected_content_with_shortcode(&mut self, shortcode: String) {
        delete(rejected_content::table).filter(rejected_content::original_shortcode.eq(&shortcode)).filter(rejected_content::username.eq(&self.username)).execute(&mut self.conn).unwrap();
    }

    pub fn save_rejected_content(&mut self, rejected_content: RejectedContent) {
        diesel::insert_into(rejected_content::table)
            .values(&rejected_content)
            .on_conflict((rejected_content::username, rejected_content::original_shortcode))
            .do_update()
            .set(&rejected_content)
            .execute(&mut self.conn)
            .unwrap();
    }

    pub fn load_rejected_content(&mut self) -> Vec<RejectedContent> {
        rejected_content::table.filter(rejected_content::username.eq(&self.username)).load::<RejectedContent>(&mut self.conn).unwrap()
    }

    /// Save a posted content to the database
    ///
    /// Will automatically remove the content from the content_queue
    pub fn save_published_content(&mut self, published_content: PublishedContent) {
        let queued_content = self.get_queued_content_by_shortcode(published_content.original_shortcode.clone());
        let mut removed = false;

        if let Some(queued_content) = queued_content {
            let user_settings = self.load_user_settings();
            let posting_interval = Duration::try_seconds((user_settings.posting_interval * 60) as i64).unwrap();
            if DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap() < now_in_my_timezone(&user_settings) - posting_interval {
                // If so, we remove the post from the queue using this function, since it also recalculates the will_post_at for the remaining posts
                // And will avoid content being posted all at once
                self.remove_post_from_queue_with_shortcode(published_content.original_shortcode.clone());
                removed = true;
            }
        }

        // Otherwise we remove the post from the queue to avoid recalculating the will_post_at of the other posts
        // This is what the "normal" behavior should be, the above will only happen if the bot was offline for a long time
        if !removed {
            // Firstly we remove the published_content from the content_queue
            delete(queued_content::table)
                .filter(queued_content::original_shortcode.eq(&published_content.original_shortcode))
                .filter(queued_content::username.eq(&self.username))
                .execute(&mut self.conn)
                .unwrap();
        }

        // We remove the published_content if it is already there
        delete(published_content::table)
            .filter(published_content::original_shortcode.eq(&published_content.original_shortcode))
            .filter(published_content::username.eq(&self.username))
            .execute(&mut self.conn)
            .unwrap();

        // Then we add the published_content to the published_content table
        diesel::insert_into(published_content::table).values(&published_content).execute(&mut self.conn).unwrap();
    }

    pub fn load_posted_content(&mut self) -> Vec<PublishedContent> {
        published_content::table.filter(published_content::username.eq(&self.username)).load::<PublishedContent>(&mut self.conn).unwrap()
    }

    /// Save a content that failed to upload to the database
    ///
    /// Will automatically remove the content from the content_queue
    pub fn save_failed_content(&mut self, failed_content: FailedContent) {
        // First we check if the content is actually in the content_queue
        let exists = queued_content::table.filter(queued_content::username.eq(&self.username)).filter(queued_content::original_shortcode.eq(&failed_content.original_shortcode)).execute(&mut self.conn).unwrap();

        if exists > 0 {
            // we remove the failed_content from the content_queue using this function
            // Since it also automatically recalculates the will_post_at for the remaining posts
            self.remove_post_from_queue_with_shortcode(failed_content.original_shortcode.clone());
        }

        // Then we add the failed_content to the failed_content table
        diesel::insert_into(failed_content::table).values(&failed_content).execute(&mut self.conn).unwrap();
    }

    pub fn load_failed_content(&mut self) -> Vec<FailedContent> {
        failed_content::table.filter(failed_content::username.eq(&self.username)).load::<FailedContent>(&mut self.conn).unwrap()
    }

    pub fn get_new_post_time(&mut self) -> String {
        let user_settings = self.load_user_settings();

        let posted_content = self.load_posted_content();
        let queued_content = self.load_content_queue();

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

    pub fn load_hashed_videos(&mut self) -> Vec<HashedVideo> {
        let hashed_videos = video_hashes::table.filter(video_hashes::username.eq(&self.username)).load::<InnerHashedVideo>(&mut self.conn).unwrap();

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

    pub fn save_hashed_video(&mut self, hashed_video: HashedVideo) {
        let inner_hashed_video = InnerHashedVideo {
            username: hashed_video.username.clone(),
            duration: hashed_video.duration.to_string(),
            original_shortcode: hashed_video.original_shortcode.clone(),
            hash_frame_1: hashed_video.hash_frame_1.to_base64(),
            hash_frame_2: hashed_video.hash_frame_2.to_base64(),
            hash_frame_3: hashed_video.hash_frame_3.to_base64(),
            hash_frame_4: hashed_video.hash_frame_4.to_base64(),
        };

        diesel::insert_into(video_hashes::table).values(&inner_hashed_video).on_conflict(video_hashes::original_shortcode).do_update().set(&inner_hashed_video).execute(&mut self.conn).unwrap();
    }

    pub fn does_content_exist_with_shortcode(&mut self, shortcode: String) -> bool {
        // Execute each statement and check if the URL exists
        let tables = ["content_info", "posted_content", "content_queue", "rejected_content", "failed_content", "duplicate_content"];
        let exists = tables.iter().any(|table| self.shortcode_exists_in_table(table, &shortcode));

        exists
    }

    pub fn does_content_exist_with_shortcode_in_queue(&mut self, shortcode: String) -> bool {
        // Execute each statement and check if the URL exists
        let tables = ["content_queue"];
        let exists = tables.iter().any(|table| self.shortcode_exists_in_table(table, &shortcode));

        exists
    }

    fn shortcode_exists_in_table(&mut self, table_name: &str, shortcode: &str) -> bool {
        match table_name {
            "content_info" => select(exists(content_info::table.filter(content_info::original_shortcode.eq(shortcode)).filter(content_info::username.eq(&self.username)))).get_result(&mut self.conn).unwrap(),
            "published_content" => select(exists(published_content::table.filter(published_content::original_shortcode.eq(shortcode)).filter(published_content::username.eq(&self.username)))).get_result(&mut self.conn).unwrap(),
            "queued_content" => select(exists(queued_content::table.filter(queued_content::original_shortcode.eq(shortcode)).filter(queued_content::username.eq(&self.username)))).get_result(&mut self.conn).unwrap(),
            "rejected_content" => select(exists(rejected_content::table.filter(rejected_content::original_shortcode.eq(shortcode)).filter(rejected_content::username.eq(&self.username)))).get_result(&mut self.conn).unwrap(),
            "failed_content" => select(exists(failed_content::table.filter(failed_content::original_shortcode.eq(shortcode)).filter(failed_content::username.eq(&self.username)))).get_result(&mut self.conn).unwrap(),
            "duplicate_content" => select(exists(duplicate_content::table.filter(duplicate_content::original_shortcode.eq(shortcode)).filter(duplicate_content::username.eq(&self.username)))).get_result(&mut self.conn).unwrap(),
            _ => false,
        }
    }

    pub async fn clear_all_other_bot_statuses(&mut self) {
        delete(bot_status::table.filter(bot_status::username.ne(&self.username))).execute(&mut self.conn).unwrap();
    }
}
