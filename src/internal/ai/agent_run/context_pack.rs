//! `AgentContextPack[S]` snapshot — the read-only context bundle handed to a
//! sub-agent at spawn time.
//!
//! Per CEX-S2-01 readiness matrix, the **schema** of this pack depends on
//! Step 1.3 (`list_symbols` / `read_symbol`) and Step 1.9
//! (`ContextFrame` / `MemoryAnchor`). Until those land, this scaffold only
//! holds a minimal placeholder: scope paths the sub-agent may read/write, plus
//! a free-form goal string. The struct is forward-stable; CEX-S2-10 will
//! re-open it once Step 1.3/1.9 ship.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentTaskId, workspace_strategy::check_write_in_scope};

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

impl AgentContextPack {
    /// `write_scope` entries that are **not** covered by `read_scope`,
    /// violating the documented "`write_scope` ⊆ `read_scope`"
    /// invariant (a sub-agent must never be able to write a path it
    /// cannot read).
    ///
    /// Coverage uses the same lexical, component-wise containment as
    /// [`check_write_in_scope`] — a write entry is covered when it is
    /// equal to or nested under some `read_scope` entry. Absolute or
    /// `..`-escaping entries are never covered (they aren't valid
    /// repo-relative scope paths), so a malformed write entry surfaces
    /// here too.
    ///
    /// An empty vec means the invariant holds. Returned in `write_scope`
    /// order so callers can report each offending entry.
    pub fn write_paths_outside_read_scope(&self) -> Vec<&str> {
        self.write_scope
            .iter()
            .filter(|entry| check_write_in_scope(entry, &self.read_scope).is_err())
            .map(String::as_str)
            .collect()
    }

    /// `true` when every `write_scope` entry is covered by `read_scope`
    /// (the "writable ⊆ readable" invariant holds). Convenience wrapper
    /// over [`write_paths_outside_read_scope`](Self::write_paths_outside_read_scope).
    pub fn write_scope_within_read_scope(&self) -> bool {
        self.write_paths_outside_read_scope().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(read: &[&str], write: &[&str]) -> AgentContextPack {
        AgentContextPack {
            task_id: AgentTaskId::new(),
            goal: "test".to_string(),
            read_scope: read.iter().map(|s| s.to_string()).collect(),
            write_scope: write.iter().map(|s| s.to_string()).collect(),
            source_intent_id: None,
        }
    }

    /// CEX-S2-10 freezes the `AgentContextPack` wire contract
    /// (`#[serde(deny_unknown_fields)]`). The tests below cover scope
    /// LOGIC; this one pins the SERDE contract: the EXACT key set
    /// (`read_scope` / `write_scope` carry `#[serde(default)]` but NO
    /// skip, so they always serialize — even empty as `[]`), the
    /// `source_intent_id` skip-when-None / present-when-Some, the
    /// `serde(default)` scope behaviour on read, the `deny_unknown_fields`
    /// rejection, and the round-trip.
    #[test]
    fn agent_context_pack_wire_contract_is_frozen() {
        // Minimal pack: empty scopes still serialize as `[]`; intent omitted.
        let minimal = pack(&[], &[]);
        let json = serde_json::to_value(&minimal).expect("serialize AgentContextPack");
        let keys: std::collections::BTreeSet<&str> = json
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        let base: std::collections::BTreeSet<&str> =
            ["task_id", "goal", "read_scope", "write_scope"]
                .into_iter()
                .collect();
        assert_eq!(
            keys, base,
            "AgentContextPack (None intent) must serialize EXACTLY the frozen key set \
             (scopes have no skip_serializing_if), got: {json}",
        );
        assert_eq!(
            json["read_scope"],
            serde_json::json!([]),
            "empty read_scope must serialize as [], not be omitted",
        );
        assert_eq!(json["write_scope"], serde_json::json!([]));

        // Populated scopes + intent present.
        let full = AgentContextPack {
            source_intent_id: Some(Uuid::new_v4()),
            ..pack(&["src"], &["src/foo.rs"])
        };
        let full_json = serde_json::to_value(&full).expect("serialize AgentContextPack");
        let full_keys: std::collections::BTreeSet<&str> = full_json
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut full_expected = base.clone();
        full_expected.insert("source_intent_id");
        assert_eq!(
            full_keys, full_expected,
            "populated AgentContextPack must add EXACTLY source_intent_id, got: {full_json}",
        );

        // `#[serde(default)]` scopes: omitted on read default to empty.
        let mut without_scopes = json.as_object().expect("object").clone();
        without_scopes.remove("read_scope");
        without_scopes.remove("write_scope");
        let parsed: AgentContextPack =
            serde_json::from_value(serde_json::Value::Object(without_scopes))
                .expect("deserialize without scopes");
        assert!(
            parsed.read_scope.is_empty() && parsed.write_scope.is_empty(),
            "omitted #[serde(default)] scopes must default to empty",
        );

        // deny_unknown_fields: an unknown field is rejected on read.
        let mut with_extra = full_json.as_object().expect("object").clone();
        with_extra.insert("bogus".to_string(), serde_json::Value::Bool(true));
        assert!(
            serde_json::from_value::<AgentContextPack>(serde_json::Value::Object(with_extra))
                .is_err(),
            "deny_unknown_fields must reject an unknown field",
        );

        // Round-trip: the wire shape deserializes and re-serializes intact.
        let back: AgentContextPack =
            serde_json::from_value(full_json.clone()).expect("deserialize AgentContextPack");
        assert_eq!(
            serde_json::to_value(&back).expect("re-serialize"),
            full_json,
            "AgentContextPack must round-trip its wire shape",
        );
    }

    /// Write entries equal to or nested under a read entry satisfy the
    /// subset invariant.
    #[test]
    fn write_scope_within_read_scope_when_nested() {
        let p = pack(&["src", "docs"], &["src", "src/foo.rs", "docs/api"]);
        assert!(p.write_scope_within_read_scope());
        assert!(p.write_paths_outside_read_scope().is_empty());
    }

    /// A read scope of `.` (whole repo) covers any write entry.
    #[test]
    fn root_read_scope_covers_all_writes() {
        let p = pack(&["."], &["anything/at/all.rs", "Cargo.toml"]);
        assert!(p.write_scope_within_read_scope());
    }

    /// A write entry not covered by any read entry is reported as a
    /// violation (in `write_scope` order).
    #[test]
    fn write_outside_read_scope_is_reported() {
        let p = pack(&["src"], &["src/ok.rs", "lib/bad.rs", "tests/also_bad.rs"]);
        assert!(!p.write_scope_within_read_scope());
        assert_eq!(
            p.write_paths_outside_read_scope(),
            vec!["lib/bad.rs", "tests/also_bad.rs"],
        );
    }

    /// An empty `read_scope` covers nothing, so any non-empty
    /// `write_scope` violates the invariant (fail-closed).
    #[test]
    fn empty_read_scope_rejects_all_writes() {
        let p = pack(&[], &["src/foo.rs"]);
        assert!(!p.write_scope_within_read_scope());
        assert_eq!(p.write_paths_outside_read_scope(), vec!["src/foo.rs"]);
    }

    /// Absolute or `..`-escaping write entries are never covered — they
    /// aren't valid repo-relative paths — so they surface as violations
    /// even under a permissive `.` read scope.
    #[test]
    fn malformed_write_entries_are_violations() {
        let p = pack(&["."], &["/etc/passwd", "../outside"]);
        assert_eq!(
            p.write_paths_outside_read_scope(),
            vec!["/etc/passwd", "../outside"],
        );
    }

    /// No write scope at all trivially satisfies the invariant.
    #[test]
    fn empty_write_scope_is_within_any_read_scope() {
        assert!(pack(&["src"], &[]).write_scope_within_read_scope());
        assert!(pack(&[], &[]).write_scope_within_read_scope());
    }
}
