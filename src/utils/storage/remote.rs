//! Remote object storage backend for Git objects
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use git_internal::{errors::GitError, hash::ObjectHash, internal::object::types::ObjectType};
use object_store::{ObjectStore, path::Path as ObjectPath};

use super::Storage;

/// Remote object storage backend
/// Adapts object_store crate to Libra's StorageTrait
pub struct RemoteStorage {
    inner: Arc<dyn ObjectStore>,
}

impl RemoteStorage {
    /// Create a new RemoteStorage instance
    pub fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self { inner }
    }

    /// Convert ObjectHash to storage path (aa/bbcc...)
    fn hash_to_path(&self, hash: &ObjectHash) -> ObjectPath {
        let h = hash.to_string();
        ObjectPath::from(format!("{}/{}", &h[0..2], &h[2..]))
    }
}

#[async_trait]
impl Storage for RemoteStorage {
    /// Get object from remote storage
    /// Downloads, decompresses, and strips header
    async fn get(&self, hash: &ObjectHash) -> Result<(Vec<u8>, ObjectType), GitError> {
        let path = self.hash_to_path(hash);
        let result = self
            .inner
            .get(&path)
            .await
            .map_err(|e| GitError::ObjectNotFound(format!("Remote object not found: {}", e)))?;

        let bytes = result
            .bytes()
            .await
            .map_err(|e| GitError::IOError(std::io::Error::other(e)))?;

        // Decompress
        let mut decoder = flate2::read::ZlibDecoder::new(&bytes[..]);
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed)?;

        // Strip header
        let end_of_header = decompressed
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| GitError::InvalidObjectInfo("No header terminator".into()))?;

        // Parse type
        let header_str = std::str::from_utf8(&decompressed[..end_of_header])
            .map_err(|_| GitError::InvalidObjectInfo("Invalid UTF-8 in header".into()))?;
        let obj_type_str = header_str.split(' ').next().unwrap_or("");
        let obj_type = ObjectType::from_string(obj_type_str)?;

        Ok((decompressed[end_of_header + 1..].to_vec(), obj_type))
    }

    /// Put object to remote storage
    /// Constructs header, compresses, and uploads
    async fn put(
        &self,
        hash: &ObjectHash,
        data: &[u8],
        obj_type: ObjectType,
    ) -> Result<String, GitError> {
        let path = self.hash_to_path(hash);

        // Construct header + content
        let header = format!("{} {}\0", obj_type, data.len());
        let mut full_content = Vec::with_capacity(header.len() + data.len());
        full_content.extend_from_slice(header.as_bytes());
        full_content.extend_from_slice(data);

        // Compress
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &full_content)?;
        let compressed = encoder.finish()?;

        // Upload
        self.inner
            .put(&path, Bytes::from(compressed).into())
            .await
            .map_err(|e| GitError::IOError(std::io::Error::other(e)))?;

        Ok(path.to_string())
    }

    async fn exist(&self, hash: &ObjectHash) -> bool {
        let path = self.hash_to_path(hash);
        self.inner.head(&path).await.is_ok()
    }

    async fn search(&self, _prefix: &str) -> Vec<ObjectHash> {
        // Prefix search implementation is complex due to "aa/bb..." structure.
        // A prefix "aabb" maps to "aa/bb".
        // A prefix "a" maps to "a*".
        // S3 listing is expensive. For now, return empty as this is mainly used for local maintenance.
        Vec::new()
    }
}
