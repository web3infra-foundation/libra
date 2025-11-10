use serde::{Deserialize, Serialize};
use std::io::Error as IOError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: i64,
    pub ref_name: String,
    pub old_oid: String,
    pub new_oid: String,
    pub timestamp: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub action: String,
    pub message: String,
}

impl Model {
    /// Parse a Model from a Turso row
    pub fn from_row(row: &turso::Row) -> Result<Self, IOError> {
        Ok(Self {
            id: row
                .get::<i64>(0)
                .map_err(|e| IOError::other(format!("Parse id error: {e:?}")))?,
            ref_name: row
                .get::<String>(1)
                .map_err(|e| IOError::other(format!("Parse ref_name error: {e:?}")))?,
            old_oid: row
                .get::<String>(2)
                .map_err(|e| IOError::other(format!("Parse old_oid error: {e:?}")))?,
            new_oid: row
                .get::<String>(3)
                .map_err(|e| IOError::other(format!("Parse new_oid error: {e:?}")))?,
            committer_name: row
                .get::<String>(4)
                .map_err(|e| IOError::other(format!("Parse committer_name error: {e:?}")))?,
            committer_email: row
                .get::<String>(5)
                .map_err(|e| IOError::other(format!("Parse committer_email error: {e:?}")))?,
            timestamp: row
                .get::<i64>(6)
                .map_err(|e| IOError::other(format!("Parse timestamp error: {e:?}")))?,
            action: row
                .get::<String>(7)
                .map_err(|e| IOError::other(format!("Parse action error: {e:?}")))?,
            message: row
                .get::<String>(8)
                .map_err(|e| IOError::other(format!("Parse message error: {e:?}")))?,
        })
    }
}
