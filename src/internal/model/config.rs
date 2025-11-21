use serde::{Deserialize, Serialize};
use std::io::Error as IOError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: i64,
    // [configuration "name"]=>[remote "origin"]
    pub configuration: String, // configuration option
    pub name: Option<String>,  // name of the configuration (optionally)
    pub key: String,
    pub value: String,
}

impl Model {
    /// Parse a Model from a Turso row
    pub fn from_row(row: &turso::Row) -> Result<Self, IOError> {
        Ok(Self {
            id: row
                .get::<i64>(0)
                .map_err(|e| IOError::other(format!("Parse id error: {e:?}")))?,
            configuration: row
                .get::<String>(1)
                .map_err(|e| IOError::other(format!("Parse configuration error: {e:?}")))?,
            name: row
                .get::<Option<String>>(2)
                .map_err(|e| IOError::other(format!("Parse name error: {e:?}")))?,
            key: row
                .get::<String>(3)
                .map_err(|e| IOError::other(format!("Parse key error: {e:?}")))?,
            value: row
                .get::<String>(4)
                .map_err(|e| IOError::other(format!("Parse value error: {e:?}")))?,
        })
    }
}
