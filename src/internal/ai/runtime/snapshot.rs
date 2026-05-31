//! Top-level `Snapshot` trait — CEX-00.5 deliverable.
//!
//! Every immutable Snapshot type the runtime stores (Plan, Task, Run,
//! IntentSpec, AgentTask, AgentRun, …) should implement this trait so that
//! projection rebuild and observability code can carry `&dyn Snapshot`
//! references without specializing on each concrete type.
//!
//! # Compatibility
//!
//! `MaterializedProjection` (in `runtime/contracts.rs`) implements this trait
//! so existing projection callers can treat the projection as a Snapshot for
//! audit / observability purposes without API churn. Future Snapshot types
//! (Step 2 `AgentTask`, `AgentRun`, `MergeCandidate`) plug in the same way.

use uuid::Uuid;

/// Marker + metadata trait for immutable snapshot types persisted by the
/// agent runtime.
///
/// The trait is intentionally minimal — it only exposes the discriminator
/// and id that every persistent snapshot needs. Concrete snapshot business
/// fields stay on the implementing type.
pub trait Snapshot: Send + Sync {
    /// Stable kind discriminator in `snake_case`. **Must not change** once a
    /// snapshot has shipped.
    fn snapshot_kind(&self) -> &'static str;

    /// Stable id for this snapshot occurrence.
    fn snapshot_id(&self) -> Uuid;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummySnap {
        id: Uuid,
    }

    impl Snapshot for DummySnap {
        fn snapshot_kind(&self) -> &'static str {
            "dummy"
        }

        fn snapshot_id(&self) -> Uuid {
            self.id
        }
    }

    #[test]
    fn snapshot_trait_is_dyn_compatible() {
        let s = DummySnap { id: Uuid::nil() };
        let dyn_ref: &dyn Snapshot = &s;
        assert_eq!(dyn_ref.snapshot_kind(), "dummy");
        assert_eq!(dyn_ref.snapshot_id(), Uuid::nil());
    }
}
