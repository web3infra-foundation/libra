//! Notes operations: add, list, show, and remove notes attached to commits.
//!
//! Notes are stored as blobs in the object store with mappings persisted in the
//! SQLite `notes` table. Each row maps a (`notes_ref`, `object`) pair to a blob
//! hash. The default notes ref is `refs/notes/commits`.

use std::str::FromStr;

use git_internal::{hash::ObjectHash, internal::object::ObjectTrait};
use sea_orm::{ConnectionTrait, DbErr, Statement};

use crate::{internal::db::get_db_conn_instance, utils::util};

/// Default notes ref namespace.
pub const DEFAULT_NOTES_REF: &str = "refs/notes/commits";

/// Validates that a notes ref starts with `refs/notes/`.
pub fn validate_notes_ref(notes_ref: &str) -> Result<(), NotesError> {
    if notes_ref.starts_with("refs/notes/") {
        Ok(())
    } else {
        Err(NotesError::InvalidNotesRef(notes_ref.to_string()))
    }
}

/// The result of adding a note.
#[derive(Debug, Clone)]
pub struct AddNoteResult {
    pub notes_ref: String,
    pub object: String,
    pub note_hash: String,
}

/// A note entry returned from listing.
///
/// When listing by a specific object that has no note, `note_hash` is `None`.
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub note_hash: Option<String>,
    pub annotated_object: String,
}

/// Errors that can occur during note operations.
#[derive(Debug, thiserror::Error)]
pub enum NotesError {
    #[error("notes ref must start with 'refs/notes/': {0}")]
    InvalidNotesRef(String),

    #[error("note already exists for object '{object}' in {notes_ref}")]
    AlreadyExists { notes_ref: String, object: String },

    #[error("no note found for object '{object}' in {notes_ref}")]
    NotFound { notes_ref: String, object: String },

    #[error("invalid object reference '{0}': {1}")]
    InvalidObject(String, String),

    #[error("HEAD does not point to a commit")]
    HeadUnborn,

    #[error("failed to query notes: {0}")]
    QueryFailed(#[from] DbErr),

    #[error("failed to resolve object: {0}")]
    ResolveFailed(String),

    #[error("failed to store blob: {0}")]
    StoreBlobFailed(#[source] std::io::Error),
}

/// Resolve an optional object string to a commit [`ObjectHash`].
///
/// When `object` is `None`, resolves HEAD. When `Some(s)`, delegates to
/// [`util::get_commit_base`].
pub async fn resolve_object(object: Option<&str>) -> Result<ObjectHash, NotesError> {
    match object {
        Some(s) if !s.is_empty() => resolve_ref(s).await,
        _ => resolve_head().await,
    }
}

async fn resolve_head() -> Result<ObjectHash, NotesError> {
    match crate::internal::head::Head::current_commit_result().await {
        Ok(Some(hash)) => Ok(hash),
        Ok(None) => Err(NotesError::HeadUnborn),
        Err(e) => Err(NotesError::InvalidObject("HEAD".to_string(), e.to_string())),
    }
}

async fn resolve_ref(s: &str) -> Result<ObjectHash, NotesError> {
    // When resolving HEAD explicitly, check for unborn HEAD via the Head API
    // so we surface the correct HeadUnborn error instead of a generic invalid-object.
    if s == "HEAD" {
        match crate::internal::head::Head::current_commit_result().await {
            Ok(Some(hash)) => return Ok(hash),
            Ok(None) => return Err(NotesError::HeadUnborn),
            Err(e) => return Err(NotesError::InvalidObject("HEAD".to_string(), e.to_string())),
        }
    }
    util::get_commit_base(s)
        .await
        .map_err(|e| NotesError::InvalidObject(s.to_string(), e))
}

/// Add a note to an object.
///
/// Creates a blob from `message`, stores it in the object database, and
/// inserts a row into the `notes` table. If `force` is true, overwrites an
/// existing note for the same (`notes_ref`, `object`) pair.
pub async fn add(
    notes_ref: &str,
    object: &str,
    message: &str,
    force: bool,
) -> Result<AddNoteResult, NotesError> {
    validate_notes_ref(notes_ref)?;
    let object_hash = resolve_object(Some(object)).await?;
    let object_str = object_hash.to_string();

    let db = get_db_conn_instance().await;

    // Check if a note already exists for this object in this ref
    let existing = find_note_blob(&db, notes_ref, &object_str).await?;
    if existing.is_some() && !force {
        return Err(NotesError::AlreadyExists {
            notes_ref: notes_ref.to_string(),
            object: object_str,
        });
    }

    // Create and store the blob
    let blob = git_internal::internal::object::blob::Blob::from_content(message);
    let storage = crate::utils::client_storage::ClientStorage::init(crate::utils::path::objects());
    storage
        .put(&blob.id, &blob.data, blob.get_type())
        .map_err(NotesError::StoreBlobFailed)?;
    let note_hash = blob.id.to_string();

    if let Some(_existing_blob) = existing {
        // Update existing row
        db.execute(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "UPDATE notes SET blob = ? WHERE notes_ref = ? AND object = ?",
            [
                note_hash.clone().into(),
                notes_ref.into(),
                object_str.clone().into(),
            ],
        ))
        .await?;
    } else {
        // Insert new row
        db.execute(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "INSERT INTO notes (notes_ref, object, blob) VALUES (?, ?, ?)",
            [
                notes_ref.into(),
                object_str.clone().into(),
                note_hash.clone().into(),
            ],
        ))
        .await?;
    }

    Ok(AddNoteResult {
        notes_ref: notes_ref.to_string(),
        object: object_str,
        note_hash,
    })
}

/// List notes in a notes ref.
///
/// When `object` is `Some`, returns only the note (if any) for that object.
/// When `None`, returns all notes in the given notes ref.
pub async fn list(notes_ref: &str, object: Option<&str>) -> Result<Vec<NoteEntry>, NotesError> {
    validate_notes_ref(notes_ref)?;
    let db = get_db_conn_instance().await;

    if let Some(obj) = object {
        let resolved = resolve_object(Some(obj)).await?;
        let obj_str = resolved.to_string();
        let rows = db
            .query_all(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT blob, object FROM notes WHERE notes_ref = ? AND object = ?",
                [notes_ref.into(), obj_str.clone().into()],
            ))
            .await?;
        if rows.is_empty() {
            return Ok(vec![NoteEntry {
                note_hash: None,
                annotated_object: obj_str,
            }]);
        }
        Ok(rows
            .iter()
            .map(|row| NoteEntry {
                note_hash: Some(row.try_get::<String>("", "blob").unwrap_or_default()),
                annotated_object: row.try_get::<String>("", "object").unwrap_or_default(),
            })
            .collect())
    } else {
        let rows = db
            .query_all(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT blob, object FROM notes WHERE notes_ref = ?",
                [notes_ref.into()],
            ))
            .await?;
        Ok(rows
            .iter()
            .map(|row| NoteEntry {
                note_hash: Some(row.try_get::<String>("", "blob").unwrap_or_default()),
                annotated_object: row.try_get::<String>("", "object").unwrap_or_default(),
            })
            .collect())
    }
}

/// Show the note text for an object.
pub async fn show(
    notes_ref: &str,
    object: Option<&str>,
) -> Result<(String, String, String), NotesError> {
    validate_notes_ref(notes_ref)?;
    let obj_hash = resolve_object(object).await?;
    let obj_str = obj_hash.to_string();

    let db = get_db_conn_instance().await;
    let blob_hash = find_note_blob(&db, notes_ref, &obj_str)
        .await?
        .ok_or_else(|| NotesError::NotFound {
            notes_ref: notes_ref.to_string(),
            object: obj_str.clone(),
        })?;

    // Load the blob to get the text
    let blob_hash_parsed = ObjectHash::from_str(&blob_hash)
        .map_err(|e| NotesError::InvalidObject(blob_hash.clone(), e))?;
    let storage = crate::utils::client_storage::ClientStorage::init(crate::utils::path::objects());
    let data = storage.get(&blob_hash_parsed).map_err(|e| {
        NotesError::InvalidObject(blob_hash.clone(), format!("failed to read blob: {e}"))
    })?;
    let text = String::from_utf8_lossy(&data).to_string();

    Ok((obj_str, blob_hash, text))
}

/// Remove notes for one or more objects.
///
/// Resolves and verifies every object before deleting any row so that a
/// partial-delete on mixed valid/invalid input is impossible: either all
/// targets are valid and the entire removal succeeds, or nothing is deleted
/// and the caller gets the first error.
///
/// Returns the list of (object, note_hash) that were removed.
pub async fn remove(
    notes_ref: &str,
    objects: &[String],
) -> Result<Vec<(String, String)>, NotesError> {
    validate_notes_ref(notes_ref)?;
    let db = get_db_conn_instance().await;

    // Phase 1: resolve and verify every target first.
    let mut to_delete: Vec<(String, String)> = Vec::new();
    for obj in objects {
        let resolved = resolve_object(Some(obj)).await?;
        let obj_str = resolved.to_string();
        let blob_hash = find_note_blob(&db, notes_ref, &obj_str)
            .await?
            .ok_or_else(|| NotesError::NotFound {
                notes_ref: notes_ref.to_string(),
                object: obj_str.clone(),
            })?;
        to_delete.push((obj_str, blob_hash));
    }

    // Phase 2: delete all verified rows.
    for (obj_str, _blob_hash) in &to_delete {
        db.execute(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "DELETE FROM notes WHERE notes_ref = ? AND object = ?",
            [notes_ref.into(), obj_str.clone().into()],
        ))
        .await?;
    }

    Ok(to_delete)
}

/// Find the blob hash for a note, if it exists.
async fn find_note_blob(
    db: &sea_orm::DatabaseConnection,
    notes_ref: &str,
    object: &str,
) -> Result<Option<String>, DbErr> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT blob FROM notes WHERE notes_ref = ? AND object = ?",
            [notes_ref.into(), object.into()],
        ))
        .await?;
    Ok(rows
        .first()
        .map(|row| row.try_get::<String>("", "blob").unwrap_or_default()))
}
