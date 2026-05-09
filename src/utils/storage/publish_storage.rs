//! Publish-specific arbitrary-object storage on top of `object_store`.
//!
//! Per `docs/improvement/publish.md` Phase 2, this module is the
//! Worker- and CLI-side wrapper for non-Git artefacts the publish
//! pipeline writes to R2 (or any S3-compatible bucket): code
//! manifests, file previews keyed by sha256, AI object JSONs, AI
//! bundles, AI graph indexes, and per-sync-run audit blobs. The
//! `Storage` trait in `super` is intentionally Git-only; this
//! wrapper does NOT implement that trait so we cannot accidentally
//! route arbitrary publish keys through Git zlib/header packing.
//!
//! # Path layout
//!
//! Every object lives under a base prefix derived from the
//! repository id and site id:
//!
//! ```text
//! {repo_id}/publish/sites/{site_id}/<relative>
//! ```
//!
//! Callers pass a *relative* path. The wrapper concatenates the
//! base prefix and validates the result so a malicious or buggy
//! caller cannot escape the namespace via `..`, an empty segment,
//! or an absolute path. The full key list lives in publish.md
//! "R2 object layout".
//!
//! # Safety contract
//!
//! `path_relative` must:
//!
//!   * be non-empty
//!   * NOT start with `/`
//!   * NOT contain `\0`
//!   * NOT contain any segment equal to `..`, `.`, or empty
//!   * NOT exceed 4096 chars
//!
//! Violations surface as `PublishStorageError::InvalidKey` so the
//! caller gets actionable feedback instead of a confusing
//! object-store error from later in the request lifecycle.

use std::sync::Arc;

use bytes::Bytes;
use object_store::{ObjectStore, ObjectStoreExt, path::Path as ObjectPath};
use serde::{Serialize, de::DeserializeOwned};

/// Wrapper that scopes arbitrary publish object reads/writes to a
/// single `(repo_id, site_id)` namespace.
#[derive(Clone)]
pub struct PublishStorage {
    inner: Arc<dyn ObjectStore>,
    base_prefix: String,
}

/// Errors surfaced by [`PublishStorage`]. Distinct from the Git
/// [`GitError`](git_internal::errors::GitError) variants so callers
/// can pattern-match on publish-specific failures (path safety, JSON
/// shape, missing object) without re-using Git-flavoured codes.
#[derive(Debug, thiserror::Error)]
pub enum PublishStorageError {
    #[error("publish storage key is invalid: {message}")]
    InvalidKey { message: String },
    #[error("publish object not found at {key}")]
    NotFound { key: String },
    #[error("publish object store error at {key}: {source}")]
    Store {
        key: String,
        #[source]
        source: object_store::Error,
    },
    #[error("publish JSON encode/decode error at {key}: {source}")]
    Json {
        key: String,
        #[source]
        source: serde_json::Error,
    },
}

impl PublishStorage {
    /// Construct a new `PublishStorage` rooted at
    /// `{repo_id}/publish/sites/{site_id}/`.
    ///
    /// Both `repo_id` and `site_id` are validated as non-empty, no
    /// path separators, no `..` segments. The wrapper does not
    /// validate that the underlying bucket exists; the first read
    /// or write will surface that failure as a `Store` error.
    pub fn new(
        inner: Arc<dyn ObjectStore>,
        repo_id: &str,
        site_id: &str,
    ) -> Result<Self, PublishStorageError> {
        validate_id_segment("repo_id", repo_id)?;
        validate_id_segment("site_id", site_id)?;
        let base_prefix = format!("{repo_id}/publish/sites/{site_id}");
        Ok(Self { inner, base_prefix })
    }

    /// Resolve a `relative` path to a fully-qualified `ObjectPath`,
    /// validating safety along the way.
    fn resolve(&self, relative: &str) -> Result<ObjectPath, PublishStorageError> {
        validate_relative_path(relative)?;
        Ok(ObjectPath::from(format!(
            "{}/{}",
            self.base_prefix, relative
        )))
    }

    /// Return the absolute key for `relative` without performing a
    /// store call. Useful for publish.md fixtures and tests.
    pub fn key_for(&self, relative: &str) -> Result<String, PublishStorageError> {
        Ok(self.resolve(relative)?.to_string())
    }

    /// PUT a value as canonical JSON.
    ///
    /// `serde_json::to_vec` is used (not `to_vec_pretty`) so the
    /// stored bytes match the serialized form the contract round-
    /// trip tests expect.
    pub async fn put_json<T: Serialize>(
        &self,
        relative: &str,
        value: &T,
    ) -> Result<(), PublishStorageError> {
        let key = self.resolve(relative)?;
        let bytes = serde_json::to_vec(value).map_err(|source| PublishStorageError::Json {
            key: key.to_string(),
            source,
        })?;
        self.inner
            .put(&key, Bytes::from(bytes).into())
            .await
            .map_err(|source| PublishStorageError::Store {
                key: key.to_string(),
                source,
            })?;
        Ok(())
    }

    /// GET a value as JSON.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        relative: &str,
    ) -> Result<T, PublishStorageError> {
        let key = self.resolve(relative)?;
        let result = self.inner.get(&key).await.map_err(|source| match &source {
            object_store::Error::NotFound { .. } => PublishStorageError::NotFound {
                key: key.to_string(),
            },
            _ => PublishStorageError::Store {
                key: key.to_string(),
                source,
            },
        })?;
        let bytes = result
            .bytes()
            .await
            .map_err(|source| PublishStorageError::Store {
                key: key.to_string(),
                source,
            })?;
        serde_json::from_slice(&bytes).map_err(|source| PublishStorageError::Json {
            key: key.to_string(),
            source,
        })
    }

    /// PUT raw bytes (used for file previews keyed by sha256).
    pub async fn put_bytes(&self, relative: &str, bytes: &[u8]) -> Result<(), PublishStorageError> {
        let key = self.resolve(relative)?;
        self.inner
            .put(&key, Bytes::copy_from_slice(bytes).into())
            .await
            .map_err(|source| PublishStorageError::Store {
                key: key.to_string(),
                source,
            })?;
        Ok(())
    }

    /// GET raw bytes.
    pub async fn get_bytes(&self, relative: &str) -> Result<Vec<u8>, PublishStorageError> {
        let key = self.resolve(relative)?;
        let result = self.inner.get(&key).await.map_err(|source| match &source {
            object_store::Error::NotFound { .. } => PublishStorageError::NotFound {
                key: key.to_string(),
            },
            _ => PublishStorageError::Store {
                key: key.to_string(),
                source,
            },
        })?;
        let bytes = result
            .bytes()
            .await
            .map_err(|source| PublishStorageError::Store {
                key: key.to_string(),
                source,
            })?;
        Ok(bytes.to_vec())
    }

    /// HEAD: returns true iff the object exists and the wrapper can
    /// access it. Distinguishes "not found" (false) from network
    /// failure (returns the `Store` error).
    pub async fn head(&self, relative: &str) -> Result<bool, PublishStorageError> {
        let key = self.resolve(relative)?;
        match self.inner.head(&key).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(source) => Err(PublishStorageError::Store {
                key: key.to_string(),
                source,
            }),
        }
    }

    /// Expose the base prefix for callers that need to log it.
    /// Read-only — the prefix is set at construction time and
    /// cannot be mutated.
    pub fn base_prefix(&self) -> &str {
        &self.base_prefix
    }
}

fn validate_id_segment(label: &str, value: &str) -> Result<(), PublishStorageError> {
    if value.is_empty() {
        return Err(PublishStorageError::InvalidKey {
            message: format!("{label} must not be empty"),
        });
    }
    if value.contains('/') || value.contains('\\') || value.contains('\0') {
        return Err(PublishStorageError::InvalidKey {
            message: format!("{label} must not contain path separators or NUL"),
        });
    }
    if value == "." || value == ".." {
        return Err(PublishStorageError::InvalidKey {
            message: format!("{label} must not be '.' or '..'"),
        });
    }
    if value.len() > 256 {
        return Err(PublishStorageError::InvalidKey {
            message: format!("{label} length is out of range (1..=256)"),
        });
    }
    Ok(())
}

fn validate_relative_path(relative: &str) -> Result<(), PublishStorageError> {
    if relative.is_empty() {
        return Err(PublishStorageError::InvalidKey {
            message: "publish path must not be empty".to_string(),
        });
    }
    if relative.len() > 4096 {
        return Err(PublishStorageError::InvalidKey {
            message: format!("publish path length {} exceeds 4096 chars", relative.len()),
        });
    }
    if relative.starts_with('/') {
        return Err(PublishStorageError::InvalidKey {
            message: "publish path must not start with '/'".to_string(),
        });
    }
    if relative.contains('\0') {
        return Err(PublishStorageError::InvalidKey {
            message: "publish path must not contain NUL".to_string(),
        });
    }
    if relative.contains("//") {
        return Err(PublishStorageError::InvalidKey {
            message: "publish path must not contain doubled slashes".to_string(),
        });
    }
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(PublishStorageError::InvalidKey {
                message: format!(
                    "publish path segment {segment:?} is invalid (no '', '.', or '..')"
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use object_store::memory::InMemory;

    use super::*;

    fn make_storage() -> PublishStorage {
        PublishStorage::new(
            Arc::new(InMemory::new()),
            "11111111-2222-3333-4444-555555555555",
            "00000000-0000-0000-0000-0000publish01",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn json_round_trip() {
        let storage = make_storage();
        let value = serde_json::json!({"hello": "world", "n": 42});
        storage
            .put_json("revisions/abc123/code-manifest.json", &value)
            .await
            .unwrap();
        let got: serde_json::Value = storage
            .get_json("revisions/abc123/code-manifest.json")
            .await
            .unwrap();
        assert_eq!(got, value);
    }

    #[tokio::test]
    async fn bytes_round_trip() {
        let storage = make_storage();
        let body = b"# Demo\n\nHello".to_vec();
        storage
            .put_bytes("revisions/abc123/files/9a0364b9.txt", &body)
            .await
            .unwrap();
        let got = storage
            .get_bytes("revisions/abc123/files/9a0364b9.txt")
            .await
            .unwrap();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn head_distinguishes_present_and_missing() {
        let storage = make_storage();
        assert!(!storage.head("latest.json").await.unwrap());
        storage
            .put_json("latest.json", &serde_json::json!({"x": 1}))
            .await
            .unwrap();
        assert!(storage.head("latest.json").await.unwrap());
    }

    #[tokio::test]
    async fn get_missing_surfaces_typed_error() {
        let storage = make_storage();
        let err = storage
            .get_bytes("does-not-exist.json")
            .await
            .expect_err("missing object must surface as NotFound");
        assert!(matches!(err, PublishStorageError::NotFound { .. }));
    }

    #[tokio::test]
    async fn invalid_paths_are_rejected() {
        let storage = make_storage();
        for bad in [
            "",
            "/leading-slash",
            "double//slash",
            "trailing/",
            "..",
            "../escape",
            "ok/../bad",
            "ok//bad",
            "with\0nul",
        ] {
            let err = storage
                .resolve(bad)
                .expect_err(&format!("path {bad:?} must be rejected"));
            assert!(matches!(err, PublishStorageError::InvalidKey { .. }));
        }
        // Long path (>4096 chars) is rejected.
        let long = "a".repeat(4097);
        let err = storage
            .resolve(&long)
            .expect_err("long path must be rejected");
        assert!(matches!(err, PublishStorageError::InvalidKey { .. }));
    }

    #[test]
    fn invalid_repo_or_site_ids_are_rejected() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        for (repo, site, label) in [
            ("", "ok", "empty repo"),
            ("ok", "", "empty site"),
            ("ok", "..", "site dot-dot"),
            ("ok", "with/slash", "site with slash"),
            ("with\0nul", "ok", "repo NUL"),
        ] {
            assert!(
                PublishStorage::new(Arc::clone(&store), repo, site).is_err(),
                "expected {label} to be rejected"
            );
        }
    }

    #[test]
    fn key_for_returns_full_path() {
        let storage = make_storage();
        assert_eq!(
            storage.key_for("latest.json").unwrap(),
            "11111111-2222-3333-4444-555555555555/publish/sites/00000000-0000-0000-0000-0000publish01/latest.json",
        );
    }
}
