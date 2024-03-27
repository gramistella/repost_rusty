use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Duration, Timelike, Utc};
use indexmap::IndexMap;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rand::Rng;
use rusqlite::{params, Result};
use serde::{Deserialize, Serialize};
use teloxide::types::MessageId;
use tokio::sync::Mutex;

use crate::telegram_bot::state::ContentStatus;
use crate::utils::now_in_my_timezone;
use crate::INTERFACE_UPDATE_INTERVAL;

#[derive(Clone, Debug)]
pub struct UserSettings {
    pub username: String,
    pub can_post: bool,
    pub posting_interval: i64,
    pub random_interval_variance: i64,
    pub rejected_content_lifespan: i64,
    pub posted_content_lifespan: i64,
    pub timezone_offset: i32,
    pub current_page: i32,
    pub page_size: i32,
}

#[derive(Clone, PartialEq)]
pub struct QueuedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub will_post_at: String,
}

#[derive(Clone)]
pub struct PostedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub posted_at: String,
    pub expired: bool,
}

#[derive(Clone)]
pub struct RejectedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub rejected_at: String,
    pub expired: bool,
}

#[derive(Clone)]
pub struct FailedContent {
    pub username: String,
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub failed_at: String,
    pub expired: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ContentInfo {
    pub username: String,
    pub url: String,
    pub status: ContentStatus,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub url_last_updated_at: String,
    pub encountered_errors: i32,
    pub page_num: i32,
}

const PROD_DB: &str = "db/prod.db";
const DEV_DB: &str = "db/dev.db";

pub const DEFAULT_FAILURE_EXPIRATION: core::time::Duration = core::time::Duration::from_secs(60 * 60 * 24);

pub(crate) struct Database {
    pool: Arc<Mutex<Pool<SqliteConnectionManager>>>,
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
        Database {
            pool: self.pool.clone(),
            username: self.username.clone(),
        }
    }
}

impl Database {
    pub fn new(username: String, is_offline: bool) -> Result<Self> {
        let manager = if is_offline { SqliteConnectionManager::file(DEV_DB) } else { SqliteConnectionManager::file(PROD_DB) };

        let pool = Pool::new(manager).unwrap();

        let conn = pool.get().unwrap();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_settings (
            username TEXT PRIMARY KEY,
            can_post INTEGER NOT NULL,
            posting_interval INTEGER NOT NULL,
            random_interval_variance INTEGER NOT NULL,
            rejected_content_lifespan INTEGER NOT NULL,
            posted_content_lifespan INTEGER NOT NULL,
            timezone_offset INTEGER NOT NULL,
            current_page INTEGER NOT NULL,
            page_size INTEGER NOT NULL
        )",
            [],
        )?;

        let default_timezone_offset = 1;
        let default_current_page = 1;

        let user_settings_exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM user_settings WHERE username = ?1)", params![username], |row| row.get(0)).unwrap_or(false);

        if !user_settings_exists {
            if is_offline {
                let default_is_posting = 1;
                let default_posting_interval = 2;
                let default_random_interval = 0;
                let default_removed_content_lifespan = 2;
                let default_posted_content_lifespan = 2;
                let default_page_size = 4;

                let query = format!(
                    "INSERT INTO user_settings (username, can_post, posting_interval, random_interval_variance, rejected_content_lifespan, posted_content_lifespan, timezone_offset, current_page, page_size) VALUES ('{}', {}, {}, {}, {}, {}, {}, {}, {})",
                    username, default_is_posting, default_posting_interval, default_random_interval, default_removed_content_lifespan, default_posted_content_lifespan, default_timezone_offset, default_current_page, default_page_size
                );
                conn.execute(&query, [])?;
            } else {
                let default_is_posting = 1;
                let default_posting_interval = 150;
                let default_random_interval = 30;
                let default_removed_content_lifespan = 120;
                let default_posted_content_lifespan = 120;
                let default_page_size = 8;
                let query = format!(
                    "INSERT INTO user_settings (can_post, posting_interval, random_interval_variance, rejected_content_lifespan, posted_content_lifespan, timezone_offset, current_page, page_size) VALUES ('{}', {}, {}, {}, {}, {}, {}, {})",
                    default_is_posting, default_posting_interval, default_random_interval, default_removed_content_lifespan, default_posted_content_lifespan, default_timezone_offset, default_current_page, default_page_size
                );
                conn.execute(&query, [])?;
            }
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS content_info (
            message_id INTEGER PRIMARY KEY,
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            last_updated_at TEXT NOT NULL,
            url_last_updated_at TEXT NOT NULL,
            page_num INTEGER NOT NULL,
            encountered_errors INTEGER NOT NULL
        )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS content_queue (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            last_updated_at TEXT NOT NULL,
            will_post_at TEXT NOT NULL
        )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS posted_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            last_updated_at TEXT NOT NULL,
            posted_at TEXT NOT NULL,
            expired BOOL NOT NULL
        )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS rejected_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            rejected_at TEXT NOT NULL,
            last_updated_at TEXT NOT NULL,
            expired BOOL NOT NULL
        )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS failed_content (
            username TEXT NOT NULL,
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            last_updated_at TEXT NOT NULL,
            failed_at TEXT NOT NULL,
            expired BOOL NOT NULL
        )",
            [],
        )?;

        let pool = Arc::new(Mutex::new(pool));

        Ok(Database { pool, username })
    }
    pub async fn begin_transaction(&self) -> Result<DatabaseTransaction> {
        let pool_guard = self.pool.lock().await;
        let conn = pool_guard.get().unwrap();
        Ok(DatabaseTransaction { conn, username: self.username.clone() })
    }
}

#[derive(Debug)]
pub struct DatabaseTransaction {
    conn: PooledConnection<SqliteConnectionManager>,
    username: String,
}

impl DatabaseTransaction {
    pub fn load_user_settings(&mut self) -> Result<UserSettings> {
        let mut can_post: Option<bool> = None;
        let mut posting_interval: Option<i64> = None;
        let mut random_interval_variance: Option<i64> = None;
        let mut rejected_content_lifespan: Option<i64> = None;
        let mut posted_content_lifespan: Option<i64> = None;
        let mut timezone_offset: Option<i32> = None;
        let mut current_page: Option<i32> = None;
        let mut page_size: Option<i32> = None;
        let tx = self.conn.transaction()?;

        tx.query_row(
            "SELECT can_post, posting_interval, random_interval_variance, rejected_content_lifespan, posted_content_lifespan, timezone_offset, current_page, page_size FROM user_settings WHERE username = ?1",
            [self.username.clone()],
            |row| {
                can_post = Some(row.get(0)?);
                posting_interval = Some(row.get(1)?);
                random_interval_variance = Some(row.get(2)?);
                rejected_content_lifespan = Some(row.get(3)?);
                posted_content_lifespan = Some(row.get(4)?);
                timezone_offset = Some(row.get(5)?);
                current_page = Some(row.get(6)?);
                page_size = Some(row.get(7)?);
                Ok(())
            },
        )?;

        let user_settings = UserSettings {
            username: self.username.clone(),
            can_post: can_post.unwrap(),
            posting_interval: posting_interval.unwrap(),
            random_interval_variance: random_interval_variance.unwrap(),
            rejected_content_lifespan: rejected_content_lifespan.unwrap(),
            posted_content_lifespan: posted_content_lifespan.unwrap(),
            timezone_offset: timezone_offset.unwrap(),
            current_page: current_page.unwrap(),
            page_size: page_size.unwrap(),
        };

        Ok(user_settings)
    }

    pub fn save_user_settings(&mut self, user_settings: UserSettings) -> Result<()> {
        let tx = self.conn.transaction()?;

        // Remove all the user settings
        tx.execute("DELETE FROM user_settings WHERE username = ?1", [self.username.clone()])?;

        // Update user settings
        tx.execute(
            "INSERT INTO user_settings (username, can_post, posting_interval, random_interval_variance, rejected_content_lifespan, posted_content_lifespan, timezone_offset, current_page, page_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,?9)",
            params![
                user_settings.username,
                user_settings.can_post as i64,
                user_settings.posting_interval,
                user_settings.random_interval_variance,
                user_settings.rejected_content_lifespan,
                user_settings.posted_content_lifespan,
                user_settings.timezone_offset,
                user_settings.current_page,
                user_settings.page_size
            ],
        )?;

        tx.commit()?;

        Ok(())
    }
    pub fn get_content_info_by_message_id(&mut self, message_id: MessageId) -> Option<ContentInfo> {
        let video_mapping = self.load_content_mapping().unwrap();

        match video_mapping.get(&message_id) {
            Some(content_info) => Some(content_info.clone()),
            None => None,
        }
    }

    pub fn get_content_info_by_shortcode(&mut self, shortcode: String) -> Option<(MessageId, ContentInfo)> {
        let video_mapping = self.load_content_mapping().unwrap();

        for (message_id, content_info) in video_mapping {
            if content_info.original_shortcode == shortcode {
                return Some((message_id, content_info));
            }
        }
        None
    }

    pub fn remove_content_info_with_shortcode(&mut self, shortcode: String) -> Result<()> {
        let tx = self.conn.transaction()?;

        // Remove the content info with the given shortcode
        tx.execute("DELETE FROM content_info WHERE original_shortcode = ?1 AND username = ?2", params![shortcode, self.username])?;

        tx.commit()?;

        // Load the remaining content info
        let content_info = self.load_content_mapping()?;

        // Recalculate the page numbers
        let user_settings = self.load_user_settings()?;
        let page_size = user_settings.page_size as i64;
        let mut new_content_info = IndexMap::new();
        let mut index = 0;
        for (message_id, mut info) in content_info {
            index += 1;
            let correct_page_num = ((index as f64 / page_size as f64) + 0.5).floor() as i32;
            if info.page_num != correct_page_num {
                info.page_num = correct_page_num;
                new_content_info.insert(message_id, info);
            } else {
                new_content_info.insert(message_id, info.clone());
            }
        }

        // Save the updated content info back to the database
        self.save_content_mapping(new_content_info)?;

        Ok(())
    }

    pub fn get_max_records_in_content_info(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row("SELECT COUNT(*) FROM content_info WHERE username = ?1", params![self.username], |row| row.get(0))?;
        Ok(count)
    }
    pub fn get_total_pages(&mut self) -> Result<i32> {
        Ok((self.get_max_records_in_content_info().unwrap() as i32 - 1) / self.load_user_settings().unwrap().page_size + 1)
    }

    pub fn save_content_mapping(&mut self, video_mapping: IndexMap<MessageId, ContentInfo>) -> Result<()> {
        let span = tracing::span!(tracing::Level::INFO, "save_content_mapping");
        let _enter = span.enter();

        let existing_mapping = self.load_content_mapping()?;
        let user_settings = self.load_user_settings()?;
        let page_size = user_settings.page_size as i64;

        let total_records;
        {
            total_records = self.get_max_records_in_content_info()?;
        }

        let tx = self.conn.transaction()?;
        for (new_key, mut new_value) in video_mapping {
            if let Some(existing_value) = existing_mapping.iter().find(|(_k, v)| v.original_shortcode == new_value.original_shortcode) {
                new_value.page_num = existing_value.1.page_num;
                tx.execute("DELETE FROM content_info WHERE original_shortcode = ?1 AND username = ?2", params![existing_value.1.original_shortcode, self.username])?;
            } else {
                new_value.page_num = (total_records / page_size + 1) as i32;
            }

            let status_string = new_value.status.to_string();
            tx.execute(
                "INSERT INTO content_info (username, message_id, url, status, caption, hashtags, original_author, original_shortcode, last_updated_at, url_last_updated_at, page_num, encountered_errors) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    self.username,
                    new_key.0,
                    new_value.url,
                    status_string,
                    new_value.caption,
                    new_value.hashtags,
                    new_value.original_author,
                    new_value.original_shortcode,
                    new_value.last_updated_at,
                    new_value.url_last_updated_at,
                    new_value.page_num,
                    new_value.encountered_errors
                ],
            )?;
        }

        tx.commit()?;

        // Handle gaps in the content mapping
        let content_mapping;
        {
            content_mapping = self.load_content_mapping()?;
        }

        let tx = self.conn.transaction()?;

        let mut new_content_info = IndexMap::new();
        let mut index = 0;
        for content_info in content_mapping {
            index += 1;
            new_content_info.insert(index, content_info);
        }

        let mut video_mapping = IndexMap::new();
        for (index, content_info) in new_content_info {
            let (message_id, mut info) = content_info;
            let correct_page_num = ((index as f64 - 1.0) / page_size as f64 + 1.0).floor() as i32;
            if info.page_num != correct_page_num {
                info.page_num = correct_page_num;
                tx.execute("UPDATE content_info SET page_num = ?1 WHERE original_shortcode = ?2 AND username = ?3", params![correct_page_num, info.original_shortcode, self.username])?;
            }
            video_mapping.insert(message_id, info);
        }

        tx.commit()?;
        Ok(())
    }

    pub fn load_next_page(&mut self) -> Result<IndexMap<MessageId, ContentInfo>> {
        let mut user_settings = self.load_user_settings().unwrap();
        user_settings.current_page += 1;
        self.save_user_settings(user_settings).unwrap();
        self.load_page()
    }

    pub fn load_previous_page(&mut self) -> Result<IndexMap<MessageId, ContentInfo>> {
        let mut user_settings = self.load_user_settings().unwrap();
        user_settings.current_page -= 1;
        self.save_user_settings(user_settings).unwrap();
        self.load_page()
    }
    pub fn load_page(&mut self) -> Result<IndexMap<MessageId, ContentInfo>> {
        let user_settings = self.load_user_settings().unwrap();
        let current_page = user_settings.current_page as i64;

        let mut current_content_mapping = IndexMap::new();
        for content_info in self.load_content_mapping().unwrap() {
            let (message_id, info) = content_info;
            if info.page_num == current_page as i32 {
                current_content_mapping.insert(message_id, info);
            }
        }

        Ok(current_content_mapping)
    }

    pub fn load_content_mapping(&mut self) -> Result<IndexMap<MessageId, ContentInfo>> {
        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare("SELECT username, message_id, url, status, caption, hashtags, original_author, original_shortcode, last_updated_at, url_last_updated_at, page_num, encountered_errors FROM content_info WHERE username = ?1 ORDER BY page_num, message_id")?;
        let content_info_iter = stmt.query_map([self.username.clone()], |row| {
            let username: String = row.get(0)?;
            let message_id: i32 = row.get(1)?;
            let url: String = row.get(2)?;
            let status: String = row.get(3)?;
            let caption: String = row.get(4)?;
            let hashtags: String = row.get(5)?;
            let original_author: String = row.get(6)?;
            let original_shortcode: String = row.get(7)?;
            let last_updated_at: String = row.get(8)?;
            let url_last_updated_at: String = row.get(9)?;
            let page_num: i32 = row.get(10)?;
            let encountered_errors: i32 = row.get(11)?;

            let status: ContentStatus = status.parse().unwrap();
            let content_info = ContentInfo {
                username,
                url,
                status,
                caption,
                hashtags,
                original_author,
                original_shortcode,
                last_updated_at,
                url_last_updated_at,
                page_num,
                encountered_errors,
            };

            Ok((MessageId(message_id), content_info))
        })?;

        let mut content_mapping = IndexMap::new();
        for content_info in content_info_iter {
            let (message_id, info) = content_info?;
            content_mapping.insert(message_id, info);
        }

        Ok(content_mapping)
    }

    pub fn get_temp_message_id(&mut self, user_settings: UserSettings) -> i32 {
        let tx = self.conn.transaction().unwrap();
        let mut stmt = tx.prepare("SELECT message_id FROM content_info WHERE username = ?1").unwrap();
        let message_id_iter = stmt
            .query_map([self.username.clone()], |row| {
                let message_id: i32 = row.get(0).unwrap();
                Ok(message_id)
            })
            .unwrap();

        let mut max_message_id = None;
        for message_id in message_id_iter {
            let message_id = message_id.unwrap();
            max_message_id = max_message_id.map_or(Some(message_id), |max: i32| Some(max.max(message_id)));
        }

        let max_message_id = match max_message_id {
            Some(max) => max + 1000,
            None => now_in_my_timezone(user_settings).num_seconds_from_midnight() as i32,
        };

        max_message_id
    }

    pub fn remove_post_from_queue_with_shortcode(&mut self, shortcode: String) -> Result<()> {
        let mut queued_posts = self.load_content_queue().unwrap();
        let user_settings = self.load_user_settings().unwrap();
        let tx = self.conn.transaction()?;

        // Get the rowid of the row with the matching URL
        let rowid: i64 = tx.query_row("SELECT rowid FROM content_queue WHERE original_shortcode = ?1 AND username = ?2", params![shortcode, self.username], |row| row.get(0))?;

        // Create a temporary table with rowids of all rows that should be deleted
        let temp_table_name = format!("to_delete_{}", self.username);
        let sql = format!("CREATE TEMPORARY TABLE {} AS SELECT rowid FROM content_queue WHERE rowid >= ?1 AND username = ?2", temp_table_name);
        tx.execute(&sql, params![rowid, self.username])?;

        // Delete all rows from the original table where the rowid is in the temporary table
        let sql = format!("DELETE FROM content_queue WHERE rowid IN (SELECT rowid FROM {}) AND username = ?1", temp_table_name);
        tx.execute(&sql, params![self.username])?;

        // Drop the temporary table
        let sql = format!("DROP TABLE {}", temp_table_name);
        tx.execute(&sql, [])?;

        tx.commit()?;

        if let Some(removed_post_index) = queued_posts.iter().position(|post| post.original_shortcode == shortcode) {
            // Remove the post from the queued_posts vector
            queued_posts.remove(removed_post_index);

            // Sort queued_posts by will_post_at
            queued_posts.sort_by(|a, b| a.will_post_at.cmp(&b.will_post_at));

            // Recalculate will_post_at for remaining posts
            if removed_post_index <= queued_posts.len() {
                for post in queued_posts.iter_mut().skip(removed_post_index) {
                    let new_post_time = self.get_new_post_time().unwrap();
                    post.will_post_at = new_post_time.clone();
                    post.last_updated_at = (now_in_my_timezone(user_settings.clone()) - INTERFACE_UPDATE_INTERVAL).to_rfc3339();

                    let new_post = QueuedContent {
                        username: post.username.clone(),
                        url: post.url.clone(),
                        caption: post.caption.clone(),
                        hashtags: post.hashtags.clone(),
                        original_author: post.original_author.clone(),
                        original_shortcode: post.original_shortcode.clone(),
                        last_updated_at: post.last_updated_at.clone(),
                        will_post_at: post.will_post_at.clone(),
                    };

                    self.save_content_queue(new_post)?;

                    let (message_id, mut content_info) = self.get_content_info_by_shortcode(post.original_shortcode.clone()).unwrap();
                    if content_info.status.to_string().contains("shown") {
                        content_info.status = ContentStatus::Queued { shown: true };
                    } else {
                        content_info.status = ContentStatus::Queued { shown: false };
                    }
                    self.save_content_mapping(IndexMap::from([(message_id, content_info)]))?;
                }
            }
        }

        Ok(())
    }
    pub fn save_content_queue(&mut self, queued_post: QueuedContent) -> Result<()> {
        let tx = self.conn.transaction()?;

        tx.execute("DELETE FROM content_queue WHERE original_shortcode = ?1 AND username = ?2", params![queued_post.original_shortcode, self.username])?;

        tx.execute(
            "INSERT INTO content_queue (username, url, caption, hashtags, original_author, original_shortcode, last_updated_at, will_post_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                queued_post.username,
                queued_post.url,
                queued_post.caption,
                queued_post.hashtags,
                queued_post.original_author,
                queued_post.original_shortcode,
                queued_post.last_updated_at,
                queued_post.will_post_at
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn load_content_queue(&mut self) -> Result<Vec<QueuedContent>> {
        let tx = self.conn.transaction()?;

        let mut queued_videos_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, last_updated_at, will_post_at FROM content_queue WHERE username = ?1")?;
        let video_queue_iter = queued_videos_stmt.query_map(params![self.username], |row| {
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let last_updated_at: String = row.get(5)?;
            let will_post_at: String = row.get(6)?;

            let queued_post = QueuedContent {
                username: self.username.clone(),
                url,
                caption,
                hashtags,
                original_author,
                original_shortcode,
                last_updated_at,
                will_post_at,
            };

            Ok(queued_post)
        })?;

        let mut queued_posts = Vec::new();
        for queued_post in video_queue_iter {
            let queued_post = queued_post?;
            queued_posts.push(queued_post.clone());
        }

        Ok(queued_posts)
    }

    pub fn get_queued_content_by_shortcode(&mut self, shortcode: String) -> Option<QueuedContent> {
        let content_queue = self.load_content_queue().unwrap();

        for content in content_queue {
            if content.original_shortcode == shortcode {
                return Some(content);
            }
        }
        None
    }

    pub fn get_rejected_content_by_shortcode(&mut self, shortcode: String) -> Option<RejectedContent> {
        let rejected_content = self.load_rejected_content().unwrap();

        for content in rejected_content {
            if content.original_shortcode == shortcode {
                return Some(content);
            }
        }
        None
    }

    pub fn get_failed_content_by_shortcode(&mut self, shortcode: String) -> Option<FailedContent> {
        let failed_content = self.load_failed_content().unwrap();

        for content in failed_content {
            if content.original_shortcode == shortcode {
                return Some(content);
            }
        }
        None
    }

    pub fn get_posted_content_by_shortcode(&mut self, shortcode: String) -> Option<PostedContent> {
        let failed_content = self.load_posted_content().unwrap();

        for content in failed_content {
            if content.original_shortcode == shortcode {
                return Some(content);
            }
        }
        None
    }

    pub fn remove_rejected_content_with_shortcode(&mut self, shortcode: String) -> Result<()> {
        let tx = self.conn.transaction()?;

        // Firstly we remove the posted_content from the content_queue
        tx.execute("DELETE FROM rejected_content WHERE original_shortcode = ?1 AND username = ?2", params![shortcode, self.username])?;

        tx.commit()?;

        Ok(())
    }

    pub fn save_rejected_content(&mut self, rejected_content: RejectedContent) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM rejected_content WHERE original_shortcode = ?1 AND username = ?2", params![rejected_content.original_shortcode, self.username])?;

        tx.execute(
            "INSERT INTO rejected_content (username, url, caption, hashtags, original_author, original_shortcode, rejected_at, last_updated_at, expired) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                rejected_content.username,
                rejected_content.url,
                rejected_content.caption,
                rejected_content.hashtags,
                rejected_content.original_author,
                rejected_content.original_shortcode,
                rejected_content.rejected_at,
                rejected_content.last_updated_at,
                rejected_content.expired
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn load_rejected_content(&mut self) -> Result<Vec<RejectedContent>> {
        let tx = self.conn.transaction()?;

        let mut posted_content_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, rejected_at, last_updated_at, expired FROM rejected_content WHERE username = ?1")?;
        let posted_content_iter = posted_content_stmt.query_map(params![self.username], |row| {
            let username = self.username.clone();
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let rejected_at: String = row.get(5)?;
            let last_updated_at: String = row.get(6)?;
            let expired: bool = row.get(7)?;

            let rejected_content = RejectedContent {
                username,
                url,
                caption,
                hashtags,
                original_author,
                original_shortcode,
                rejected_at,
                last_updated_at,
                expired,
            };

            Ok(rejected_content)
        })?;

        let mut posted_content_list = Vec::new();
        for queued_post in posted_content_iter {
            let queued_post = queued_post?;
            posted_content_list.push(queued_post.clone());
        }

        Ok(posted_content_list)
    }

    /// Save a posted content to the database
    ///
    /// Will automatically remove the content from the content_queue
    pub fn save_posted_content(&mut self, posted_content: PostedContent) -> Result<()> {
        let queued_content = match self.get_queued_content_by_shortcode(posted_content.original_shortcode.clone()) {
            None => None,
            Some(post) => Some(post),
        };

        let mut removed = false;

        if let Some(queued_content) = queued_content {
            let user_settings = self.load_user_settings().unwrap();
            let posting_interval = Duration::try_seconds(user_settings.posting_interval * 60).unwrap();
            if DateTime::parse_from_rfc3339(&queued_content.will_post_at).unwrap() < now_in_my_timezone(user_settings) - posting_interval {
                // If so, we remove the post from the queue using this function, since it also recalculates the will_post_at for the remaining posts
                // And will avoid content being posted all at once
                self.remove_post_from_queue_with_shortcode(posted_content.original_shortcode.clone())?;
                removed = true;
            }
        }

        // Check if the post was supposed to be posted in the past, more than the posting interval ago

        let tx = self.conn.transaction()?;

        // Otherwise we remove the post from the queue with the tx to avoid recalculating the will_post_at of the other posts
        // This is what the "normal" behavior should be, the above will only happen if the bot was offline for a long time
        if !removed {
            // Firstly we remove the posted_content from the content_queue
            tx.execute("DELETE FROM content_queue WHERE original_shortcode = ?1 AND username = ?2", params![posted_content.original_shortcode, self.username])?;
        }

        // We remove the posted_content if it is already there
        tx.execute("DELETE FROM posted_content WHERE original_shortcode = ?1 AND username = ?2", params![posted_content.original_shortcode, self.username])?;

        // Then we add the posted_content to the posted_content table
        tx.execute(
            "INSERT INTO posted_content (username, url, caption, hashtags, original_author, original_shortcode, posted_at, last_updated_at, expired) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.username,
                posted_content.url,
                posted_content.caption,
                posted_content.hashtags,
                posted_content.original_author,
                posted_content.original_shortcode,
                posted_content.posted_at,
                posted_content.last_updated_at,
                posted_content.expired
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn load_posted_content(&mut self) -> Result<Vec<PostedContent>> {
        let tx = self.conn.transaction()?;

        let mut posted_content_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, posted_at, last_updated_at, expired FROM posted_content WHERE username = ?1")?;
        let posted_content_iter = posted_content_stmt.query_map(params![self.username], |row| {
            let username = self.username.clone();
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let posted_at: String = row.get(5)?;
            let last_updated_at: String = row.get(6)?;
            let expired: bool = row.get(7)?;

            let queued_post = PostedContent {
                username,
                url,
                caption,
                hashtags,
                original_author,
                original_shortcode,
                posted_at,
                last_updated_at,
                expired,
            };

            Ok(queued_post)
        })?;

        let mut posted_content_list = Vec::new();
        for queued_post in posted_content_iter {
            let queued_post = queued_post?;
            posted_content_list.push(queued_post.clone());
        }

        Ok(posted_content_list)
    }

    /// Save a content that failed to upload to the database
    ///
    /// Will automatically remove the content from the content_queue
    pub fn save_failed_content(&mut self, failed_content: FailedContent) -> Result<()> {
        // First we check if the content is actually in the content_queue
        let mut exists = false;
        {
            let queued_posts = self.load_content_queue()?;
            for post in queued_posts {
                if post.original_shortcode == failed_content.original_shortcode {
                    exists = true;
                    break;
                }
            }
        }

        //println!("Failed content exists in queue: {}", exists);

        if exists {
            // we remove the failed_content from the content_queue using this function
            // Since it also automatically recalculates the will_post_at for the remaining posts
            self.remove_post_from_queue_with_shortcode(failed_content.original_shortcode.clone())?;
        }

        // Then we add the failed_content to the failed_content table
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO failed_content (username, url, caption, hashtags, original_author, original_shortcode, last_updated_at, failed_at, expired) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.username,
                failed_content.url,
                failed_content.caption,
                failed_content.hashtags,
                failed_content.original_author,
                failed_content.original_shortcode,
                failed_content.last_updated_at,
                failed_content.failed_at,
                failed_content.expired
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    /// This function updates a matching failed content with the new failed content
    pub fn update_failed_content(&mut self, failed_content: FailedContent) -> Result<()> {
        let tx = self.conn.transaction()?;
        let username = self.username.clone();
        tx.execute(
            "UPDATE failed_content SET url = ?1, caption = ?2, hashtags = ?3, original_author = ?4, last_updated_at = ?5, failed_at = ?6, expired = ?7 WHERE original_shortcode = ?8 AND username = ?9",
            params![
                failed_content.url,
                failed_content.caption,
                failed_content.hashtags,
                failed_content.original_author,
                failed_content.last_updated_at,
                failed_content.failed_at,
                failed_content.expired,
                failed_content.original_shortcode,
                username
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn load_failed_content(&mut self) -> Result<Vec<FailedContent>> {
        let tx = self.conn.transaction()?;

        let mut failed_content_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, last_updated_at, failed_at, expired FROM failed_content WHERE username = ?1")?;
        let failed_content_iter = failed_content_stmt.query_map(params![self.username], |row| {
            let username = self.username.clone();
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let last_updated_at: String = row.get(5)?;
            let failed_at: String = row.get(6)?;
            let expired: bool = row.get(7)?;

            let failed_content = FailedContent {
                username,
                url,
                caption,
                hashtags,
                original_author,
                original_shortcode,
                last_updated_at,
                failed_at,
                expired,
            };

            Ok(failed_content)
        })?;

        let mut failed_content_list = Vec::new();
        for failed_content in failed_content_iter {
            let failed_content = failed_content?;
            failed_content_list.push(failed_content.clone());
        }

        Ok(failed_content_list)
    }

    pub fn get_new_post_time(&mut self) -> Result<String, rusqlite::Error> {
        let user_settings = self.load_user_settings()?;

        let posted_content = self.load_posted_content()?;
        let queued_content = self.load_content_queue()?;

        let current_time = now_in_my_timezone(user_settings.clone());

        // Get all the post times
        let mut post_times = Vec::new();
        for post in &posted_content {
            let post_time = DateTime::parse_from_rfc3339(&post.posted_at).unwrap().with_timezone(&Utc);
            post_times.push(post_time);
        }
        for post in &queued_content {
            let post_time = DateTime::parse_from_rfc3339(&post.will_post_at).unwrap().with_timezone(&Utc);
            post_times.push(post_time);
        }

        post_times.sort();

        let posting_interval = Duration::try_seconds(user_settings.posting_interval * 60).unwrap();
        // Filter out the post times that are before the current time
        post_times = post_times.into_iter().filter(|&time| time >= current_time - posting_interval).collect();

        let random_interval = user_settings.random_interval_variance * 60;
        let mut rng = rand::thread_rng();
        let random_variance = rng.gen_range(-random_interval..=random_interval);

        let randomized_posting_interval = Duration::try_seconds(user_settings.posting_interval * 60 + random_variance).unwrap();

        // Find the first gap in the post times
        for windows in post_times.windows(2) {
            let gap = windows[1] - windows[0];
            if gap > posting_interval + Duration::try_seconds(random_interval).unwrap() {
                let new_post_time = windows[0] + randomized_posting_interval;
                tracing::info!("Gap found, new post time: {}", new_post_time.to_rfc3339());
                return Ok(new_post_time.to_rfc3339());
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

        Ok(new_post_time.to_rfc3339())
    }

    pub fn does_content_exist_with_shortcode(&mut self, shortcode: String) -> bool {
        // Execute each statement and check if the URL exists
        let tables = ["content_info", "posted_content", "content_queue", "rejected_content", "failed_content"];
        let exists = tables.iter().any(|table| self.shortcode_exists_in_table(table, &shortcode));

        exists
    }

    fn shortcode_exists_in_table(&mut self, table_name: &str, shortcode: &str) -> bool {
        let tx = self.conn.transaction().unwrap();
        let mut stmt = tx.prepare(&format!("SELECT url FROM {} WHERE original_shortcode = ?1 AND username = ?2", table_name)).unwrap();
        let exists = stmt.query_map(params![shortcode, self.username], |row| Ok(row.get::<_, String>(0)?)).unwrap().next().is_some();
        exists
    }

    pub fn reorder_pages(&mut self) -> Result<()> {
        let user_settings = self.load_user_settings()?;
        let page_size = user_settings.page_size as i64;

        // Load all content
        let mut content_mapping = self.load_content_mapping()?;

        // Sort content by page_num
        content_mapping.sort_by(|_a, b, _c, d| b.page_num.cmp(&d.page_num).reverse());

        // Recalculate page numbers
        let mut new_content_info = IndexMap::new();
        let mut index = 0;
        for (message_id, mut info) in content_mapping {
            let correct_page_num = ((index as f64 / page_size as f64) + 0.5).floor() as i32;
            info.page_num = correct_page_num;
            new_content_info.insert(message_id, info);
            index += 1;
        }

        // Save the updated content info back to the database
        self.save_content_mapping(new_content_info)?;

        Ok(())
    }
}
