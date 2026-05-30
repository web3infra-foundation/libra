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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// `legacy_session_path` must append `<id>.json` to the configured
    /// sessions directory. Pin the path-shape contract; callers like
    /// `code resume` rely on this exact layout to locate legacy files.
    #[test]
    fn legacy_session_path_appends_json_suffix() {
        let tmp = TempDir::new().expect("tmp dir");
        let migrator = LegacySessionMigrator::new(tmp.path().to_path_buf());
        let path = migrator.legacy_session_path("abc-123");
        assert_eq!(path, tmp.path().join("abc-123.json"));
        assert!(path.to_string_lossy().ends_with(".json"));
    }

    /// `is_stale_lock` returns `false` when the path doesn't exist —
    /// "no lock file" is NOT a stale lock; the caller can race to
    /// create one.
    #[test]
    fn is_stale_lock_false_for_missing_path() {
        let tmp = TempDir::new().expect("tmp dir");
        let missing = tmp.path().join("never-created.lock");
        assert!(!is_stale_lock(&missing));
    }

    /// `is_stale_lock` returns `false` for a freshly-created file
    /// (within the staleness window).
    #[test]
    fn is_stale_lock_false_for_freshly_created_lock() {
        let tmp = TempDir::new().expect("tmp dir");
        let lock_path = tmp.path().join("fresh.lock");
        std::fs::write(&lock_path, "pid=123\n").expect("write lock");
        assert!(!is_stale_lock(&lock_path));
    }

    /// `migrate_if_needed` returns `Ok(false)` when no legacy file
    /// exists at the configured path — there's nothing to migrate.
    #[test]
    fn migrate_if_needed_returns_false_when_no_legacy_file() {
        let tmp = TempDir::new().expect("tmp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("mkdir");
        let session_root = tmp.path().join("session-root");
        std::fs::create_dir_all(&session_root).expect("mkdir");

        let migrator = LegacySessionMigrator::new(sessions_dir);
        let migrated = migrator
            .migrate_if_needed("nonexistent", &session_root)
            .expect("call must succeed");
        assert!(!migrated, "no legacy file → must report false");
    }

    /// `migrate_if_needed` returns `Ok(false)` when the JSONL store
    /// already has events — the migration is idempotent, and a
    /// second call is a no-op.
    #[test]
    fn migrate_if_needed_returns_false_when_jsonl_already_populated() {
        let tmp = TempDir::new().expect("tmp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("mkdir");
        let session_root = tmp.path().join("session-root");
        std::fs::create_dir_all(&session_root).expect("mkdir");

        // Pre-populate the JSONL store with a snapshot.
        let jsonl = super::SessionJsonlStore::new(session_root.clone());
        let initial = super::SessionState::new("/tmp/work");
        jsonl
            .append(&super::SessionEvent::snapshot(initial))
            .expect("append snapshot");

        // Also place a "legacy" file that *would* otherwise migrate.
        let migrator = LegacySessionMigrator::new(sessions_dir.clone());
        let legacy_path = migrator.legacy_session_path("session-id");
        std::fs::write(
            &legacy_path,
            serde_json::to_string(&super::SessionState::new("/tmp/legacy")).unwrap(),
        )
        .expect("write legacy");

        let migrated = migrator
            .migrate_if_needed("session-id", &session_root)
            .expect("call must succeed");
        assert!(
            !migrated,
            "JSONL store already populated → must NOT migrate again",
        );
    }

    /// Happy-path migration: a legacy `<id>.json` file is parsed and
    /// converted into a JSONL snapshot event.
    #[test]
    fn migrate_if_needed_converts_legacy_json_to_jsonl_snapshot() {
        let tmp = TempDir::new().expect("tmp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("mkdir");
        let session_root = tmp.path().join("session-root");

        let migrator = LegacySessionMigrator::new(sessions_dir.clone());
        let legacy = super::SessionState::new("/tmp/work");
        let legacy_path = migrator.legacy_session_path("session-id");
        std::fs::write(&legacy_path, serde_json::to_string(&legacy).unwrap())
            .expect("write legacy");

        let migrated = migrator
            .migrate_if_needed("session-id", &session_root)
            .expect("migrate must succeed");
        assert!(migrated, "legacy file exists + no JSONL → must migrate");

        // The JSONL store should now have exactly one snapshot event.
        let jsonl = super::SessionJsonlStore::new(session_root);
        assert!(jsonl.has_events().expect("has_events"));
    }

    /// Migration is idempotent: a second `migrate_if_needed` call
    /// after a successful migration must report `false` and leave the
    /// JSONL store unchanged.
    #[test]
    fn migrate_if_needed_is_idempotent_on_second_call() {
        let tmp = TempDir::new().expect("tmp dir");
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("mkdir");
        let session_root = tmp.path().join("session-root");

        let migrator = LegacySessionMigrator::new(sessions_dir.clone());
        let legacy = super::SessionState::new("/tmp/work");
        let legacy_path = migrator.legacy_session_path("session-id");
        std::fs::write(&legacy_path, serde_json::to_string(&legacy).unwrap())
            .expect("write legacy");

        // First call migrates.
        assert!(
            migrator
                .migrate_if_needed("session-id", &session_root)
                .expect("first call"),
        );

        // Second call must NOT re-migrate.
        assert!(
            !migrator
                .migrate_if_needed("session-id", &session_root)
                .expect("second call"),
        );
    }
}
