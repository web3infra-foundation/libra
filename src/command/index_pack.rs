//! Builds pack index files by reading pack data, computing object offsets and hashes, and writing corresponding .idx outputs.

use std::{
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
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
use sha1::{Digest, Sha1};

#[derive(Parser, Debug)]
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

pub fn execute(args: IndexPackArgs) {
    let pack_file = args.pack_file;
    let index_file = args.index_file.unwrap_or_else(|| {
        if !pack_file.ends_with(".pack") {
            eprintln!("fatal: pack-file does not end with '.pack'");
            return String::new();
        }
        pack_file.replace(".pack", ".idx")
    });
    if index_file.is_empty() {
        return;
    }
    if index_file == pack_file {
        eprintln!("fatal: pack-file and index-file are the same file");
        return;
    }

    if let Some(version) = args.index_version {
        match version {
            1 => build_index_v1(&pack_file, &index_file).unwrap(),
            2 => build_index_v2(&pack_file, &index_file).unwrap(),
            _ => eprintln!("fatal: unsupported index version"),
        }
    } else {
        // default version = 1
        build_index_v1(&pack_file, &index_file).unwrap();
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
    let tmp_path = pack_path.parent().unwrap();
    let pack_file = std::fs::File::open(pack_file)?;
    let mut pack_reader = std::io::BufReader::new(pack_file);
    let obj_map = Arc::new(Mutex::new(BTreeMap::new())); // sorted by hash
    let obj_map_c = obj_map.clone();
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
            let offset = meta_entry.meta.pack_offset.unwrap();
            obj_map_c.lock().unwrap().insert(hash_key, offset);
        },
        None::<fn(ObjectHash)>,
    )?;

    let mut index_hash = Sha1::new();
    let mut index_file = std::fs::File::create(index_file)?;
    // fan-out table
    // The header consists of 256 4-byte network byte order integers.
    // N-th entry of this table records the number of objects in the corresponding pack,
    // the first byte of whose object name is less than or equal to N.
    // This is called the first-level fan-out table.
    let mut i: u8 = 0;
    let mut cnt: u32 = 0;
    let mut fan_out = Vec::with_capacity(256 * 4);
    let obj_map = Arc::try_unwrap(obj_map).unwrap().into_inner().unwrap();
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
    index_file.write_all(&fan_out)?;

    // 4-byte network byte order integer, recording where the
    // object is stored in the pack-file as the offset from the beginning.
    // one object name of the appropriate size (20 bytes).
    for (hash, offset) in obj_map {
        let mut buf = Vec::with_capacity(24);
        buf.write_u32::<BigEndian>(offset as u32)?;
        buf.write_all(hash.as_ref())?;

        index_hash.update(&buf);
        index_file.write_all(&buf)?;
    }

    index_hash.update(pack.signature.as_ref());
    // A copy of the pack checksum at the end of the corresponding pack-file.
    index_file.write_all(pack.signature.as_ref())?;
    let index_hash: [u8; 20] = index_hash.finalize().into();
    // Index checksum of all of the above.
    index_file.write_all(&index_hash)?;

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
    let mut idx_file = tokio::fs::File::create(index_file).await?;
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;

        while let Some(chunk) = rx.recv().await {
            idx_file.write_all(&chunk).await?;
        }
        idx_file.flush().await?;
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

/// Build index file for pack file, version 2
pub fn build_index_v2(pack_file: &str, index_file: &str) -> Result<(), GitError> {
    let pack_path = PathBuf::from(pack_file);
    let tmp_path = pack_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
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
                Ok(entry) => idx_entries_c.lock().unwrap().push(entry),
                Err(e) => {
                    let mut guard = err_c.lock().unwrap();
                    if guard.is_none() {
                        *guard = Some(e);
                    }
                }
            };
        },
        None::<fn(ObjectHash)>,
    )?;

    if let Some(err) = err.lock().unwrap().take() {
        return Err(err);
    }

    let idx_entries = Arc::try_unwrap(idx_entries).unwrap().into_inner().unwrap();
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
