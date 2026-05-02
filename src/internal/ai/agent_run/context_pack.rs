//! `AgentContextPack[S]` snapshot — the read-only context bundle handed to a
//! sub-agent at spawn time.
//!
//! Per CEX-S2-01 readiness matrix, the **schema** of this pack depends on
//! Step 1.3 (`list_symbols` / `read_symbol`) and Step 1.9
//! (`ContextFrame` / `MemoryAnchor`). Until those land, this scaffold only
//! holds a minimal placeholder: scope paths the sub-agent may read/write, plus
//! a free-form goal string. The struct is forward-stable; CEX-S2-10 will
//! re-open it once Step 1.3/1.9 ship.

#![cfg(feature = "subagent-scaffold")]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AgentTaskId;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContextPack {
    pub task_id: AgentTaskId,

    /// Goal description in natural language. Layer 1 derives this from the
    /// confirmed `Task` acceptance summary.
    pub goal: String,

    /// Filesystem scope (relative paths inside the source repo) the sub-agent
    /// may read. Drives sparse-checkout selection in CEX-S2-11.
    #[serde(default)]
    pub read_scope: Vec<String>,

    /// Filesystem scope the sub-agent may write to (subset of `read_scope`).
    #[serde(default)]
    pub write_scope: Vec<String>,

    /// `IntentSpec` id, if applicable, so the sub-agent can pull additional
    /// context from the persistent intent without re-asking Layer 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_intent_id: Option<Uuid>,
}
