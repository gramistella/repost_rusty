use std::error::Error;
use std::fmt;
use std::str::FromStr;

use serde::de::Visitor;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, PartialEq, Debug)]
pub enum ContentStatus {
    Waiting,
    RemovedFromView,
    Pending { shown: bool },
    Published { shown: bool },
    Queued { shown: bool },
    Rejected { shown: bool },
    Failed { shown: bool },
}

impl Serialize for ContentStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let status = get_status_string(self.clone());
        serializer.serialize_str(&status)
    }
}

#[derive(Debug, Clone)]
pub struct ContentStatusParseError;

impl fmt::Display for ContentStatusParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "could not parse the provided string")
    }
}

impl Error for ContentStatusParseError {}

impl FromStr for ContentStatus {
    type Err = ContentStatusParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "waiting" => Ok(ContentStatus::Waiting),
            "pending_shown" => Ok(ContentStatus::Pending { shown: true }),
            "pending_hidden" => Ok(ContentStatus::Pending { shown: false }),
            "published_shown" => Ok(ContentStatus::Published { shown: true }),
            "published_hidden" => Ok(ContentStatus::Published { shown: false }),
            "queued_shown" => Ok(ContentStatus::Queued { shown: true }),
            "queued_hidden" => Ok(ContentStatus::Queued { shown: false }),
            "rejected_shown" => Ok(ContentStatus::Rejected { shown: true }),
            "rejected_hidden" => Ok(ContentStatus::Rejected { shown: false }),
            "failed_shown" => Ok(ContentStatus::Failed { shown: true }),
            "failed_hidden" => Ok(ContentStatus::Failed { shown: false }),
            "removed_from_view" => Ok(ContentStatus::RemovedFromView),
            _ => Err(ContentStatusParseError),
        }
    }
}

impl fmt::Display for ContentStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", get_status_string(self.clone()))
    }
}

struct ContentStatusVisitor;

impl<'de> Visitor<'de> for ContentStatusVisitor {
    type Value = ContentStatus;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string representing ContentStatus")
    }

    fn visit_str<E>(self, value: &str) -> Result<ContentStatus, E>
    where
        E: de::Error,
    {
        match value {
            "waiting" => Ok(ContentStatus::Waiting),
            "pending_shown" => Ok(ContentStatus::Pending { shown: true }),
            "pending_hidden" => Ok(ContentStatus::Pending { shown: false }),
            "published_shown" => Ok(ContentStatus::Published { shown: true }),
            "published_hidden" => Ok(ContentStatus::Published { shown: false }),
            "queued_shown" => Ok(ContentStatus::Queued { shown: true }),
            "queued_hidden" => Ok(ContentStatus::Queued { shown: false }),
            "rejected_shown" => Ok(ContentStatus::Rejected { shown: true }),
            "rejected_hidden" => Ok(ContentStatus::Rejected { shown: false }),
            "failed_shown" => Ok(ContentStatus::Failed { shown: true }),
            "failed_hidden" => Ok(ContentStatus::Failed { shown: false }),
            _ => Err(de::Error::unknown_variant(
                value,
                &["waiting", "pending_shown", "pending_hidden", "published_shown", "published_hidden", "queued_shown", "queued_hidden", "rejected_shown", "rejected_hidden", "failed_shown", "failed_hidden"],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ContentStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ContentStatusVisitor)
    }
}

fn get_status_string(content_status: ContentStatus) -> String {
    match content_status {
        ContentStatus::Waiting => "waiting".to_string(),
        ContentStatus::RemovedFromView => "removed_from_view".to_string(),
        ContentStatus::Pending { shown } => {
            if shown {
                "pending_shown".to_string()
            } else {
                "pending_hidden".to_string()
            }
        }
        ContentStatus::Published { shown } => {
            if shown {
                "published_shown".to_string()
            } else {
                "published_hidden".to_string()
            }
        }
        ContentStatus::Queued { shown } => {
            if shown {
                "queued_shown".to_string()
            } else {
                "queued_hidden".to_string()
            }
        }
        ContentStatus::Rejected { shown } => {
            if shown {
                "rejected_shown".to_string()
            } else {
                "rejected_hidden".to_string()
            }
        }
        ContentStatus::Failed { shown } => {
            if shown {
                "failed_shown".to_string()
            } else {
                "failed_hidden".to_string()
            }
        }
    }
}
