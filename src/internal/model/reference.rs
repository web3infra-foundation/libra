use serde::{Deserialize, Serialize};
use std::io::Error as IOError;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: i64,
    pub name: Option<String>,
    pub kind: ConfigKind, // type is a reserved keyword
    pub commit: Option<String>,
    pub remote: Option<String>, // None for local, Some for remote, '' is not valid
}

impl Model {
    /// Parse a Model from a Turso row
    pub fn from_row(row: &turso::Row) -> Result<Self, IOError> {
        let kind_str: String = row
            .get::<String>(2)
            .map_err(|e: turso::Error| IOError::other(format!("Parse kind error: {e:?}")))?;
        let kind = ConfigKind::from_str(&kind_str)?;

        Ok(Self {
            id: row
                .get::<i64>(0)
                .map_err(|e| IOError::other(format!("Parse id error: {e:?}")))?,
            name: row
                .get::<Option<String>>(1)
                .map_err(|e| IOError::other(format!("Parse name error: {e:?}")))?,
            kind,
            commit: row
                .get::<Option<String>>(3)
                .map_err(|e| IOError::other(format!("Parse commit error: {e:?}")))?,
            remote: row
                .get::<Option<String>>(4)
                .map_err(|e| IOError::other(format!("Parse remote error: {e:?}")))?,
        })
    }
}

/// kind enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigKind {
    Branch, // .git/refs/heads
    Tag,    // .git/refs/tags
    Head,   // .git/HEAD
}

impl ConfigKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigKind::Branch => "Branch",
            ConfigKind::Tag => "Tag",
            ConfigKind::Head => "Head",
        }
    }
}

impl FromStr for ConfigKind {
    type Err = IOError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Branch" => Ok(ConfigKind::Branch),
            "Tag" => Ok(ConfigKind::Tag),
            "Head" => Ok(ConfigKind::Head),
            _ => Err(IOError::other(format!("Invalid ConfigKind: {s}"))),
        }
    }
}
