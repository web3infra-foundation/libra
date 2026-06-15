use std::collections::BTreeMap;

use git_internal::{
    hash::ObjectHash,
    internal::{object::types::ObjectType, pack::pack_index::IndexEntry},
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VerifyPackOutput {
    pub(crate) idx_file: String,
    pub(crate) pack_file: String,
    pub(crate) index_version: u8,
    pub(crate) object_count: usize,
    pub(crate) pack_hash: String,
    pub(crate) index_hash: String,
    pub(crate) verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stats: Option<VerifyPackStats>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) objects: Vec<VerifyPackObjectOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VerifyPackObjectOutput {
    pub(crate) oid: String,
    pub(crate) object_type: String,
    pub(crate) size: usize,
    pub(crate) size_in_pack: u64,
    pub(crate) offset: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) crc32: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VerifyPackStats {
    pub(crate) non_delta: usize,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) chain_lengths: BTreeMap<usize, usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedIndex {
    pub(crate) version: u8,
    pub(crate) entries: Vec<ParsedIndexEntry>,
    pub(crate) pack_hash: ObjectHash,
    pub(crate) index_hash: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedIndexEntry {
    pub(crate) hash: ObjectHash,
    pub(crate) offset: u64,
    pub(crate) crc32: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct DecodedPack {
    pub(crate) pack_hash: ObjectHash,
    pub(crate) pack_len: u64,
    pub(crate) entries: BTreeMap<ObjectHash, DecodedPackEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct DecodedPackEntry {
    pub(crate) index: IndexEntry,
    pub(crate) object_type: ObjectType,
    pub(crate) size: usize,
    pub(crate) chain_len: usize,
}
