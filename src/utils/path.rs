//! Path builders for repository storage: index, objects, database, hooks, and attributes locations relative to the working directory.

use std::{io, path::PathBuf};

use crate::utils::util;

pub fn index() -> PathBuf {
    util::storage_path().join("index")
}

pub fn try_index() -> io::Result<PathBuf> {
    Ok(util::try_get_storage_path(None)?.join("index"))
}

pub fn objects() -> PathBuf {
    util::storage_path().join("objects")
}

pub fn try_objects() -> io::Result<PathBuf> {
    Ok(util::try_get_storage_path(None)?.join("objects"))
}

pub fn database() -> PathBuf {
    util::storage_path().join(util::DATABASE)
}

pub fn hooks() -> PathBuf {
    util::storage_path().join("hooks")
}

pub fn attributes() -> PathBuf {
    util::working_dir().join(util::ATTRIBUTES)
}
