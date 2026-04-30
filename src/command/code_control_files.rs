//! Local TUI automation control file lifecycle.
//!
//! `libra code --control write` uses three files under the repository storage
//! root by default:
//!
//! - `.libra/code/control-token` stores the per-process bearer token.
//! - `.libra/code/control.json` stores non-secret endpoint discovery metadata.
//! - `.libra/code/control.lock` is an advisory single-instance lock.
//!
//! The lock is the owner contract: callers must acquire it before writing a new
//! token, so a second write-enabled instance cannot silently replace the first
//! instance's credentials. Stale `control.json` files from crashed processes are
//! ignored when their PID is not live. On Unix the token file must be a regular
//! non-symlink file with exact `0600` permissions; Windows currently treats the
//! permission check as a no-op because ACL semantics need a separate design.

#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, fs::PermissionsExt, io::AsRawFd};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use super::code::resolve_storage_root;

/// Discovery metadata written to `control.json`.
///
/// This struct intentionally contains no control token, token hash, token path,
/// provider credentials, request body, or environment dump.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlInfo {
    pub version: u8,
    pub mode: String,
    pub pid: u32,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_url: Option<String>,
    pub working_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// Resolved token, info, and lock paths for a control-enabled session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlPaths {
    pub token: PathBuf,
    pub info: PathBuf,
    pub lock: PathBuf,
}

/// Best-effort summary of an existing live control instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveInstanceInfo {
    pub pid: u32,
    pub base_url: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
}

/// Advisory lock guard. Dropping releases the lock and best-effort removes the
/// lock file to keep manual inspection clear after normal shutdown.
#[derive(Debug)]
pub struct ControlLockGuard {
    file: File,
    lock_path: PathBuf,
}

impl Drop for ControlLockGuard {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
        if let Err(error) = fs::remove_file(&self.lock_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(
                path = %self.lock_path.display(),
                error = %error,
                "failed to remove local TUI control lock file"
            );
        }
    }
}

/// Errors returned while acquiring the write-control single-instance lock.
#[derive(Debug)]
pub enum ControlLockError {
    AlreadyHeld {
        existing: Option<LiveInstanceInfo>,
        info_path: PathBuf,
        lock_path: PathBuf,
    },
    Io(std::io::Error),
}

impl fmt::Display for ControlLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyHeld {
                existing: Some(existing),
                info_path,
                lock_path,
            } => {
                write!(
                    f,
                    "CONTROL_INSTANCE_CONFLICT: another `libra code --control write` instance is active"
                )?;
                write!(f, " (pid: {}", existing.pid)?;
                if let Some(base_url) = &existing.base_url {
                    write!(f, ", baseUrl: {base_url}")?;
                }
                write!(
                    f,
                    "). info: {}, lock: {}. Stop the existing instance (Ctrl-C / kill {}) or pass `--control-token-file` and `--control-info-file` to use separate paths.",
                    info_path.display(),
                    lock_path.display(),
                    existing.pid
                )
            }
            Self::AlreadyHeld {
                existing: None,
                info_path,
                lock_path,
            } => write!(
                f,
                "CONTROL_INSTANCE_CONFLICT: another `libra code --control write` instance holds the control lock. info: {}, lock: {}. Stop the existing instance or pass `--control-token-file` and `--control-info-file` to use separate paths.",
                info_path.display(),
                lock_path.display()
            ),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ControlLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::AlreadyHeld { .. } => None,
        }
    }
}

/// Resolve default or overridden local-control paths.
pub fn resolve_control_paths(
    working_dir: &Path,
    token_override: Option<&Path>,
    info_override: Option<&Path>,
) -> ControlPaths {
    let control_dir = resolve_storage_root(working_dir).join("code");
    let token = token_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| control_dir.join("control-token"));
    let info = info_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| control_dir.join("control.json"));
    let lock = info.with_extension("lock");
    ControlPaths { token, info, lock }
}

/// Acquire the write-control advisory lock, failing fast when another live
/// process already owns it.
pub fn acquire_control_lock(
    lock_path: &Path,
) -> std::result::Result<ControlLockGuard, ControlLockError> {
    if let Some(parent) = lock_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return Err(ControlLockError::Io(error));
    }

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(ControlLockError::Io)?;

    match try_lock_file_exclusive(&file) {
        Ok(true) => {}
        Ok(false) => {
            let info_path = lock_path.with_extension("json");
            let existing = inspect_existing_instance(&info_path).ok().flatten();
            return Err(ControlLockError::AlreadyHeld {
                existing,
                info_path,
                lock_path: lock_path.to_path_buf(),
            });
        }
        Err(error) => return Err(ControlLockError::Io(error)),
    }

    if let Err(error) = write_lock_pid(&file) {
        let _ = unlock_file(&file);
        return Err(ControlLockError::Io(error));
    }

    Ok(ControlLockGuard {
        file,
        lock_path: lock_path.to_path_buf(),
    })
}

fn write_lock_pid(mut file: &File) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()
}

/// Return a live instance if `control.json` points at a process that still
/// exists. Malformed or stale files return `Ok(None)`.
pub fn inspect_existing_instance(info_path: &Path) -> Result<Option<LiveInstanceInfo>> {
    let content = match fs::read_to_string(info_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to read existing control info"),
    };

    let info: ControlInfo = match serde_json::from_str(&content) {
        Ok(info) => info,
        Err(error) => {
            tracing::debug!(
                path = %info_path.display(),
                error = %error,
                "ignoring malformed local TUI control info file"
            );
            return Ok(None);
        }
    };

    if !pid_is_live(info.pid) {
        return Ok(None);
    }

    Ok(Some(LiveInstanceInfo {
        pid: info.pid,
        base_url: Some(info.base_url),
        started_at: Some(info.started_at),
    }))
}

/// Create or overwrite the per-process control token file.
///
/// The caller must already hold [`ControlLockGuard`]; this function enforces
/// file type and permissions but deliberately does not perform a second
/// concurrency check.
pub async fn ensure_control_token_file(path: &Path) -> Result<String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create control token parent directory '{}'",
                parent.display()
            )
        })?;
    }

    let token = generate_control_token()?;

    if path.exists() || fs::symlink_metadata(path).is_ok() {
        validate_token_file_perms(path)?;
        let mut file = writable_token_file(path, false)?;
        file.set_len(0).with_context(|| {
            format!("failed to truncate control token file '{}'", path.display())
        })?;
        file.write_all(token.as_bytes())
            .with_context(|| format!("failed to write control token file '{}'", path.display()))?;
        file.flush()
            .with_context(|| format!("failed to flush control token file '{}'", path.display()))?;
        return Ok(token);
    }

    let mut file = writable_token_file(path, true)?;
    file.write_all(token.as_bytes())
        .with_context(|| format!("failed to write control token file '{}'", path.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush control token file '{}'", path.display()))?;
    Ok(token)
}

fn generate_control_token() -> Result<String> {
    let rng = SystemRandom::new();
    let mut token = [0u8; 32];
    rng.fill(&mut token)
        .map_err(|_| anyhow!("failed to generate secure local TUI control token"))?;
    Ok(URL_SAFE_NO_PAD.encode(token))
}

/// Validate that the token path is a regular `0600` file on Unix.
pub fn validate_token_file_perms(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect control token file '{}'", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "control token file '{}' must not be a symlink",
            path.display()
        );
    }
    if !metadata.file_type().is_file() {
        bail!(
            "control token path '{}' must be a regular file",
            path.display()
        );
    }
    validate_token_file_mode(path, &metadata)
}

#[cfg(unix)]
fn validate_token_file_mode(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    let mode = metadata.permissions().mode() & 0o777;
    if mode != 0o600 {
        bail!(
            "control token file '{}' must have permissions 0600 (currently {:03o}); run: chmod 0600 {}",
            path.display(),
            mode,
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_token_file_mode(_path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn writable_token_file(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    if create_new {
        options.create_new(true);
    } else {
        options.create(false).truncate(false);
    }
    options
        .open(path)
        .with_context(|| format!("failed to open control token file '{}'", path.display()))
}

#[cfg(not(unix))]
fn writable_token_file(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(false).truncate(false);
    }
    options
        .open(path)
        .with_context(|| format!("failed to open control token file '{}'", path.display()))
}

/// Write non-secret local-control discovery metadata.
pub fn write_control_info(path: &Path, info: &ControlInfo) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create control info parent directory '{}'",
                parent.display()
            )
        })?;
    }
    let serialized =
        serde_json::to_string_pretty(info).context("failed to serialize local TUI control info")?;
    fs::write(path, serialized)
        .with_context(|| format!("failed to write control info file '{}'", path.display()))
}

/// Best-effort cleanup for token/info files on normal shutdown or startup
/// failure. Lock file cleanup is owned by [`ControlLockGuard::drop`].
pub fn cleanup_control_files(paths: &ControlPaths, remove_token: bool, remove_info: bool) {
    let cleanup_paths = [
        remove_token.then_some(&paths.token),
        remove_info.then_some(&paths.info),
    ];
    for path in cleanup_paths.into_iter().flatten() {
        if let Err(error) = fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(
                path = %path.display(),
                error = %error,
                "failed to remove local TUI control file"
            );
        }
    }
}

/// Return whether a PID appears to still be live.
pub fn pid_is_live(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    pid_is_live_impl(pid)
}

#[cfg(unix)]
fn pid_is_live_impl(pid: u32) -> bool {
    if pid > i32::MAX as u32 {
        return false;
    }
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == libc::ESRCH => false,
        Some(code) if code == libc::EPERM => true,
        _ => true,
    }
}

#[cfg(not(unix))]
fn pid_is_live_impl(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn try_lock_file_exclusive(file: &File) -> std::io::Result<bool> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(false),
        _ => Err(error),
    }
}

#[cfg(unix)]
fn unlock_file(file: &File) -> std::io::Result<()> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn try_lock_file_exclusive(_file: &File) -> std::io::Result<bool> {
    Ok(true)
}

#[cfg(not(unix))]
fn unlock_file(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::{fs::Permissions, os::unix::fs as unix_fs};

    use super::*;

    fn test_control_info(pid: u32, base_url: &str) -> ControlInfo {
        ControlInfo {
            version: 1,
            mode: "write".to_string(),
            pid,
            base_url: base_url.to_string(),
            mcp_url: Some("http://127.0.0.1:6789".to_string()),
            working_dir: PathBuf::from("/tmp/repo"),
            thread_id: None,
            started_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn code_control_files_create_new_token_with_0600_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let token_path = temp.path().join("control-token");
        let lock_path = temp.path().join("control.lock");
        let guard = acquire_control_lock(&lock_path).unwrap();

        let token = ensure_control_token_file(&token_path).await.unwrap();

        assert!(!token.is_empty());
        assert_eq!(fs::read_to_string(&token_path).unwrap(), token);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&token_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let lock_contents = fs::read_to_string(&lock_path).unwrap();
        assert!(lock_contents.contains(&std::process::id().to_string()));
        assert!(!lock_contents.contains(&token));
        drop(guard);
    }

    #[tokio::test]
    async fn code_control_files_existing_0600_token_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let token_path = temp.path().join("control-token");
        fs::write(&token_path, "old-token").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&token_path, Permissions::from_mode(0o600)).unwrap();

        let new_token = ensure_control_token_file(&token_path).await.unwrap();

        assert_ne!(new_token, "old-token");
        assert_eq!(fs::read_to_string(&token_path).unwrap(), new_token);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn code_control_files_rejects_wide_token_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let token_path = temp.path().join("control-token");
        fs::write(&token_path, "old-token").unwrap();
        fs::set_permissions(&token_path, Permissions::from_mode(0o644)).unwrap();

        let error = ensure_control_token_file(&token_path).await.unwrap_err();

        assert!(error.to_string().contains("chmod 0600"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn code_control_files_rejects_symlink_token_path() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let token_path = temp.path().join("control-token");
        fs::write(&target, "target-content").unwrap();
        unix_fs::symlink(&target, &token_path).unwrap();

        let error = ensure_control_token_file(&token_path).await.unwrap_err();

        assert!(error.to_string().contains("must not be a symlink"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "target-content");
    }

    #[test]
    fn code_control_files_control_info_contains_no_token_material() {
        let info = test_control_info(12345, "http://127.0.0.1:3000");

        let json = serde_json::to_string(&info).unwrap();

        assert!(json.contains("baseUrl"));
        assert!(!json.contains("control-token"));
        assert!(!json.contains("token"));
        assert!(!json.contains("tokenHash"));
    }

    #[test]
    fn code_control_files_second_lock_fails_fast_with_live_instance() {
        let temp = tempfile::tempdir().unwrap();
        let lock_path = temp.path().join("control.lock");
        let info_path = temp.path().join("control.json");
        write_control_info(
            &info_path,
            &test_control_info(std::process::id(), "http://127.0.0.1:3000"),
        )
        .unwrap();

        let _guard = acquire_control_lock(&lock_path).unwrap();
        let error = acquire_control_lock(&lock_path).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("CONTROL_INSTANCE_CONFLICT"));
        assert!(message.contains(&std::process::id().to_string()));
        assert!(message.contains("http://127.0.0.1:3000"));
    }

    #[test]
    fn code_control_files_stale_info_does_not_block_lock() {
        let temp = tempfile::tempdir().unwrap();
        let lock_path = temp.path().join("control.lock");
        let info_path = temp.path().join("control.json");
        write_control_info(
            &info_path,
            &test_control_info(u32::MAX, "http://127.0.0.1:3000"),
        )
        .unwrap();

        assert!(inspect_existing_instance(&info_path).unwrap().is_none());
        let _guard = acquire_control_lock(&lock_path).unwrap();
    }

    #[test]
    fn code_control_files_custom_paths_have_independent_locks() {
        let temp = tempfile::tempdir().unwrap();
        let working_dir = temp.path().join("repo");
        let token_a = temp.path().join("a-token");
        let info_a = temp.path().join("a.json");
        let token_b = temp.path().join("b-token");
        let info_b = temp.path().join("b.json");

        let paths_a = resolve_control_paths(&working_dir, Some(&token_a), Some(&info_a));
        let paths_b = resolve_control_paths(&working_dir, Some(&token_b), Some(&info_b));

        assert_ne!(paths_a.lock, paths_b.lock);
        let _guard_a = acquire_control_lock(&paths_a.lock).unwrap();
        let _guard_b = acquire_control_lock(&paths_b.lock).unwrap();
    }

    #[test]
    fn code_control_files_pid_liveness_rejects_invalid_pid_values() {
        assert!(!pid_is_live(0));
        assert!(!pid_is_live(u32::MAX));
    }
}
