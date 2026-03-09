//! Thread projection types for the Libra runtime layer.
//!
//! These records do not replace immutable `Intent` history. They materialize
//! the current conversational view over an Intent DAG so Libra can resume the
//! active branch, render participants, and track thread-local metadata without
//! rewriting snapshot objects.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use git_internal::internal::object::types::{ActorKind, ActorRef};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    EntityTrait, QueryFilter, QueryOrder, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::internal::model::{ai_thread, ai_thread_intent, ai_thread_participant};

/// Libra-side identifier for a projected conversation thread.
pub type ThreadId = Uuid;

/// Current conversational projection over a related Intent DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadProjection {
    /// Stable projection key for the thread inside Libra storage.
    pub thread_id: ThreadId,
    /// Human-readable thread title shown in UI surfaces when available.
    pub title: Option<String>,
    /// Actor that owns or initiated the thread.
    pub owner: ActorRef,
    /// Human and agent members currently attached to the thread projection.
    #[serde(default)]
    pub participants: Vec<ThreadParticipant>,
    /// Intent currently selected as the default resume target for the thread.
    pub current_intent_id: Option<Uuid>,
    /// Most recently linked Intent revision in the thread, regardless of focus.
    pub latest_intent_id: Option<Uuid>,
    /// Ordered Intent membership view, including branch-head markers.
    #[serde(default)]
    pub intents: Vec<ThreadIntentRef>,
    /// Optional projection-only routing or presentation hints.
    pub metadata: Option<Value>,
    /// Whether the thread is closed for normal mutation in the runtime/UI.
    pub archived: bool,
    /// Projection creation time.
    pub created_at: DateTime<Utc>,
    /// Last time Libra updated this projection row.
    pub updated_at: DateTime<Utc>,
    /// Projection revision maintained by Libra for updates and rebuilds.
    pub version: i64,
}

/// Actor membership in a thread projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadParticipant {
    /// Actor included in the thread membership list.
    pub actor: ActorRef,
    /// Thread-local role used for routing, permissions, or presentation.
    pub role: ThreadParticipantRole,
    /// Time at which the actor joined the projected thread.
    pub joined_at: DateTime<Utc>,
}

/// Intent membership state within a thread projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadIntentRef {
    /// Intent revision that belongs to this thread's DAG view.
    pub intent_id: Uuid,
    /// Stable display / traversal order for Intents within the thread.
    pub ordinal: i64,
    /// Whether this Intent is currently a branch head in the projected DAG.
    pub is_head: bool,
    /// Time at which the Intent was attached to this thread.
    pub linked_at: DateTime<Utc>,
    /// Why Libra linked this Intent into the thread projection.
    pub link_reason: ThreadIntentLinkReason,
}

/// Role of an actor inside a projected thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadParticipantRole {
    /// Primary owner or initiator of the thread.
    Owner,
    /// Regular participant who can contribute to the thread.
    Member,
    /// Reviewer focused on validation or approval work.
    Reviewer,
    /// Read-mostly observer included for visibility.
    Observer,
}

/// Reason an Intent revision became part of a projected thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadIntentLinkReason {
    /// Initial Intent that seeded the thread.
    Seed,
    /// A new Intent revision linked from an existing thread branch.
    Revision,
    /// A branch split created an additional line of work in the thread.
    Split,
    /// Multiple branches were joined back into one conversational view.
    Merge,
    /// A follow-up request was attached to continue the thread.
    Followup,
    /// Existing history was imported into Libra from another source.
    Imported,
}

impl ThreadProjection {
    /// Persist a new thread projection and all child rows to the database.
    pub async fn create(&self, db: &DatabaseConnection) -> Result<()> {
        let txn = db
            .begin()
            .await
            .context("Failed to start transaction for thread projection create")?;

        if let Err(err) = self.insert_projection(&txn).await {
            txn.rollback().await.with_context(|| {
                format!(
                    "Failed to rollback thread projection create for {}",
                    self.thread_id
                )
            })?;
            return Err(err);
        }

        txn.commit().await.with_context(|| {
            format!(
                "Failed to commit thread projection create for {}",
                self.thread_id
            )
        })?;
        Ok(())
    }

    /// Update an existing thread projection and replace its child rows.
    pub async fn update(&self, db: &DatabaseConnection) -> Result<()> {
        let txn = db
            .begin()
            .await
            .context("Failed to start transaction for thread projection update")?;

        if let Err(err) = self.update_projection(&txn).await {
            txn.rollback().await.with_context(|| {
                format!(
                    "Failed to rollback thread projection update for {}",
                    self.thread_id
                )
            })?;
            return Err(err);
        }

        txn.commit().await.with_context(|| {
            format!(
                "Failed to commit thread projection update for {}",
                self.thread_id
            )
        })?;
        Ok(())
    }

    /// Load a thread projection and its child rows from the database.
    pub async fn find_by_id<C: ConnectionTrait>(
        db: &C,
        thread_id: ThreadId,
    ) -> Result<Option<Self>> {
        let thread_id_text = thread_id.to_string();
        let model = ai_thread::Entity::find_by_id(thread_id_text.clone())
            .one(db)
            .await
            .with_context(|| format!("Failed to query thread projection {thread_id}"))?;

        let Some(model) = model else {
            return Ok(None);
        };

        let participants = ai_thread_participant::Entity::find()
            .filter(ai_thread_participant::Column::ThreadId.eq(thread_id_text.clone()))
            .order_by_asc(ai_thread_participant::Column::JoinedAt)
            .order_by_asc(ai_thread_participant::Column::ActorKind)
            .order_by_asc(ai_thread_participant::Column::ActorId)
            .all(db)
            .await
            .with_context(|| {
                format!("Failed to query participants for thread projection {thread_id}")
            })?
            .into_iter()
            .map(|row| thread_participant_from_model(thread_id, row))
            .collect::<Result<Vec<_>>>()?;

        let intents = ai_thread_intent::Entity::find()
            .filter(ai_thread_intent::Column::ThreadId.eq(thread_id_text))
            .order_by_asc(ai_thread_intent::Column::Ordinal)
            .order_by_asc(ai_thread_intent::Column::LinkedAt)
            .order_by_asc(ai_thread_intent::Column::IntentId)
            .all(db)
            .await
            .with_context(|| format!("Failed to query intents for thread projection {thread_id}"))?
            .into_iter()
            .map(|row| thread_intent_from_model(thread_id, row))
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(ThreadProjection {
            thread_id,
            title: model.title,
            owner: actor_from_row(
                &model.owner_kind,
                &model.owner_id,
                model.owner_display_name,
                thread_id,
                "owner",
            )?,
            participants,
            current_intent_id: optional_uuid_from_row(
                model.current_intent_id.as_deref(),
                thread_id,
                "current_intent_id",
            )?,
            latest_intent_id: optional_uuid_from_row(
                model.latest_intent_id.as_deref(),
                thread_id,
                "latest_intent_id",
            )?,
            intents,
            metadata: metadata_from_row(model.metadata_json.as_deref(), thread_id)?,
            archived: model.archived,
            created_at: datetime_from_row(model.created_at, thread_id, "created_at")?,
            updated_at: datetime_from_row(model.updated_at, thread_id, "updated_at")?,
            version: model.version,
        }))
    }

    async fn insert_projection<C: ConnectionTrait>(&self, db: &C) -> Result<()> {
        let active_model = ai_thread::ActiveModel {
            thread_id: Set(self.thread_id.to_string()),
            title: Set(self.title.clone()),
            owner_kind: Set(self.owner.kind().to_string()),
            owner_id: Set(self.owner.id().to_string()),
            owner_display_name: Set(self.owner.display_name().map(str::to_owned)),
            current_intent_id: Set(self.current_intent_id.as_ref().map(Uuid::to_string)),
            latest_intent_id: Set(self.latest_intent_id.as_ref().map(Uuid::to_string)),
            metadata_json: Set(metadata_to_row(self.metadata.as_ref(), self.thread_id)?),
            archived: Set(self.archived),
            version: Set(self.version),
            created_at: Set(self.created_at.timestamp()),
            updated_at: Set(self.updated_at.timestamp()),
        };

        active_model.insert(db).await.with_context(|| {
            format!(
                "Failed to insert thread projection row for {}",
                self.thread_id
            )
        })?;

        self.replace_related_rows(db).await
    }

    async fn update_projection<C: ConnectionTrait>(&self, db: &C) -> Result<()> {
        let Some(model) = ai_thread::Entity::find_by_id(self.thread_id.to_string())
            .one(db)
            .await
            .with_context(|| {
                format!(
                    "Failed to query thread projection {} for update",
                    self.thread_id
                )
            })?
        else {
            bail!("Thread projection {} does not exist", self.thread_id);
        };

        let mut active_model: ai_thread::ActiveModel = model.into();
        active_model.title = Set(self.title.clone());
        active_model.owner_kind = Set(self.owner.kind().to_string());
        active_model.owner_id = Set(self.owner.id().to_string());
        active_model.owner_display_name = Set(self.owner.display_name().map(str::to_owned));
        active_model.current_intent_id = Set(self.current_intent_id.as_ref().map(Uuid::to_string));
        active_model.latest_intent_id = Set(self.latest_intent_id.as_ref().map(Uuid::to_string));
        active_model.metadata_json = Set(metadata_to_row(self.metadata.as_ref(), self.thread_id)?);
        active_model.archived = Set(self.archived);
        active_model.version = Set(self.version);
        active_model.created_at = Set(self.created_at.timestamp());
        active_model.updated_at = Set(self.updated_at.timestamp());
        active_model.update(db).await.with_context(|| {
            format!(
                "Failed to update thread projection row for {}",
                self.thread_id
            )
        })?;

        self.replace_related_rows(db).await
    }

    async fn replace_related_rows<C: ConnectionTrait>(&self, db: &C) -> Result<()> {
        let thread_id_text = self.thread_id.to_string();

        ai_thread_participant::Entity::delete_many()
            .filter(ai_thread_participant::Column::ThreadId.eq(thread_id_text.clone()))
            .exec(db)
            .await
            .with_context(|| {
                format!(
                    "Failed to clear participants for thread projection {}",
                    self.thread_id
                )
            })?;

        ai_thread_intent::Entity::delete_many()
            .filter(ai_thread_intent::Column::ThreadId.eq(thread_id_text.clone()))
            .exec(db)
            .await
            .with_context(|| {
                format!(
                    "Failed to clear intents for thread projection {}",
                    self.thread_id
                )
            })?;

        for participant in &self.participants {
            let active_model = ai_thread_participant::ActiveModel {
                thread_id: Set(thread_id_text.clone()),
                actor_kind: Set(participant.actor.kind().to_string()),
                actor_id: Set(participant.actor.id().to_string()),
                actor_display_name: Set(participant.actor.display_name().map(str::to_owned)),
                role: Set(thread_participant_role_to_row(&participant.role).to_string()),
                joined_at: Set(participant.joined_at.timestamp()),
            };

            active_model.insert(db).await.with_context(|| {
                format!(
                    "Failed to insert participant {} for thread projection {}",
                    participant.actor.id(),
                    self.thread_id
                )
            })?;
        }

        for intent in &self.intents {
            let active_model = ai_thread_intent::ActiveModel {
                thread_id: Set(thread_id_text.clone()),
                intent_id: Set(intent.intent_id.to_string()),
                ordinal: Set(intent.ordinal),
                is_head: Set(intent.is_head),
                linked_at: Set(intent.linked_at.timestamp()),
                link_reason: Set(thread_intent_link_reason_to_row(&intent.link_reason).to_string()),
            };

            active_model.insert(db).await.with_context(|| {
                format!(
                    "Failed to insert intent {} for thread projection {}",
                    intent.intent_id, self.thread_id
                )
            })?;
        }

        Ok(())
    }
}

fn actor_from_row(
    kind: &str,
    id: &str,
    display_name: Option<String>,
    thread_id: ThreadId,
    field_name: &str,
) -> Result<ActorRef> {
    let mut actor = ActorRef::new(ActorKind::from(kind.to_string()), id.to_string())
        .map_err(|err| anyhow::anyhow!(err))
        .with_context(|| {
            format!(
                "Invalid actor in {field_name} for thread projection {thread_id}: kind={kind}, id={id}"
            )
        })?;
    actor.set_display_name(display_name);
    Ok(actor)
}

fn optional_uuid_from_row(
    raw: Option<&str>,
    thread_id: ThreadId,
    field_name: &str,
) -> Result<Option<Uuid>> {
    raw.map(|value| {
        Uuid::parse_str(value).with_context(|| {
            format!("Invalid {field_name} value in thread projection {thread_id}: {value}")
        })
    })
    .transpose()
}

fn datetime_from_row(raw: i64, thread_id: ThreadId, field_name: &str) -> Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(raw, 0).with_context(|| {
        format!("Invalid {field_name} timestamp in thread projection {thread_id}: {raw}")
    })
}

fn metadata_to_row(metadata: Option<&Value>, thread_id: ThreadId) -> Result<Option<String>> {
    metadata
        .map(|value| {
            serde_json::to_string(value).with_context(|| {
                format!("Failed to serialize metadata for thread projection {thread_id}")
            })
        })
        .transpose()
}

fn metadata_from_row(raw: Option<&str>, thread_id: ThreadId) -> Result<Option<Value>> {
    raw.map(|value| {
        serde_json::from_str(value)
            .with_context(|| format!("Failed to parse metadata for thread projection {thread_id}"))
    })
    .transpose()
}

fn thread_participant_from_model(
    thread_id: ThreadId,
    model: ai_thread_participant::Model,
) -> Result<ThreadParticipant> {
    Ok(ThreadParticipant {
        actor: actor_from_row(
            &model.actor_kind,
            &model.actor_id,
            model.actor_display_name,
            thread_id,
            "participant",
        )?,
        role: thread_participant_role_from_row(&model.role, thread_id)?,
        joined_at: datetime_from_row(model.joined_at, thread_id, "participant.joined_at")?,
    })
}

fn thread_intent_from_model(
    thread_id: ThreadId,
    model: ai_thread_intent::Model,
) -> Result<ThreadIntentRef> {
    Ok(ThreadIntentRef {
        intent_id: Uuid::parse_str(&model.intent_id).with_context(|| {
            format!(
                "Invalid intent_id in thread projection {}: {}",
                thread_id, model.intent_id
            )
        })?,
        ordinal: model.ordinal,
        is_head: model.is_head,
        linked_at: datetime_from_row(model.linked_at, thread_id, "intent.linked_at")?,
        link_reason: thread_intent_link_reason_from_row(&model.link_reason, thread_id)?,
    })
}

fn thread_participant_role_to_row(role: &ThreadParticipantRole) -> &'static str {
    match role {
        ThreadParticipantRole::Owner => "owner",
        ThreadParticipantRole::Member => "member",
        ThreadParticipantRole::Reviewer => "reviewer",
        ThreadParticipantRole::Observer => "observer",
    }
}

fn thread_participant_role_from_row(
    raw: &str,
    thread_id: ThreadId,
) -> Result<ThreadParticipantRole> {
    match raw {
        "owner" => Ok(ThreadParticipantRole::Owner),
        "member" => Ok(ThreadParticipantRole::Member),
        "reviewer" => Ok(ThreadParticipantRole::Reviewer),
        "observer" => Ok(ThreadParticipantRole::Observer),
        _ => bail!(
            "Invalid participant role in thread projection {}: {}",
            thread_id,
            raw
        ),
    }
}

fn thread_intent_link_reason_to_row(reason: &ThreadIntentLinkReason) -> &'static str {
    match reason {
        ThreadIntentLinkReason::Seed => "seed",
        ThreadIntentLinkReason::Revision => "revision",
        ThreadIntentLinkReason::Split => "split",
        ThreadIntentLinkReason::Merge => "merge",
        ThreadIntentLinkReason::Followup => "followup",
        ThreadIntentLinkReason::Imported => "imported",
    }
}

fn thread_intent_link_reason_from_row(
    raw: &str,
    thread_id: ThreadId,
) -> Result<ThreadIntentLinkReason> {
    match raw {
        "seed" => Ok(ThreadIntentLinkReason::Seed),
        "revision" => Ok(ThreadIntentLinkReason::Revision),
        "split" => Ok(ThreadIntentLinkReason::Split),
        "merge" => Ok(ThreadIntentLinkReason::Merge),
        "followup" => Ok(ThreadIntentLinkReason::Followup),
        "imported" => Ok(ThreadIntentLinkReason::Imported),
        _ => bail!(
            "Invalid intent link reason in thread projection {}: {}",
            thread_id,
            raw
        ),
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{Database, Statement};
    use serde_json::json;

    use super::*;

    const BOOTSTRAP_SQL: &str = include_str!("../../../../sql/sqlite_20260309_init.sql");

    async fn setup_test_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let backend = db.get_database_backend();
        db.execute(Statement::from_string(backend, BOOTSTRAP_SQL))
            .await
            .unwrap();
        db
    }

    fn ts(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).unwrap()
    }

    fn actor(kind: &str, id: &str, display_name: Option<&str>) -> ActorRef {
        let mut actor = ActorRef::new(kind, id.to_string()).unwrap();
        actor.set_display_name(display_name.map(str::to_owned));
        actor
    }

    fn sample_projection() -> ThreadProjection {
        ThreadProjection {
            thread_id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            title: Some("Planner thread".to_string()),
            owner: actor("human", "user-1", Some("Alice")),
            participants: vec![
                ThreadParticipant {
                    actor: actor("human", "user-1", Some("Alice")),
                    role: ThreadParticipantRole::Owner,
                    joined_at: ts(1_700_000_000),
                },
                ThreadParticipant {
                    actor: actor("agent", "planner", Some("Planner")),
                    role: ThreadParticipantRole::Member,
                    joined_at: ts(1_700_000_010),
                },
            ],
            current_intent_id: Some(
                Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
            ),
            latest_intent_id: Some(
                Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap(),
            ),
            intents: vec![
                ThreadIntentRef {
                    intent_id: Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
                    ordinal: 0,
                    is_head: false,
                    linked_at: ts(1_700_000_020),
                    link_reason: ThreadIntentLinkReason::Seed,
                },
                ThreadIntentRef {
                    intent_id: Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap(),
                    ordinal: 1,
                    is_head: true,
                    linked_at: ts(1_700_000_030),
                    link_reason: ThreadIntentLinkReason::Revision,
                },
            ],
            metadata: Some(json!({
                "workspace": "repo-a",
                "resume": {
                    "mode": "auto"
                }
            })),
            archived: false,
            created_at: ts(1_700_000_040),
            updated_at: ts(1_700_000_050),
            version: 1,
        }
    }

    #[tokio::test]
    async fn thread_projection_create_persists_thread_and_children() {
        let db = setup_test_db().await;
        let projection = sample_projection();

        projection.create(&db).await.unwrap();

        let thread_row = ai_thread::Entity::find_by_id(projection.thread_id.to_string())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(thread_row.title.as_deref(), Some("Planner thread"));
        assert_eq!(thread_row.owner_kind, "human");
        assert_eq!(thread_row.owner_display_name.as_deref(), Some("Alice"));

        let participant_rows = ai_thread_participant::Entity::find()
            .filter(ai_thread_participant::Column::ThreadId.eq(projection.thread_id.to_string()))
            .all(&db)
            .await
            .unwrap();
        assert_eq!(participant_rows.len(), 2);

        let intent_rows = ai_thread_intent::Entity::find()
            .filter(ai_thread_intent::Column::ThreadId.eq(projection.thread_id.to_string()))
            .all(&db)
            .await
            .unwrap();
        assert_eq!(intent_rows.len(), 2);
    }

    #[tokio::test]
    async fn thread_projection_find_by_id_reconstructs_full_projection() {
        let db = setup_test_db().await;
        let projection = sample_projection();
        projection.create(&db).await.unwrap();

        let stored = ThreadProjection::find_by_id(&db, projection.thread_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(stored, projection);
    }

    #[tokio::test]
    async fn thread_projection_update_replaces_existing_rows() {
        let db = setup_test_db().await;
        let projection = sample_projection();
        projection.create(&db).await.unwrap();

        let updated = ThreadProjection {
            thread_id: projection.thread_id,
            title: Some("Release review".to_string()),
            owner: actor("agent", "coordinator", Some("Coordinator")),
            participants: vec![ThreadParticipant {
                actor: actor("human", "reviewer-1", Some("Bob")),
                role: ThreadParticipantRole::Reviewer,
                joined_at: ts(1_700_000_100),
            }],
            current_intent_id: Some(
                Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap(),
            ),
            latest_intent_id: Some(
                Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap(),
            ),
            intents: vec![ThreadIntentRef {
                intent_id: Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap(),
                ordinal: 0,
                is_head: true,
                linked_at: ts(1_700_000_110),
                link_reason: ThreadIntentLinkReason::Followup,
            }],
            metadata: Some(json!({
                "workspace": "repo-b",
                "archived_by": "system"
            })),
            archived: true,
            created_at: projection.created_at,
            updated_at: ts(1_700_000_120),
            version: 2,
        };

        updated.update(&db).await.unwrap();

        let stored = ThreadProjection::find_by_id(&db, updated.thread_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, updated);

        let participant_rows = ai_thread_participant::Entity::find()
            .filter(ai_thread_participant::Column::ThreadId.eq(updated.thread_id.to_string()))
            .all(&db)
            .await
            .unwrap();
        assert_eq!(participant_rows.len(), 1);
        assert_eq!(participant_rows[0].actor_id, "reviewer-1");

        let intent_rows = ai_thread_intent::Entity::find()
            .filter(ai_thread_intent::Column::ThreadId.eq(updated.thread_id.to_string()))
            .all(&db)
            .await
            .unwrap();
        assert_eq!(intent_rows.len(), 1);
        assert_eq!(
            intent_rows[0].intent_id,
            "55555555-5555-5555-5555-555555555555"
        );
    }

    #[tokio::test]
    async fn thread_projection_update_returns_error_for_missing_thread() {
        let db = setup_test_db().await;
        let projection = sample_projection();

        let err = projection.update(&db).await.unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn thread_projection_find_by_id_returns_none_when_missing() {
        let db = setup_test_db().await;

        let stored = ThreadProjection::find_by_id(
            &db,
            Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(),
        )
        .await
        .unwrap();

        assert!(stored.is_none());
    }
}
