//! Legacy session migration helpers.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::{
    jsonl::{SessionEvent, SessionJsonlStore},
    state::SessionState,
};

const MIGRATION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STALE_MIGRATION_LOCK_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct LegacySessionMigrator {
    sessions_dir: PathBuf,
}

#[derive(Debug)]
struct MigrationLock {
    lock_path: PathBuf,
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.lock_path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %self.lock_path.display(),
                error = %err,
                "failed to release session migration lock"
            );
        }
    }
}

impl LegacySessionMigrator {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    pub fn legacy_session_path(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }

    pub fn migrate_if_needed(&self, id: &str, session_root: &Path) -> io::Result<bool> {
        let jsonl = SessionJsonlStore::new(session_root.to_path_buf());
        if jsonl.has_events()? {
            return Ok(false);
        }

        let legacy_path = self.legacy_session_path(id);
        if !legacy_path.exists() {
            return Ok(false);
        }

        let _lock = self.acquire_migration_lock(session_root)?;
        if jsonl.has_events()? {
            return Ok(false);
        }

        let content = fs::read_to_string(&legacy_path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to read legacy session file '{}': {err}",
                    legacy_path.display()
                ),
            )
        })?;
        let session = serde_json::from_str::<SessionState>(&content)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        jsonl.append(&SessionEvent::snapshot(session))?;
        Ok(true)
    }

    fn acquire_migration_lock(&self, session_root: &Path) -> io::Result<MigrationLock> {
        fs::create_dir_all(session_root).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to create session directory '{}': {err}",
                    session_root.display()
                ),
            )
        })?;

        let lock_path = session_root.join("migration.lock");
        let start = Instant::now();
        loop {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    let lock_content = format!(
                        "pid={}\ncreated_at_ns={}\n",
                        std::process::id(),
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );
                    file.write_all(lock_content.as_bytes())?;
                    return Ok(MigrationLock { lock_path });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    if is_stale_lock(&lock_path) {
                        match fs::remove_file(&lock_path) {
                            Ok(()) => continue,
                            Err(remove_err) if remove_err.kind() == io::ErrorKind::NotFound => {
                                continue;
                            }
                            Err(remove_err) => {
                                return Err(io::Error::new(
                                    remove_err.kind(),
                                    format!(
                                        "failed to clear stale session migration lock '{}': {remove_err}",
                                        lock_path.display()
                                    ),
                                ));
                            }
                        }
                    }

                    if start.elapsed() >= MIGRATION_LOCK_TIMEOUT {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            format!(
                                "timed out waiting for session migration lock '{}'",
                                lock_path.display()
                            ),
                        ));
                    }
                    thread::sleep(MIGRATION_LOCK_POLL_INTERVAL);
                }
                Err(err) => {
                    return Err(io::Error::new(
                        err.kind(),
                        format!(
                            "failed to open session migration lock '{}': {err}",
                            lock_path.display()
                        ),
                    ));
                }
            }
        }
    }
}

fn is_stale_lock(lock_path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(lock_path) else {
        return false;
    };
    let Ok(modified_at) = metadata.modified() else {
        return false;
    };
    let Ok(elapsed) = modified_at.elapsed() else {
        return false;
    };
    elapsed >= STALE_MIGRATION_LOCK_AGE
}
