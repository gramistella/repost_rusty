use chrono::{DateTime, Duration, Utc};

use crate::database::UserSettings;

pub fn now_in_my_timezone(user_settings: UserSettings) -> DateTime<Utc> {
    let utc_now = Utc::now();
    let timezone_offset = Duration::try_hours(user_settings.timezone_offset as i64).unwrap();
    utc_now + timezone_offset
}
