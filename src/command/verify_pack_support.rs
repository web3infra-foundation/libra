use std::{fs, io, path::Path};

use crate::utils::error::{CliError, CliResult, StableErrorCode};

pub(crate) fn read_file(path: &Path, label: &str) -> CliResult<Vec<u8>> {
    fs::read(path).map_err(|error| {
        CliError::fatal(format!(
            "could not open {label} '{}' for reading: {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })
}

pub(crate) fn invalid_index(path: &Path, detail: String) -> CliError {
    CliError::fatal(format!("invalid pack index '{}': {detail}", path.display()))
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

pub(crate) fn verification_failed(idx_file: &Path, pack_file: &Path, detail: String) -> CliError {
    CliError::fatal(format!(
        "pack verification failed for '{}' against '{}': {detail}",
        idx_file.display(),
        pack_file.display()
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt)
}

pub(crate) fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn format_io_error(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "No such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => err.to_string(),
    }
}
