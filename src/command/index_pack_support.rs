use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use git_internal::errors::GitError;

use crate::utils::error::{CliError, CliResult, StableErrorCode};

const INDEX_WRITE_ERROR_PREFIX: &str = "index write failed";
const ISSUE_URL: &str = "https://github.com/web3infra-foundation/libra/issues";

pub(crate) fn index_pack_error(err: GitError) -> CliError {
    let stable_code = match err {
        GitError::PackEncodeError(ref message) if message.starts_with(INDEX_WRITE_ERROR_PREFIX) => {
            StableErrorCode::IoWriteFailed
        }
        GitError::IOError(_) => StableErrorCode::IoReadFailed,
        GitError::InvalidArgument(_) => StableErrorCode::CliInvalidArguments,
        GitError::InvalidPackFile(_)
        | GitError::InvalidPackHeader(_)
        | GitError::InvalidIdxFile(_)
        | GitError::ConversionError(_)
        | GitError::DeltaObjectError(_)
        | GitError::InvalidHashValue(_)
        | GitError::InvalidObjectInfo(_)
        | GitError::ObjectNotFound(_) => StableErrorCode::RepoCorrupt,
        _ => StableErrorCode::InternalInvariant,
    };

    let cli =
        CliError::fatal(format!("failed to build pack index: {err}")).with_stable_code(stable_code);
    if stable_code == StableErrorCode::InternalInvariant {
        cli.with_hint(format!("this is a bug; please report it at {ISSUE_URL}"))
    } else {
        cli
    }
}

pub(crate) fn format_io_error(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => err.to_string(),
    }
}

pub(crate) fn index_write_error(action: &str, error: std::io::Error) -> GitError {
    GitError::PackEncodeError(format!(
        "{INDEX_WRITE_ERROR_PREFIX} while {action}: {error}"
    ))
}

pub(crate) fn keep_file_path(pack_file: &str) -> PathBuf {
    PathBuf::from(pack_file).with_extension("keep")
}

pub(crate) fn write_keep_file(keep_file: &str, message: &str) -> CliResult<()> {
    let mut file = std::fs::File::create(keep_file).map_err(|e| {
        CliError::fatal(format!(
            "could not create '{}' for writing: {}",
            keep_file,
            format_io_error(&e)
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)
    })?;

    if !message.is_empty() {
        writeln!(file, "{message}").map_err(|e| {
            CliError::fatal(format!(
                "could not write keep message to '{}': {}",
                keep_file,
                format_io_error(&e)
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    }

    Ok(())
}

pub(crate) fn lock_state<'a, T>(
    mutex: &'a Mutex<T>,
    label: &str,
) -> Result<MutexGuard<'a, T>, GitError> {
    mutex
        .lock()
        .map_err(|_| GitError::PackEncodeError(format!("{label} mutex poisoned")))
}

pub(crate) fn take_arc_mutex<T>(arc: Arc<Mutex<T>>, label: &str) -> Result<T, GitError> {
    let mutex = Arc::try_unwrap(arc).map_err(|_| {
        GitError::PackEncodeError(format!("{label} still has outstanding references"))
    })?;
    mutex
        .into_inner()
        .map_err(|_| GitError::PackEncodeError(format!("{label} mutex poisoned")))
}

pub(crate) fn record_first_pack_error(slot: &Arc<Mutex<Option<GitError>>>, error: GitError) {
    if let Ok(mut guard) = slot.lock()
        && guard.is_none()
    {
        *guard = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_pack_error_maps_wrapped_write_failures_to_io_write_failed() {
        let cli_error = index_pack_error(index_write_error(
            "writing index data",
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
        ));

        assert_eq!(cli_error.stable_code(), StableErrorCode::IoWriteFailed);
    }

    #[test]
    fn lock_state_reports_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new(1_u8));
        let poisoned = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison test mutex");
        })
        .join();

        let err = lock_state(&mutex, "index entry buffer").expect_err("mutex should be poisoned");
        match err {
            GitError::PackEncodeError(message) => {
                assert_eq!(message, "index entry buffer mutex poisoned");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn take_arc_mutex_reports_outstanding_references() {
        let mutex = Arc::new(Mutex::new(vec![1_u8]));
        let _extra_ref = Arc::clone(&mutex);

        let err =
            take_arc_mutex(mutex, "index entry buffer").expect_err("extra Arc ref should fail");
        match err {
            GitError::PackEncodeError(message) => {
                assert_eq!(
                    message,
                    "index entry buffer still has outstanding references"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn take_arc_mutex_reports_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new(vec![1_u8]));
        let poisoned = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("poison test mutex");
        })
        .join();

        let err =
            take_arc_mutex(mutex, "index entry buffer").expect_err("mutex should be poisoned");
        match err {
            GitError::PackEncodeError(message) => {
                assert_eq!(message, "index entry buffer mutex poisoned");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn index_pack_error_maps_unknown_git_error_with_issue_url_hint() {
        let cli_error = index_pack_error(GitError::UnCompletedPackObject(
            "synthetic uncompleted pack object".to_string(),
        ));

        assert_eq!(cli_error.stable_code(), StableErrorCode::InternalInvariant);
        assert!(
            cli_error
                .hints()
                .iter()
                .any(|h| h.as_str().contains("issues")),
            "InternalInvariant fall-through must include the Issues URL hint, got hints: {:?}",
            cli_error.hints()
        );
    }
}
