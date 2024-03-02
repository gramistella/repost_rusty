use chrono::{DateTime, Duration, FixedOffset, ParseError, Timelike, Utc};
use indexmap::IndexMap;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rand::Rng;

use teloxide::types::MessageId;

use crate::utils::now_in_my_timezone;
use rusqlite::{params, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct UserSettings {
    pub can_post: bool,
    pub posting_interval: i64,
    pub random_interval_variance: i64,
    pub rejected_content_lifespan: i64,
    pub posted_content_lifespan: i64,
    pub timezone_offset: i32,
}

#[derive(Clone, PartialEq)]
pub struct QueuedContent {
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
    pub url: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub failed_at: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct VideoInfo {
    pub url: String,
    pub status: String,
    pub caption: String,
    pub hashtags: String,
    pub original_author: String,
    pub original_shortcode: String,
    pub last_updated_at: String,
    pub encountered_errors: i32,
}

const PROD_DB: &str = "db/prod.db";
const DEV_DB: &str = "db/dev.db";

pub(crate) struct Database {
    pool: Pool<SqliteConnectionManager>,
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Database { pool: self.pool.clone() }
    }
}

impl Database {
    pub fn new(is_offline: bool) -> Result<Self> {
        let manager = if is_offline { SqliteConnectionManager::file(DEV_DB) } else { SqliteConnectionManager::file(PROD_DB) };

        let pool = r2d2::Pool::new(manager).unwrap();

        let conn = pool.get().unwrap();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_settings (
            can_post INTEGER NOT NULL,
            posting_interval INTEGER NOT NULL,
            random_interval_variance INTEGER NOT NULL,
            rejected_content_lifespan INTEGER NOT NULL,
            posted_content_lifespan INTEGER NOT NULL,
            timezone_offset INTEGER NOT NULL
        )",
            [],
        )?;

        let default_timezone_offset = 1;
        if is_offline {
            let default_is_posting = 1;
            let default_posting_interval = 1;
            let default_random_interval = 0;
            let default_removed_content_lifespan = 2;
            let default_posted_content_lifespan = 2;

            let query = format!(
                "INSERT INTO user_settings (can_post, posting_interval, random_interval_variance, rejected_content_lifespan, posted_content_lifespan, timezone_offset) VALUES ({}, {}, {}, {}, {}, {})",
                default_is_posting, default_posting_interval, default_random_interval, default_removed_content_lifespan, default_posted_content_lifespan, default_timezone_offset
            );
            conn.execute(&query, [])?;
        } else {
            let default_is_posting = 1;
            let default_posting_interval = 180;
            let default_random_interval = 30;
            let default_removed_content_lifespan = 120;
            let default_posted_content_lifespan = 180;

            let query = format!(
                "INSERT INTO user_settings (can_post, posting_interval, random_interval_variance, rejected_content_lifespan, posted_content_lifespan, timezone_offset) VALUES ({}, {}, {}, {}, {}, {})",
                default_is_posting, default_posting_interval, default_random_interval, default_removed_content_lifespan, default_posted_content_lifespan, default_timezone_offset
            );
            conn.execute(&query, [])?;
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS content_info (
            message_id INTEGER PRIMARY KEY,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            encountered_errors INTEGER NOT NULL,
            last_updated_at TEXT NOT NULL
        )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS content_queue (
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
            url TEXT NOT NULL,
            caption TEXT NOT NULL,
            hashtags TEXT NOT NULL,
            original_author TEXT NOT NULL,
            original_shortcode TEXT NOT NULL,
            last_updated_at TEXT NOT NULL,
            failed_at TEXT NOT NULL
        )",
            [],
        )?;

        Ok(Database { pool })
    }
    pub fn begin_transaction(&self) -> Result<DatabaseTransaction> {
        let conn = self.pool.get().unwrap();
        Ok(DatabaseTransaction { conn })
    }
}

pub struct DatabaseTransaction {
    conn: PooledConnection<SqliteConnectionManager>,
}

impl DatabaseTransaction {
    pub fn load_user_settings(&mut self) -> Result<UserSettings> {
        let mut can_post: Option<bool> = None;
        let mut posting_interval: Option<i64> = None;
        let mut random_interval_variance: Option<i64> = None;
        let mut rejected_content_lifespan: Option<i64> = None;
        let mut posted_content_lifespan: Option<i64> = None;
        let mut timezone_offset: Option<i32> = None;
        let tx = self.conn.transaction()?;

        tx.query_row("SELECT can_post, posting_interval, random_interval_variance, rejected_content_lifespan, posted_content_lifespan, timezone_offset FROM user_settings", [], |row| {
            can_post = Some(row.get(0)?);
            posting_interval = Some(row.get(1)?);
            random_interval_variance = Some(row.get(2)?);
            rejected_content_lifespan = Some(row.get(3)?);
            posted_content_lifespan = Some(row.get(4)?);
            timezone_offset = Some(row.get(5)?);
            Ok(())
        })?;

        let mut queued_videos_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, last_updated_at, will_post_at FROM content_queue")?;
        let video_queue_iter = queued_videos_stmt.query_map([], |row| {
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let last_updated_at: String = row.get(5)?;
            let will_post_at: String = row.get(6)?;

            let queued_post = QueuedContent {
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

        let user_settings = UserSettings {
            can_post: can_post.unwrap(),
            posting_interval: posting_interval.unwrap(),
            random_interval_variance: random_interval_variance.unwrap(),
            rejected_content_lifespan: rejected_content_lifespan.unwrap(),
            posted_content_lifespan: posted_content_lifespan.unwrap(),
            timezone_offset: timezone_offset.unwrap(),
        };

        Ok(user_settings)
    }

    pub fn save_user_settings(&mut self, user_settings: UserSettings) -> Result<()> {
        let tx = self.conn.transaction()?;
        // Update user settings
        tx.execute(
            "UPDATE user_settings SET can_post = ?1, posting_interval = ?2, random_interval_variance = ?3, rejected_content_lifespan = ?4, posted_content_lifespan = ?5, timezone_offset = ?6",
            params![
                user_settings.can_post as i64,
                user_settings.posting_interval,
                user_settings.random_interval_variance,
                user_settings.rejected_content_lifespan,
                user_settings.posted_content_lifespan,
                user_settings.timezone_offset
            ],
        )?;

        tx.commit()?;

        Ok(())
    }
    pub fn get_content_info_by_message_id(&mut self, message_id: MessageId) -> Option<VideoInfo> {
        let video_mapping = self.load_content_mapping().unwrap();

        match video_mapping.get(&message_id) {
            Some(content_info) => Some(content_info.clone()),
            None => None,
        }
    }

    pub fn get_content_info_by_shortcode(&mut self, shortcode: String) -> Option<(MessageId, VideoInfo)> {
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

        // Firstly we remove the posted_content from the content_queue
        tx.execute("DELETE FROM content_info WHERE original_shortcode = ?1", params![shortcode])?;

        tx.commit()?;

        Ok(())
    }

    pub fn save_content_mapping(&mut self, video_mapping: IndexMap<MessageId, VideoInfo>) -> Result<()> {
        let existing_mapping = self.load_content_mapping()?;
        let tx = self.conn.transaction()?;
        for (new_key, new_value) in video_mapping {
            for (existing_key, existing_value) in &existing_mapping {
                if existing_value.original_shortcode == new_value.original_shortcode {
                    tx.execute("DELETE FROM content_info WHERE message_id = ?1", params![existing_key.0])?;
                }
            }

            tx.execute(
                "INSERT OR REPLACE INTO content_info (message_id, url, status, caption, hashtags, original_author, original_shortcode, last_updated_at, encountered_errors) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    new_key.0,
                    new_value.url,
                    new_value.status,
                    new_value.caption,
                    new_value.hashtags,
                    new_value.original_author,
                    new_value.original_shortcode,
                    new_value.last_updated_at,
                    new_value.encountered_errors
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn load_content_mapping(&mut self) -> Result<IndexMap<MessageId, VideoInfo>> {
        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare("SELECT message_id, url, status, caption, hashtags, original_author, original_shortcode, last_updated_at, encountered_errors FROM content_info")?;
        let content_info_iter = stmt.query_map([], |row| {
            // let message_id: String = row.get(0)?;
            let url: String = row.get(1)?;
            let status: String = row.get(2)?;
            let caption: String = row.get(3)?;
            let hashtags: String = row.get(4)?;
            let original_author: String = row.get(5)?;
            let original_shortcode: String = row.get(6)?;
            let last_updated_at: String = row.get(7)?;
            let encountered_errors: i32 = row.get(8)?;

            let content_info = VideoInfo {
                url,
                status,
                caption,
                hashtags,
                original_author,
                original_shortcode,
                last_updated_at,
                encountered_errors,
            };

            Ok((MessageId(row.get(0)?), content_info))
        })?;

        let mut video_mapping = IndexMap::new();
        for content_info in content_info_iter {
            let (message_id, info) = content_info?;
            video_mapping.insert(message_id, info);
        }

        Ok(video_mapping)
    }

    pub fn get_temp_message_id(&mut self, user_settings: UserSettings) -> i32 {
        let tx = self.conn.transaction().unwrap();
        let mut stmt = tx.prepare("SELECT message_id FROM content_info").unwrap();
        let message_id_iter = stmt
            .query_map([], |row| {
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
        let rowid: i64 = tx.query_row("SELECT rowid FROM content_queue WHERE original_shortcode = ?1", params![shortcode], |row| row.get(0))?;

        // Create a temporary table with rowids of all rows that should be deleted
        tx.execute("CREATE TEMPORARY TABLE to_delete AS SELECT rowid FROM content_queue WHERE rowid >= ?1", params![rowid])?;

        // Delete all rows from the original table where the rowid is in the temporary table
        tx.execute("DELETE FROM content_queue WHERE rowid IN (SELECT rowid FROM to_delete)", [])?;

        // Drop the temporary table
        tx.execute("DROP TABLE to_delete", [])?;

        tx.commit()?;

        if let Some(removed_post_index) = queued_posts.iter().position(|post| post.original_shortcode == shortcode) {
            // Remove the post from the queued_posts vector
            queued_posts.remove(removed_post_index);

            // Recalculate will_post_at for remaining posts
            if removed_post_index < queued_posts.len() {
                for post in queued_posts.iter_mut().skip(removed_post_index) {
                    let new_post_time = self.get_new_post_time(user_settings.clone()).unwrap();
                    post.will_post_at = new_post_time.clone();

                    let new_post = QueuedContent {
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
                    content_info.status = "queued_hidden".to_string();
                    self.save_content_mapping(IndexMap::from([(message_id, content_info)]))?;
                }
            }
        }

        Ok(())
    }

    pub fn save_content_queue(&mut self, queued_post: QueuedContent) -> Result<()> {
        let tx = self.conn.transaction()?;

        tx.execute("DELETE FROM content_queue WHERE original_shortcode = ?1", params![queued_post.original_shortcode])?;

        tx.execute(
            "INSERT INTO content_queue (url, caption, hashtags, original_author, original_shortcode, last_updated_at, will_post_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![queued_post.url, queued_post.caption, queued_post.hashtags, queued_post.original_author, queued_post.original_shortcode, queued_post.last_updated_at, queued_post.will_post_at],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn load_content_queue(&mut self) -> Result<Vec<QueuedContent>> {
        let tx = self.conn.transaction()?;

        let mut queued_videos_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, last_updated_at, will_post_at FROM content_queue")?;
        let video_queue_iter = queued_videos_stmt.query_map([], |row| {
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let last_updated_at: String = row.get(5)?;
            let will_post_at: String = row.get(6)?;

            let queued_post = QueuedContent {
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

    pub fn remove_rejected_content_with_shortcode(&mut self, shortcode: String) -> Result<()> {
        let tx = self.conn.transaction()?;

        // Firstly we remove the posted_content from the content_queue
        tx.execute("DELETE FROM rejected_content WHERE original_shortcode = ?1", params![shortcode])?;

        tx.commit()?;

        Ok(())
    }

    pub fn save_rejected_content(&mut self, rejected_content: RejectedContent) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM rejected_content WHERE original_shortcode = ?1", params![rejected_content.original_shortcode])?;

        tx.execute(
            "INSERT INTO rejected_content (url, caption, hashtags, original_author, original_shortcode, rejected_at, last_updated_at, expired) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
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

        let mut posted_content_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, rejected_at, last_updated_at, expired FROM rejected_content")?;
        let posted_content_iter = posted_content_stmt.query_map([], |row| {
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let rejected_at: String = row.get(5)?;
            let last_updated_at: String = row.get(6)?;
            let expired: bool = row.get(7)?;

            let rejected_content = RejectedContent {
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
        let tx = self.conn.transaction()?;

        // Firstly we remove the posted_content from the content_queue
        tx.execute("DELETE FROM content_queue WHERE original_shortcode = ?1", params![posted_content.original_shortcode])?;

        // We remove the posted_content if it is already there
        tx.execute("DELETE FROM posted_content WHERE original_shortcode = ?1", params![posted_content.original_shortcode])?;

        // Then we add the posted_content to the posted_content table
        tx.execute(
            "INSERT INTO posted_content (url, caption, hashtags, original_author, original_shortcode, posted_at, last_updated_at, expired) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
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

        let mut posted_content_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, posted_at, last_updated_at, expired FROM posted_content")?;
        let posted_content_iter = posted_content_stmt.query_map([], |row| {
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let posted_at: String = row.get(5)?;
            let last_updated_at: String = row.get(6)?;
            let expired: bool = row.get(7)?;

            let queued_post = PostedContent {
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
        {
            // Firstly we remove the failed_content from the content_queue using this method
            // Since this one also automatically recalculates the will_post_at for the remaining posts
            self.remove_post_from_queue_with_shortcode(failed_content.original_shortcode.clone())?;
        }
        // Then we add the posted_content to the posted_content table
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO failed_content (url, caption, hashtags, original_author, original_shortcode, last_updated_at, failed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                failed_content.url,
                failed_content.caption,
                failed_content.hashtags,
                failed_content.original_author,
                failed_content.original_shortcode,
                failed_content.last_updated_at,
                failed_content.failed_at
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    /// This function updates a matching failed content with the new failed content
    pub fn update_failed_content(&mut self, failed_content: FailedContent) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE failed_content SET url = ?1, caption = ?2, hashtags = ?3, original_author = ?4, last_updated_at = ?5, failed_at = ?6 WHERE original_shortcode = ?7",
            params![
                failed_content.url,
                failed_content.caption,
                failed_content.hashtags,
                failed_content.original_author,
                failed_content.last_updated_at,
                failed_content.failed_at,
                failed_content.original_shortcode
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn load_failed_content(&mut self) -> Result<Vec<FailedContent>> {
        let tx = self.conn.transaction()?;

        let mut failed_content_stmt = tx.prepare("SELECT url, caption, hashtags, original_author, original_shortcode, last_updated_at, failed_at FROM failed_content")?;
        let failed_content_iter = failed_content_stmt.query_map([], |row| {
            let url: String = row.get(0)?;
            let caption: String = row.get(1)?;
            let hashtags: String = row.get(2)?;
            let original_author: String = row.get(3)?;
            let original_shortcode: String = row.get(4)?;
            let last_updated_at: String = row.get(5)?;
            let failed_at: String = row.get(6)?;

            let failed_content = FailedContent {
                url,
                caption,
                hashtags,
                original_author,
                original_shortcode,
                last_updated_at,
                failed_at,
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

    pub fn get_new_post_time(&mut self, user_settings: UserSettings) -> std::result::Result<String, ParseError> {
        let tx = self.conn.transaction().unwrap();
        let mut stmt = tx.prepare("SELECT will_post_at FROM content_queue ORDER BY will_post_at DESC LIMIT 1").unwrap();

        let mut latest_post_time_iter = stmt
            .query_map([], |row| {
                let will_post_at: String = row.get(0)?;
                let post_time: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(&will_post_at).unwrap();
                Ok(post_time.with_timezone(&Utc))
            })
            .unwrap();

        let posting_interval = Duration::seconds(user_settings.posting_interval * 60);
        let random_interval = Duration::seconds(user_settings.random_interval_variance * 60);

        let mut rng = rand::thread_rng();
        let random_variance = rng.gen_range(-random_interval.num_seconds()..=random_interval.num_seconds());
        let random_variance_seconds = Duration::seconds(random_variance);

        let mut new_post_time: DateTime<Utc> = match latest_post_time_iter.next() {
            Some(Ok(time)) => time + posting_interval + random_variance_seconds,
            _ => {
                let mut stmt = tx.prepare("SELECT posted_at FROM posted_content ORDER BY posted_at DESC LIMIT 1").unwrap();
                let mut latest_posted_time_iter = stmt
                    .query_map([], |row| {
                        let posted_at: String = row.get(0)?;
                        let post_time: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(&posted_at).unwrap();
                        Ok(post_time.with_timezone(&Utc))
                    })
                    .unwrap();

                match latest_posted_time_iter.next() {
                    Some(Ok(time)) => time + posting_interval + random_variance_seconds,
                    _ => now_in_my_timezone(user_settings.clone()) + Duration::seconds(60),
                }
            }
        };

        // Check if the new post time is in the past
        if new_post_time < now_in_my_timezone(user_settings.clone()) {
            new_post_time = now_in_my_timezone(user_settings.clone()) + Duration::seconds(60)
        }

        Ok(new_post_time.to_rfc3339())
    }

    pub fn does_content_exist_with_shortcode(&mut self, shortcode: String) -> bool {
        let tx = self.conn.transaction().unwrap();

        // Prepare statements for each table
        let mut stmt_content_info = tx.prepare("SELECT url FROM content_info WHERE original_shortcode = ?1").unwrap();
        let mut stmt_posted_content = tx.prepare("SELECT url FROM posted_content WHERE original_shortcode = ?1").unwrap();
        let mut stmt_content_queue = tx.prepare("SELECT url FROM content_queue WHERE original_shortcode = ?1").unwrap();
        let mut stmt_rejected_content = tx.prepare("SELECT url FROM rejected_content WHERE original_shortcode = ?1").unwrap();
        let mut stmt_failed_content = tx.prepare("SELECT url FROM failed_content WHERE original_shortcode = ?1").unwrap();

        // Execute each statement and check if the URL exists
        let exists_in_content_info = stmt_content_info.query_map(params![shortcode.clone()], |row| Ok(row.get::<_, String>(0)?)).unwrap().next().is_some();
        let exists_in_posted_content = stmt_posted_content.query_map(params![shortcode.clone()], |row| Ok(row.get::<_, String>(0)?)).unwrap().next().is_some();
        let exists_in_content_queue = stmt_content_queue.query_map(params![shortcode.clone()], |row| Ok(row.get::<_, String>(0)?)).unwrap().next().is_some();
        let exists_in_rejected_content = stmt_rejected_content.query_map(params![shortcode], |row| Ok(row.get::<_, String>(0)?)).unwrap().next().is_some();
        let exists_in_failed_content = stmt_failed_content.query_map(params![shortcode], |row| Ok(row.get::<_, String>(0)?)).unwrap().next().is_some();

        // Return true if the URL is found in any table
        exists_in_content_info || exists_in_posted_content || exists_in_content_queue || exists_in_rejected_content || exists_in_failed_content
    }
}