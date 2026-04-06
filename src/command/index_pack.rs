//! Builds pack index files by reading pack data, computing object offsets and hashes, and writing corresponding .idx outputs.

use std::{
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use byteorder::{BigEndian, WriteBytesExt};
use clap::Parser;
//use crc32fast::Hasher as Crc32; //use for index version 2
use git_internal::errors::GitError;
use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
        pack::{
            Pack,
            entry::Entry,
            pack_index::{IdxBuilder, IndexEntry},
        },
    },
};
use serde::Serialize;
use sha1::{Digest, Sha1};

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::{OutputConfig, emit_json_data},
};

const INDEX_PACK_EXAMPLES: &str = "\
EXAMPLES:
  libra index-pack pack-123.pack
  libra index-pack pack-123.pack -o pack-123.idx
  libra index-pack pack-123.pack --json
";
const INDEX_WRITE_ERROR_PREFIX: &str = "index write failed";

#[derive(Parser, Debug)]
#[command(after_help = INDEX_PACK_EXAMPLES)]
pub struct IndexPackArgs {
    /// Pack file path
    pub pack_file: String,
    /// output index file path.
    /// Without this option the name of pack index file is constructed from
    /// the name of packed archive file by replacing `.pack` with `.idx`
    #[clap(short = 'o', required = false)]
    pub index_file: Option<String>, // Option is must, or clap will require it

    /// This is intended to be used by the test suite only.
    /// It allows to force the version for the generated pack index
    #[clap(long, required = false)]
    pub index_version: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct IndexPackOutput {
    pack_file: String,
    index_file: String,
    index_version: u8,
}

pub fn execute(args: IndexPackArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()) {
        err.print_stderr();
    }
}

pub fn execute_safe(args: IndexPackArgs, output: &OutputConfig) -> CliResult<()> {
    let pack_file = args.pack_file;
    let index_file = match args.index_file {
        Some(index_file) => index_file,
        None => {
            if !pack_file.ends_with(".pack") {
                return Err(CliError::fatal("pack-file does not end with '.pack'")
                    .with_stable_code(StableErrorCode::CliInvalidArguments));
            }
            pack_file.replace(".pack", ".idx")
        }
    };
    if index_file == pack_file {
        return Err(
            CliError::fatal("pack-file and index-file are the same file")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }

    std::fs::File::open(&pack_file).map_err(|e| {
        CliError::fatal(format!(
            "could not open '{}' for reading: {}",
            pack_file,
            format_io_error(&e)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;

    let index_version = if let Some(version) = args.index_version {
        match version {
            1 => {
                build_index_v1(&pack_file, &index_file).map_err(index_pack_error)?;
                1
            }
            2 => {
                build_index_v2(&pack_file, &index_file).map_err(index_pack_error)?;
                2
            }
            _ => {
                return Err(CliError::fatal("unsupported index version")
                    .with_stable_code(StableErrorCode::CliInvalidArguments));
            }
        }
    } else {
        // default version = 1
        build_index_v1(&pack_file, &index_file).map_err(index_pack_error)?;
        1
    };

    let result = IndexPackOutput {
        pack_file,
        index_file,
        index_version,
    };

    if output.is_json() {
        emit_json_data("index-pack", &result, output)?;
    } else if !output.quiet {
        println!("{}", result.index_file);
    }

    Ok(())
}

fn index_pack_error(err: GitError) -> CliError {
    let stable_code = match err {
        GitError::PackEncodeError(ref message) if message.starts_with(INDEX_WRITE_ERROR_PREFIX) => {
            StableErrorCode::IoWriteFailed
        }
        // IO errors that still reach this layer originate from reading or
        // decoding the input pack. Explicit index write operations are wrapped
        // with a write-specific PackEncodeError above.
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

    CliError::fatal(format!("failed to build pack index: {err}")).with_stable_code(stable_code)
}

fn format_io_error(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => err.to_string(),
    }
}

fn index_write_error(action: &str, error: std::io::Error) -> GitError {
    GitError::PackEncodeError(format!(
        "{INDEX_WRITE_ERROR_PREFIX} while {action}: {error}"
    ))
}

fn lock_state<'a, T>(mutex: &'a Mutex<T>, label: &str) -> Result<MutexGuard<'a, T>, GitError> {
    mutex
        .lock()
        .map_err(|_| GitError::PackEncodeError(format!("{label} mutex poisoned")))
}

fn take_arc_mutex<T>(arc: Arc<Mutex<T>>, label: &str) -> Result<T, GitError> {
    let mutex = Arc::try_unwrap(arc).map_err(|_| {
        GitError::PackEncodeError(format!("{label} still has outstanding references"))
    })?;
    mutex
        .into_inner()
        .map_err(|_| GitError::PackEncodeError(format!("{label} mutex poisoned")))
}

fn record_first_pack_error(slot: &Arc<Mutex<Option<GitError>>>, error: GitError) {
    if let Ok(mut guard) = slot.lock()
        && guard.is_none()
    {
        *guard = Some(error);
    }
}

/// Build index file for pack file, version 1
/// [pack-format](https://git-scm.com/docs/pack-format)
pub fn build_index_v1(pack_file: &str, index_file: &str) -> Result<(), GitError> {
    // version 1 only supports SHA-1 hash
    if get_hash_kind() != HashKind::Sha1 {
        return Err(GitError::InvalidPackFile(
            "Index version 1 only supports SHA-1 hash".to_string(),
        ));
    }
    let pack_path = PathBuf::from(pack_file);
    let tmp_path = pack_path.parent().ok_or_else(|| {
        GitError::InvalidArgument(format!("invalid pack file path: '{pack_file}'"))
    })?;
    let pack_file = std::fs::File::open(pack_file)?;
    let mut pack_reader = std::io::BufReader::new(pack_file);
    let obj_map = Arc::new(Mutex::new(BTreeMap::new())); // sorted by hash
    let obj_map_c = obj_map.clone();
    let err = Arc::new(Mutex::new(None));
    let err_c = err.clone();
    let mut pack = Pack::new(
        Some(8),
        Some(1024 * 1024 * 1024),
        Some(tmp_path.to_path_buf()),
        true,
    );
    pack.decode(
        &mut pack_reader,
        move |meta_entry: MetaAttached<Entry, EntryMeta>| {
            let entry = &meta_entry.inner;
            let hash_key = entry.hash;
            let Some(offset) = meta_entry.meta.pack_offset else {
                record_first_pack_error(
                    &err_c,
                    GitError::ConversionError(
                        "missing pack offset while building version 1 index".to_string(),
                    ),
                );
                return;
            };

            match obj_map_c.lock() {
                Ok(mut guard) => {
                    guard.insert(hash_key, offset);
                }
                Err(_) => record_first_pack_error(
                    &err_c,
                    GitError::PackEncodeError("index entry map mutex poisoned".to_string()),
                ),
            }
        },
        None::<fn(ObjectHash)>,
    )?;
    if let Some(err) = lock_state(&err, "index-pack error slot")?.take() {
        return Err(err);
    }

    let mut index_hash = Sha1::new();
    let mut index_file = std::fs::File::create(index_file)
        .map_err(|e| index_write_error("creating output file", e))?;
    // fan-out table
    // The header consists of 256 4-byte network byte order integers.
    // N-th entry of this table records the number of objects in the corresponding pack,
    // the first byte of whose object name is less than or equal to N.
    // This is called the first-level fan-out table.
    let mut i: u8 = 0;
    let mut cnt: u32 = 0;
    let mut fan_out = Vec::with_capacity(256 * 4);
    let obj_map = take_arc_mutex(obj_map, "index entry map")?;
    for (hash, _) in obj_map.iter() {
        // sorted
        let first_byte = hash.as_ref()[0];
        while first_byte > i {
            // `while` rather than `if` to fill the gap, e.g. 0, 1, 2, 2, 2, 6
            fan_out.write_u32::<BigEndian>(cnt)?;
            i += 1;
        }
        cnt += 1;
    }
    // fill the rest
    loop {
        fan_out.write_u32::<BigEndian>(cnt)?;
        if i == 255 {
            break;
        }
        i += 1;
    }
    index_hash.update(&fan_out);
    index_file
        .write_all(&fan_out)
        .map_err(|e| index_write_error("writing fan-out table", e))?;

    // 4-byte network byte order integer, recording where the
    // object is stored in the pack-file as the offset from the beginning.
    // one object name of the appropriate size (20 bytes).
    for (hash, offset) in obj_map {
        let mut buf = Vec::with_capacity(24);
        buf.write_u32::<BigEndian>(offset as u32)?;
        buf.write_all(hash.as_ref())?;

        index_hash.update(&buf);
        index_file
            .write_all(&buf)
            .map_err(|e| index_write_error("writing object offsets", e))?;
    }

    index_hash.update(pack.signature.as_ref());
    // A copy of the pack checksum at the end of the corresponding pack-file.
    index_file
        .write_all(pack.signature.as_ref())
        .map_err(|e| index_write_error("writing pack checksum", e))?;
    let index_hash: [u8; 20] = index_hash.finalize().into();
    // Index checksum of all of the above.
    index_file
        .write_all(&index_hash)
        .map_err(|e| index_write_error("writing index checksum", e))?;

    tracing::debug!("Index file is written to {:?}", index_file);
    Ok(())
}

/// help function to write index v2 file asynchronously
async fn write_idx_v2_file(
    index_file: PathBuf,
    idx_entries: Vec<IndexEntry>,
    pack_hash: ObjectHash,
) -> Result<(), GitError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    let mut builder = IdxBuilder::new(idx_entries.len(), tx, pack_hash);
    let mut idx_file = tokio::fs::File::create(index_file)
        .await
        .map_err(|e| index_write_error("creating output file", e))?;
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;

        while let Some(chunk) = rx.recv().await {
            idx_file
                .write_all(&chunk)
                .await
                .map_err(|e| index_write_error("writing index data", e))?;
        }
        idx_file
            .flush()
            .await
            .map_err(|e| index_write_error("flushing index file", e))?;
        Ok::<(), GitError>(())
    });

    builder.write_idx(idx_entries).await?;
    let writer_result = writer
        .await
        .map_err(|e| GitError::PackEncodeError(format!("idx writer task join error: {e}")))?;
    writer_result?;
    Ok(())
}

/// write index v2 file synchronously
fn write_idx_v2_sync(
    index_file: PathBuf,
    idx_entries: Vec<IndexEntry>,
    pack_hash: ObjectHash,
) -> Result<(), GitError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(write_idx_v2_file(index_file, idx_entries, pack_hash))
}

struct TempDirGuard {
    path: PathBuf,
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Build index file for pack file, version 2
pub fn build_index_v2(pack_file: &str, index_file: &str) -> Result<(), GitError> {
    let pack_path = PathBuf::from(pack_file);
    let parent = pack_path.parent().unwrap_or(std::path::Path::new("."));

    // Create unique temp dir in the same filesystem to ensure atomic renames and performance
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_dir_path = parent.join(format!(".tmp_idx_{}", timestamp));
    std::fs::create_dir_all(&tmp_dir_path)?;

    // RAII guard to auto-delete on scope exit
    let _guard = TempDirGuard {
        path: tmp_dir_path.clone(),
    };

    let tmp_path = tmp_dir_path;
    let pack_file = std::fs::File::open(pack_file)?;
    let mut pack_reader = std::io::BufReader::new(pack_file);
    let idx_entries = Arc::new(Mutex::new(Vec::new()));
    let idx_entries_c = idx_entries.clone();
    let err = Arc::new(Mutex::new(None));
    let err_c = err.clone();

    let mut pack = Pack::new(
        Some(8),
        Some(1024 * 1024 * 1024),
        Some(tmp_path.to_path_buf()),
        true,
    );
    pack.decode(
        &mut pack_reader,
        move |meta_entry: MetaAttached<Entry, EntryMeta>| {
            match IndexEntry::try_from(&meta_entry) {
                Ok(entry) => match idx_entries_c.lock() {
                    Ok(mut guard) => guard.push(entry),
                    Err(_) => record_first_pack_error(
                        &err_c,
                        GitError::PackEncodeError("index entry buffer mutex poisoned".to_string()),
                    ),
                },
                Err(e) => record_first_pack_error(&err_c, e),
            };
        },
        None::<fn(ObjectHash)>,
    )?;

    if let Some(err) = lock_state(&err, "index-pack error slot")?.take() {
        return Err(err);
    }

    let idx_entries = take_arc_mutex(idx_entries, "index entry buffer")?;
    if idx_entries.len() != pack.number {
        return Err(GitError::ConversionError(format!(
            "decoded entries count {} != pack number {}",
            idx_entries.len(),
            pack.number
        )));
    }

    let index_path = PathBuf::from(index_file);
    let pack_hash = pack.signature;
    if tokio::runtime::Handle::try_current().is_ok() {
        let handle =
            std::thread::spawn(move || write_idx_v2_sync(index_path, idx_entries, pack_hash));
        return handle
            .join()
            .map_err(|_| GitError::PackEncodeError("idx writer thread panicked".to_string()))?;
    }

    write_idx_v2_sync(index_path, idx_entries, pack_hash)
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
}
