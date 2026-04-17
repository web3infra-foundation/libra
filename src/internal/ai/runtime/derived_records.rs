use anyhow::{Context, Result};
use chrono::Utc;
use sea_orm::{ActiveValue::Set, ConnectionTrait, EntityTrait, sea_query::OnConflict};
use uuid::Uuid;

use crate::internal::model::ai_thread;

pub(crate) async fn ensure_runtime_thread<C>(db: &C, thread_id: Uuid) -> Result<()>
where
    C: ConnectionTrait,
{
    let now = Utc::now().timestamp();
    let thread_id = thread_id.to_string();
    let mut on_conflict = OnConflict::column(ai_thread::Column::ThreadId);
    on_conflict.do_nothing();

    ai_thread::Entity::insert(ai_thread::ActiveModel {
        thread_id: Set(thread_id.clone()),
        title: Set(Some("libra code workflow".to_string())),
        owner_kind: Set("system".to_string()),
        owner_id: Set("libra-runtime".to_string()),
        owner_display_name: Set(Some("Libra Runtime".to_string())),
        current_intent_id: Set(None),
        latest_intent_id: Set(None),
        metadata_json: Set(Some(
            serde_json::json!({
                "createdBy": "runtime-derived-record-store",
                "purpose": "foreign-key anchor for validation and decision records"
            })
            .to_string(),
        )),
        archived: Set(false),
        version: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(on_conflict)
    .exec_without_returning(db)
    .await
    .with_context(|| format!("Failed to ensure runtime thread projection row for {thread_id}"))?;

    Ok(())
}
