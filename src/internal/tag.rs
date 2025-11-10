use std::str::FromStr;

use crate::command::load_object;
use crate::internal::config::Config;
use crate::internal::db::get_db_conn_instance;
use crate::internal::head::Head;
use crate::internal::model::reference::{ConfigKind, Model};
use crate::utils::client_storage::ClientStorage;
use crate::utils::path;
use git_internal::errors::GitError;
use git_internal::hash::SHA1;
use git_internal::internal::object::ObjectTrait;
use git_internal::internal::object::blob::Blob;
use git_internal::internal::object::commit::Commit;
use git_internal::internal::object::signature::{Signature, SignatureType};
use git_internal::internal::object::tag::Tag as git_internalTag;
use git_internal::internal::object::tree::Tree;
use git_internal::internal::object::types::ObjectType;

// Constants for tag references
const TAG_REF_PREFIX: &str = "refs/tags/";
const DEFAULT_USER: &str = "user";
const DEFAULT_EMAIL: &str = "user@example.com";
const UNKNOWN_TAG: &str = "<unknown>";

/// Enum representing the possible object types a tag can point to.
#[derive(Debug)]
pub enum TagObject {
    Commit(Commit),
    Tag(git_internalTag),
    Tree(Tree),
    Blob(Blob),
}

impl TagObject {
    pub fn get_type(&self) -> ObjectType {
        match self {
            TagObject::Commit(_) => ObjectType::Commit,
            TagObject::Tag(_) => ObjectType::Tag,
            TagObject::Tree(_) => ObjectType::Tree,
            TagObject::Blob(_) => ObjectType::Blob,
        }
    }

    pub fn to_data(&self) -> Result<Vec<u8>, GitError> {
        match self {
            TagObject::Commit(c) => c.to_data(),
            TagObject::Tag(t) => t.to_data(),
            TagObject::Tree(t) => t.to_data(),
            TagObject::Blob(b) => b.to_data(),
        }
    }
}

/// Represents a tag in the context of Libra, containing its name and the object it points to.
pub struct Tag {
    pub name: String,
    pub object: TagObject,
}

/// Creates a new tag, either lightweight or annotated, pointing to the current HEAD commit.
///
/// * `name` - The name of the tag.
/// * `message` - If `Some`, creates an annotated tag with the given message. If `None`, creates a lightweight tag.
pub async fn create(name: &str, message: Option<String>, force: bool) -> Result<(), anyhow::Error> {
    let head_commit_id = Head::current_commit()
        .await
        .ok_or_else(|| anyhow::anyhow!("Cannot create tag: HEAD does not point to a commit"))?;

    let db = get_db_conn_instance().await;
    let full_tag_name = format!("{}{}", TAG_REF_PREFIX, name);

    let sql = "SELECT * FROM reference WHERE name = ?1 AND kind = ?2";
    let mut rows = db
        .query(
            sql,
            turso::params![full_tag_name.clone(), ConfigKind::Tag.as_str()],
        )
        .await?;
    let exists = rows.next().await?.is_some();

    if exists && !force {
        return Err(anyhow::anyhow!("Tag '{}' already exists", name));
    } else if exists && force {
        // Delete existing tag if force is true
        delete(name).await?;
    }

    let ref_target_id: SHA1;
    if let Some(msg) = message {
        // Create an annotated tag object
        let user_name = Config::get("user", None, "name")
            .await
            .map(|m| m.value)
            .unwrap_or_else(|| DEFAULT_USER.to_string());
        let user_email = Config::get("user", None, "email")
            .await
            .map(|m| m.value)
            .unwrap_or_else(|| DEFAULT_EMAIL.to_string());
        let tagger_signature = Signature::new(SignatureType::Tagger, user_name, user_email);

        let git_internal_tag = git_internalTag::new(
            head_commit_id,
            ObjectType::Commit,
            name.to_string(),
            tagger_signature,
            msg,
        );

        // The ID is now calculated inside git_internalTag::new, so we can use it directly.
        let tag_data = git_internal_tag.to_data()?;
        let storage = ClientStorage::init(path::objects());
        storage.put(&git_internal_tag.id, &tag_data, git_internal_tag.get_type())?;

        ref_target_id = git_internal_tag.id;
    } else {
        // For lightweight tags, the target is the commit itself
        ref_target_id = head_commit_id;
    };

    // Save the reference in the database
    let db_conn = get_db_conn_instance().await;
    let sql = "INSERT INTO reference (name, kind, `commit`) VALUES (?1, ?2, ?3)";
    db_conn
        .execute(
            sql,
            turso::params![
                full_tag_name.clone(),
                ConfigKind::Tag.as_str(),
                ref_target_id.to_string()
            ],
        )
        .await?;

    Ok(())
}

/// Lists all tags available in the repository.
pub async fn list() -> Result<Vec<Tag>, anyhow::Error> {
    let db_conn = get_db_conn_instance().await;
    let sql = "SELECT * FROM reference WHERE kind = ?1";
    let mut rows = db_conn
        .query(sql, turso::params![ConfigKind::Tag.as_str()])
        .await?;

    let mut models = Vec::new();
    while let Some(row) = rows.next().await? {
        models.push(Model::from_row(&row).unwrap());
    }

    let mut tags = Vec::new();
    for m in models {
        let commit_str = m.commit.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Tag '{}' is missing commit field",
                m.name.as_deref().unwrap_or(UNKNOWN_TAG)
            )
        })?;
        let object_id =
            SHA1::from_str(commit_str).map_err(|e| anyhow::anyhow!("Invalid SHA1: {}", e))?;
        let object = load_object_trait(&object_id).await?;
        let tag_name = m
            .name
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Tag is missing name field"))?
            .strip_prefix(TAG_REF_PREFIX)
            .unwrap_or_else(|| m.name.as_ref().expect("Name field should exist"))
            .to_string();
        tags.push(Tag {
            name: tag_name,
            object,
        });
    }
    Ok(tags)
}

/// Deletes a tag reference from the repository.
pub async fn delete(name: &str) -> Result<(), anyhow::Error> {
    let db_conn = get_db_conn_instance().await;
    let full_ref_name = format!("{}{}", TAG_REF_PREFIX, name);

    let sql = "DELETE FROM reference WHERE name = ?1 AND kind = ?2";
    let rows_affected = db_conn
        .execute(sql, turso::params![full_ref_name, ConfigKind::Tag.as_str()])
        .await?;

    if rows_affected == 0 {
        Err(anyhow::anyhow!("tag '{}' not found", name))
    } else {
        Ok(())
    }
}

/// Finds a tag by name and returns the tag object and the final commit
pub async fn find_tag_and_commit(name: &str) -> Result<Option<(TagObject, Commit)>, GitError> {
    let db_conn = get_db_conn_instance().await;
    let full_ref_name = format!("{}{}", TAG_REF_PREFIX, name);

    let sql = "SELECT * FROM reference WHERE name = ?1 AND kind = ?2";
    let mut rows = db_conn
        .query(sql, turso::params![full_ref_name, ConfigKind::Tag.as_str()])
        .await
        .map_err(|e| GitError::CustomError(e.to_string()))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| GitError::CustomError(e.to_string()))?
    {
        let m = Model::from_row(&row).map_err(|e| GitError::CustomError(e.to_string()))?;
        let commit_str = m
            .commit
            .as_ref()
            .ok_or_else(|| GitError::CustomError("Tag is missing commit field".to_string()))?;
        let target_id = SHA1::from_str(commit_str)
            .map_err(|_| GitError::InvalidHashValue(commit_str.to_string()))?;
        let ref_object = load_object_trait(&target_id).await?;

        // If the ref points to a tag object, dereference it to get the commit
        let commit_id = if let TagObject::Tag(tag_object) = &ref_object {
            tag_object.object_hash
        } else {
            target_id
        };

        let commit: Commit = load_object(&commit_id)?;
        Ok(Some((ref_object, commit)))
    } else {
        Ok(None)
    }
}

/// Load a Git object and return it as a `TagObject`.
pub async fn load_object_trait(hash: &SHA1) -> Result<TagObject, GitError> {
    // Use ClientStorage to get the object type first
    let storage = ClientStorage::init(path::objects());
    let obj_type = storage
        .get_object_type(hash)
        .map_err(|e| GitError::ObjectNotFound(format!("{}: {}", hash, e)))?;
    match obj_type {
        ObjectType::Commit => {
            let commit = load_object::<Commit>(hash)
                .map_err(|e| GitError::ObjectNotFound(format!("{}: {}", hash, e)))?;
            Ok(TagObject::Commit(commit))
        }
        ObjectType::Tag => {
            let tag = load_object::<git_internalTag>(hash)
                .map_err(|e| GitError::ObjectNotFound(format!("{}: {}", hash, e)))?;
            Ok(TagObject::Tag(tag))
        }
        ObjectType::Tree => {
            let tree = load_object::<Tree>(hash)
                .map_err(|e| GitError::ObjectNotFound(format!("{}: {}", hash, e)))?;
            Ok(TagObject::Tree(tree))
        }
        ObjectType::Blob => {
            let blob = load_object::<Blob>(hash)
                .map_err(|e| GitError::ObjectNotFound(format!("{}: {}", hash, e)))?;
            Ok(TagObject::Blob(blob))
        }
        _ => Err(GitError::ObjectNotFound(hash.to_string())),
    }
}
