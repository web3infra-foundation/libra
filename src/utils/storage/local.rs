//! Local filesystem storage backend for Git objects.
//! This module implements the `Storage` trait for a local filesystem backend. It supports both loose objects and packed objects, allowing for efficient storage and retrieval of Git objects on disk.
//! The `LocalStorage` struct provides methods to read and write Git objects, as well as to search for objects by prefix. It handles the Git object storage format, including zlib compression for loose objects
//! and the pack file format for packed objects. The implementation also includes caching mechanisms for pack objects to improve performance when accessing packed data.
use std::{
    collections::HashMap,
    fs, io,
    io::{BufReader, Cursor, Read, Seek, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use byteorder::{BigEndian, ReadBytesExt};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use git_internal::{
    errors::GitError,
    hash::{HashKind, ObjectHash, get_hash_kind, set_hash_kind},
    internal::{
        object::types::ObjectType,
        pack::{Pack, cache_object::CacheObject},
    },
    utils::read_sha,
};
use lru_mem::LruCache;
use once_cell::sync::Lazy;

use crate::{command, utils::storage::Storage};

/// Cache for pack objects, keyed by "pack_file_name-offset"
static PACK_OBJ_CACHE: Lazy<Mutex<LruCache<String, CacheObject>>> =
    Lazy::new(|| Mutex::new(LruCache::new(1024 * 1024 * 200)));
/// Cache for entire pack files, keyed by "pack_file_name"
static PACK_FILE_CACHE: Lazy<Mutex<HashMap<String, Vec<u8>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const IDX_MAGIC: [u8; 4] = [0xFF, 0x74, 0x4F, 0x63];
const FANOUT: u64 = 256 * 4;

/// Index version for pack files
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdxVersion {
    V1,
    V2,
}

/// Local filesystem storage backend
#[derive(Default, Clone)]
pub struct LocalStorage {
    base_path: PathBuf,
    hash_kind: Option<HashKind>, // Capture hash kind from creation thread
}

impl LocalStorage {
    pub fn new(base_path: PathBuf) -> Self {
        fs::create_dir_all(&base_path).expect("Create directory failed!");
        Self {
            base_path,
            hash_kind: Some(get_hash_kind()),
        }
    }

    /// Transforms an object hash into a path like "ab/cdef...". This is used for loose objects.
    fn transform_path(&self, hash: &ObjectHash) -> String {
        let hash = hash.to_string();
        Path::new(&hash[0..2])
            .join(&hash[2..hash.len()])
            .into_os_string()
            .into_string()
            .unwrap()
    }

    /// Gets the full path to an object file based on its hash. For example, "base_path/ab/cdef...".
    pub(crate) fn get_obj_path(&self, obj_id: &ObjectHash) -> PathBuf {
        Path::new(&self.base_path).join(self.transform_path(obj_id))
    }

    /// Checks if a loose object exists by looking for its file. This is a quick check before looking into packs.
    fn exist_loosely(&self, obj_id: &ObjectHash) -> bool {
        let path = self.get_obj_path(obj_id);
        Path::exists(&path)
    }

    /// Reads the raw compressed data of a loose object from the filesystem. This is used when we know the object exists as a loose object.
    fn read_raw_data(&self, obj_id: &ObjectHash) -> Result<Vec<u8>, io::Error> {
        let path = self.get_obj_path(obj_id);
        let mut file = fs::File::open(path)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        Ok(buffer)
    }

    /// Decompresses zlib-compressed data, which is the format used for loose objects. This is used after reading the raw data of a loose object.
    fn decompress_zlib(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut decoder = ZlibDecoder::new(data);
        let mut decompressed_data = Vec::new();
        decoder.read_to_end(&mut decompressed_data)?;
        Ok(decompressed_data)
    }

    /// Compresses data using zlib, which is the format used for storing loose objects. This is used before writing a new loose object to the filesystem.
    fn compress_zlib(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data)?;
        let compressed_data = encoder.finish()?;
        Ok(compressed_data)
    }

    /// Parses the header of a loose object, which has the format "type size\0". This is used after decompressing a loose object's data to extract its type and size.
    fn parse_header(data: &[u8]) -> (String, usize, usize) {
        let end_of_header = data
            .iter()
            .position(|&b| b == b'\0')
            .expect("Invalid object: no header terminator");
        let header_str =
            std::str::from_utf8(&data[..end_of_header]).expect("Invalid UTF-8 in header");

        let mut parts = header_str.splitn(2, ' ');
        let obj_type = parts.next().expect("No object type in header").to_string();
        let size_str = parts.next().expect("No size in header");
        let size = size_str.parse::<usize>().expect("Invalid size in header");
        assert_eq!(size, data.len() - 1 - end_of_header, "Invalid object size");
        (obj_type, size, end_of_header)
    }

    // --- Pack related methods ---

    fn list_all_packs(&self) -> Vec<PathBuf> {
        let pack_dir = self.base_path.join("pack");
        if !pack_dir.exists() {
            return Vec::new();
        }
        let mut packs = Vec::new();
        if let Ok(entries) = fs::read_dir(pack_dir) {
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_file() && path.extension().unwrap() == "pack" {
                    packs.push(path);
                }
            }
        }
        packs
    }

    fn list_all_idx(&self) -> Vec<PathBuf> {
        let packs = self.list_all_packs();
        let mut idxs = Vec::new();
        for pack in packs {
            let idx = pack.with_extension("idx");
            let want_v2 = get_hash_kind() == HashKind::Sha256;
            let needs_rebuild = if idx.exists() {
                if want_v2 {
                    !matches!(Self::read_idx_version_path(&idx), Ok(IdxVersion::V2))
                } else {
                    false
                }
            } else {
                true
            };

            if needs_rebuild {
                if want_v2 {
                    command::index_pack::build_index_v2(
                        pack.to_str().unwrap(),
                        idx.to_str().unwrap(),
                    )
                    .unwrap();
                } else {
                    command::index_pack::build_index_v1(
                        pack.to_str().unwrap(),
                        idx.to_str().unwrap(),
                    )
                    .unwrap();
                }
            }
            idxs.push(idx);
        }
        idxs
    }

    fn read_idx_version(file: &mut fs::File) -> Result<IdxVersion, io::Error> {
        let mut header = [0u8; 4];
        file.read_exact(&mut header)?;
        if header == IDX_MAGIC {
            let mut version_buf = [0u8; 4];
            file.read_exact(&mut version_buf)?;
            let version = u32::from_be_bytes(version_buf);
            if version != 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsupported pack index version {version}"),
                ));
            }
            Ok(IdxVersion::V2)
        } else {
            file.seek(io::SeekFrom::Start(0))?;
            Ok(IdxVersion::V1)
        }
    }

    fn read_idx_version_path(idx_file: &Path) -> Result<IdxVersion, io::Error> {
        let mut idx_file = fs::File::open(idx_file)?;
        Self::read_idx_version(&mut idx_file)
    }

    fn read_idx_fanout(idx_file: &Path) -> Result<(IdxVersion, [u32; 256]), io::Error> {
        let mut idx_file = fs::File::open(idx_file)?;
        let version = Self::read_idx_version(&mut idx_file)?;
        let fanout_offset = match version {
            IdxVersion::V1 => 0,
            IdxVersion::V2 => 8,
        };
        idx_file.seek(io::SeekFrom::Start(fanout_offset))?;
        let mut fanout: [u32; 256] = [0; 256];
        let mut buf = [0; 4];
        fanout.iter_mut().for_each(|x| {
            idx_file.read_exact(&mut buf).unwrap();
            *x = u32::from_be_bytes(buf);
        });
        Ok((version, fanout))
    }

    fn read_idx(idx_file: &Path, obj_id: &ObjectHash) -> Result<Option<u64>, io::Error> {
        let (version, fanout) = Self::read_idx_fanout(idx_file)?;
        let mut idx_file = fs::File::open(idx_file)?;

        let first_byte = obj_id.as_ref()[0];
        let start = if first_byte == 0 {
            0
        } else {
            fanout[first_byte as usize - 1] as usize
        };
        let end = fanout[first_byte as usize] as usize;
        let object_count = fanout[255] as u64;
        let hash_size = get_hash_kind().size() as u64;

        match version {
            IdxVersion::V1 => {
                if hash_size != 20 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "pack index v1 only supports sha1",
                    ));
                }
                idx_file.seek(io::SeekFrom::Start(FANOUT + 24 * start as u64))?;
                for _ in start..end {
                    let offset = idx_file.read_u32::<BigEndian>()?;
                    let hash = read_sha(&mut idx_file)?;

                    if &hash == obj_id {
                        return Ok(Some(offset as u64));
                    }
                }
                Ok(None)
            }
            IdxVersion::V2 => {
                let names_offset = FANOUT + 8;
                idx_file.seek(io::SeekFrom::Start(names_offset + hash_size * start as u64))?;
                let mut found_index = None;
                for i in start..end {
                    let hash = read_sha(&mut idx_file)?;
                    if &hash == obj_id {
                        found_index = Some(i as u64);
                        break;
                    }
                }
                let Some(index) = found_index else {
                    return Ok(None);
                };

                let crc_offset = names_offset + object_count * hash_size;
                let offsets_offset = crc_offset + object_count * 4;
                idx_file.seek(io::SeekFrom::Start(offsets_offset + index * 4))?;
                let offset = idx_file.read_u32::<BigEndian>()?;
                if offset & 0x8000_0000 != 0 {
                    let large_index = (offset & 0x7fff_ffff) as u64;
                    let large_offsets_offset = offsets_offset + object_count * 4;
                    idx_file.seek(io::SeekFrom::Start(large_offsets_offset + large_index * 8))?;
                    let large_offset = idx_file.read_u64::<BigEndian>()?;
                    Ok(Some(large_offset))
                } else {
                    Ok(Some(offset as u64))
                }
            }
        }
    }

    fn read_pack_obj(pack_file: &Path, offset: u64) -> Result<CacheObject, GitError> {
        let file_name = pack_file.file_name().unwrap().to_str().unwrap().to_owned();
        let cache_key = format!("{:?}-{}", file_name, offset);

        if let Some(cached) = PACK_OBJ_CACHE.lock().unwrap().get(&cache_key) {
            return Ok(cached.clone());
        }

        let obj = {
            let mut cache = PACK_FILE_CACHE.lock().unwrap();
            let pack_file_buf = match cache.get(&file_name) {
                None => {
                    let file = fs::File::open(pack_file)?;
                    let mut pack_reader = io::BufReader::new(&file);

                    let mut buf: Vec<u8> = Vec::new();
                    pack_reader.read_to_end(&mut buf)?;
                    cache.insert(file_name.clone(), buf);
                    cache.get(&file_name).unwrap()
                }
                Some(buf) => buf,
            };

            let pack_cursor = Cursor::new(pack_file_buf);
            let mut pack_reader = BufReader::new(pack_cursor);
            pack_reader.seek(io::SeekFrom::Start(offset))?;
            {
                let mut offset = offset as usize;
                Pack::decode_pack_object(&mut pack_reader, &mut offset)?
            }
        };
        let obj = obj.ok_or_else(|| {
            GitError::InvalidObjectInfo(format!(
                "Failed to decode pack object at offset {}",
                offset
            ))
        })?;
        let full_obj = match obj.object_type() {
            ObjectType::OffsetDelta => {
                let delta = obj.offset_delta().unwrap();
                let base_offset = offset - delta as u64;
                let base_obj = Self::read_pack_obj(pack_file, base_offset)?;
                let base_obj = Arc::new(base_obj);
                Pack::rebuild_delta(obj, base_obj)
            }
            ObjectType::HashDelta => {
                let base_hash = obj.hash_delta().unwrap();
                let idx_file = pack_file.with_extension("idx");
                let base_offset = Self::read_idx(&idx_file, &base_hash)?.unwrap();
                let base_obj = Self::read_pack_obj(pack_file, base_offset)?;
                let base_obj = Arc::new(base_obj);
                Pack::rebuild_delta(obj, base_obj)
            }
            _ => obj,
        };

        if PACK_OBJ_CACHE
            .lock()
            .unwrap()
            .insert(cache_key, full_obj.clone())
            .is_err()
        {
            tracing::warn!("Pack object cache: entry too large to cache");
        }
        Ok(full_obj)
    }

    fn get_from_pack(
        &self,
        obj_id: &ObjectHash,
    ) -> Result<Option<(Vec<u8>, ObjectType)>, GitError> {
        let idxes = self.list_all_idx();
        for idx in idxes {
            let res = Self::read_pack_by_idx(&idx, obj_id)?;
            if let Some(data) = res {
                return Ok(Some((data.data_decompressed.clone(), data.object_type())));
            }
        }
        Ok(None)
    }

    fn read_pack_by_idx(
        idx_file: &Path,
        obj_id: &ObjectHash,
    ) -> Result<Option<CacheObject>, GitError> {
        let pack_file = idx_file.with_extension("pack");
        let res = Self::read_idx(idx_file, obj_id)?;
        match res {
            None => Ok(None),
            Some(offset) => {
                let res = Self::read_pack_obj(&pack_file, offset)?;
                Ok(Some(res))
            }
        }
    }
}

#[async_trait]
impl Storage for LocalStorage {
    async fn get(&self, hash: &ObjectHash) -> Result<(Vec<u8>, ObjectType), GitError> {
        let self_clone = self.clone();
        let hash = *hash;

        // Use spawn_blocking for IO operations
        tokio::task::spawn_blocking(move || {
            if let Some(kind) = self_clone.hash_kind {
                set_hash_kind(kind);
            }
            if self_clone.exist_loosely(&hash) {
                let raw_data = self_clone.read_raw_data(&hash)?;
                let data = Self::decompress_zlib(&raw_data)?;
                let (type_str, _, end_of_header) = Self::parse_header(&data);
                let obj_type = ObjectType::from_string(&type_str)?;
                Ok((data[end_of_header + 1..].to_vec(), obj_type))
            } else {
                self_clone
                    .get_from_pack(&hash)?
                    .map(|x| (x.0, x.1))
                    .ok_or(GitError::ObjectNotFound(hash.to_string()))
            }
        })
        .await
        .map_err(|e| GitError::IOError(io::Error::other(e)))?
    }

    async fn put(
        &self,
        hash: &ObjectHash,
        data: &[u8],
        obj_type: ObjectType,
    ) -> Result<String, GitError> {
        let self_clone = self.clone();
        let hash = *hash;
        let data = data.to_vec();

        tokio::task::spawn_blocking(move || {
            if let Some(kind) = self_clone.hash_kind {
                set_hash_kind(kind);
            }
            let path = self_clone.get_obj_path(&hash);
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir)?;

            let header = format!("{} {}\0", obj_type, data.len());
            let full_content = [header.as_bytes().to_vec(), data].concat();

            let mut file = fs::File::create(&path)?;
            file.write_all(&Self::compress_zlib(&full_content)?)?;
            Ok(path.to_str().unwrap().to_string())
        })
        .await
        .map_err(|e| GitError::IOError(io::Error::other(e)))?
    }

    async fn exist(&self, hash: &ObjectHash) -> bool {
        let self_clone = self.clone();
        let hash = *hash;

        tokio::task::spawn_blocking(move || {
            if let Some(kind) = self_clone.hash_kind {
                set_hash_kind(kind);
            }
            let path = self_clone.get_obj_path(&hash);
            Path::exists(&path) || self_clone.get_from_pack(&hash).unwrap().is_some()
        })
        .await
        .unwrap_or(false)
    }

    async fn search(&self, prefix: &str) -> Vec<ObjectHash> {
        let self_clone = self.clone();
        let prefix = prefix.to_string();

        tokio::task::spawn_blocking(move || {
            if let Some(kind) = self_clone.hash_kind {
                set_hash_kind(kind);
            }
            let mut objects = Vec::new();
            // Loose objects
            if let Ok(paths) = fs::read_dir(&self_clone.base_path) {
                for path in paths {
                    let path = path.unwrap().path();
                    if path.is_dir() && path.file_name().unwrap().len() == 2 {
                        let dir_name = path.file_name().unwrap().to_str().unwrap();
                        if !prefix.starts_with(dir_name)
                            && !dir_name.starts_with(&prefix[..std::cmp::min(2, prefix.len())])
                        {
                            continue;
                        }

                        if let Ok(sub_paths) = fs::read_dir(&path) {
                            for sub_path in sub_paths {
                                let sub_path = sub_path.unwrap().path();
                                if sub_path.is_file() {
                                    let parent_name =
                                        path.file_name().unwrap().to_str().unwrap().to_string();
                                    let file_name =
                                        sub_path.file_name().unwrap().to_str().unwrap().to_string();
                                    let full_hash = parent_name + &file_name;
                                    if full_hash.starts_with(&prefix)
                                        && let Ok(hash) = ObjectHash::from_str(&full_hash)
                                    {
                                        objects.push(hash);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Pack objects
            let idxes = self_clone.list_all_idx();
            for idx in idxes {
                if let Ok(objs) = Self::list_idx_objects(&idx) {
                    for obj in objs {
                        if obj.to_string().starts_with(&prefix) {
                            objects.push(obj);
                        }
                    }
                }
            }
            objects
        })
        .await
        .unwrap_or_default()
    }
}

impl LocalStorage {
    /// Lists all object hashes contained in a pack index file. This is used for searching objects by prefix in packs.
    fn list_idx_objects(idx_file: &Path) -> Result<Vec<ObjectHash>, io::Error> {
        let (version, fanout) = Self::read_idx_fanout(idx_file)?;
        let mut idx_file = fs::File::open(idx_file)?;
        let object_count = fanout[255] as u64;
        let hash_size = get_hash_kind().size() as u64;

        let names_offset = match version {
            IdxVersion::V1 => FANOUT,
            IdxVersion::V2 => FANOUT + 8,
        };
        idx_file.seek(io::SeekFrom::Start(names_offset))?;

        let mut objs = Vec::new();
        match version {
            IdxVersion::V1 => {
                if hash_size != 20 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "pack index v1 only supports sha1",
                    ));
                }
                for _ in 0..object_count {
                    let _offset = idx_file.read_u32::<BigEndian>()?;
                    let hash = read_sha(&mut idx_file)?;
                    objs.push(hash);
                }
            }
            IdxVersion::V2 => {
                for _ in 0..object_count {
                    let hash = read_sha(&mut idx_file)?;
                    objs.push(hash);
                }
            }
        }
        Ok(objs)
    }
}
