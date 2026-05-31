//! LFS schema definitions for lock, batch, and object structures.
//!
//! LFS 锁、批量和对象结构的架构定义。

use serde::Serialize;

use crate::lfs_structs::Lock;

#[derive(Debug, Clone, Default, Serialize)]
pub struct LfsOutput {
    pub action: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locks: Vec<Lock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<LfsFileOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refspec: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub name_only: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub show_size: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LfsFileOutput {
    pub path: String,
    /// Display oid: full 64-char hash when `--long`, otherwise the first 10.
    /// Backward-compatible — preserves the existing JSON contract.
    pub oid: String,
    /// Full 64-char LFS oid, always. Lets `--json` consumers read the canonical
    /// hash without having to also pass `--long`.
    pub full_oid: String,
    pub marker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_size: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LfsUploadSummary {
    pub files_uploaded: usize,
}

const fn is_false(value: &bool) -> bool {
    !*value
}
