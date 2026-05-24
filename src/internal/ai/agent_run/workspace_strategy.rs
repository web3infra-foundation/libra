//! Workspace strategy selection for sub-agent isolated workspaces
//! (CEX-S2-11).
//!
//! This module owns the **pure policy** that picks which materialization
//! strategy a sub-agent should use for its isolated workspace, given the
//! source repository's size. It deliberately carries no I/O: the actual
//! materialization (Libra/Git worktree reservation, sparse checkout, or
//! full copy) lands in a later CEX-S2-11 slice that wires
//! `orchestrator/workspace.rs` into the sub-agent dispatcher.
//!
//! The thresholds come from `docs/improvement/agent.md` (Step 2 workspace
//! materialization table):
//!
//! | condition                                   | strategy   |
//! |---------------------------------------------|------------|
//! | `.git` < 1 GiB **and** files < 100K         | `Worktree` |
//! | files ≥ 100K **or** `.git` ≥ 1 GiB          | `Sparse`   |
//! | preferred strategy unavailable **and** user | `FullCopy` |
//! | set `agent.allow_full_copy = true`          |            |
//!
//! [`WorkspaceStrategy::Blocked`] is not produced here — it is a *runtime*
//! decision raised when a sub-agent write escapes the materialized scope,
//! not a selection-time outcome.

use super::event::WorkspaceStrategy;

/// `.git` size (bytes) at or above which sparse materialization is
/// preferred over a full worktree. 1 GiB, per the agent.md workspace
/// materialization table.
pub const SPARSE_REPO_SIZE_THRESHOLD_BYTES: u64 = 1 << 30;

/// Worktree file count at or above which sparse materialization is
/// preferred. 100K files, per the agent.md workspace materialization
/// table.
pub const SPARSE_FILE_COUNT_THRESHOLD: u64 = 100_000;

/// Source-repository measurements used to pick the preferred workspace
/// strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceSizing {
    /// Total `.git` directory size in bytes.
    pub repo_size_bytes: u64,
    /// Number of files in the source worktree.
    pub worktree_file_count: u64,
}

impl WorkspaceSizing {
    /// `true` when either dimension reaches its sparse threshold — i.e.
    /// `.git` ≥ 1 GiB OR file count ≥ 100K. Sparse materialization is
    /// preferred in this case so a sub-agent never has to copy a huge
    /// history or file tree.
    pub fn requires_sparse(&self) -> bool {
        self.repo_size_bytes >= SPARSE_REPO_SIZE_THRESHOLD_BYTES
            || self.worktree_file_count >= SPARSE_FILE_COUNT_THRESHOLD
    }
}

/// Pick the preferred workspace strategy from repository sizing alone.
///
/// Returns [`WorkspaceStrategy::Sparse`] when either dimension reaches
/// its threshold (`.git` ≥ [`SPARSE_REPO_SIZE_THRESHOLD_BYTES`] OR file
/// count ≥ [`SPARSE_FILE_COUNT_THRESHOLD`]); otherwise
/// [`WorkspaceStrategy::Worktree`].
///
/// Never returns [`WorkspaceStrategy::FullCopy`] (an explicit opt-in
/// fallback — see [`resolve_full_copy_fallback`]) or
/// [`WorkspaceStrategy::Blocked`] (a runtime scope-violation outcome).
pub fn select_preferred_strategy(sizing: WorkspaceSizing) -> WorkspaceStrategy {
    if sizing.requires_sparse() {
        WorkspaceStrategy::Sparse
    } else {
        WorkspaceStrategy::Worktree
    }
}

/// Resolve the fallback strategy when the preferred strategy
/// ([`WorkspaceStrategy::Worktree`] / [`WorkspaceStrategy::Sparse`])
/// could not be materialized.
///
/// Per CEX-S2-11 (2), full copy is only permitted when the user has
/// explicitly opted in via `agent.allow_full_copy = true`, and callers
/// MUST log a warning when this returns `Some(FullCopy)` (full copy is
/// for debug / small fixtures / emergency compatibility only).
///
/// Returns `None` when full copy is not permitted, signalling that the
/// caller should surface the underlying materialization error instead
/// of silently copying the whole repository.
pub fn resolve_full_copy_fallback(allow_full_copy: bool) -> Option<WorkspaceStrategy> {
    allow_full_copy.then_some(WorkspaceStrategy::FullCopy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both dimensions below their thresholds → `Worktree` (the default
    /// reuse-the-object-store strategy).
    #[test]
    fn select_prefers_worktree_below_both_thresholds() {
        let sizing = WorkspaceSizing {
            repo_size_bytes: SPARSE_REPO_SIZE_THRESHOLD_BYTES - 1,
            worktree_file_count: SPARSE_FILE_COUNT_THRESHOLD - 1,
        };
        assert!(!sizing.requires_sparse());
        assert_eq!(
            select_preferred_strategy(sizing),
            WorkspaceStrategy::Worktree
        );
    }

    /// A tiny repo (the common case) → `Worktree`.
    #[test]
    fn select_prefers_worktree_for_small_repo() {
        let sizing = WorkspaceSizing {
            repo_size_bytes: 4 * 1024 * 1024, // 4 MiB
            worktree_file_count: 1_200,
        };
        assert_eq!(
            select_preferred_strategy(sizing),
            WorkspaceStrategy::Worktree
        );
    }

    /// `.git` size at exactly the 1 GiB threshold → `Sparse` (the
    /// boundary is inclusive: `>=`). Pins the `≥ 1 GiB` half of the
    /// agent.md rule so an off-by-one refactor to `>` trips here.
    #[test]
    fn select_switches_to_sparse_at_repo_size_threshold() {
        let sizing = WorkspaceSizing {
            repo_size_bytes: SPARSE_REPO_SIZE_THRESHOLD_BYTES,
            worktree_file_count: 10,
        };
        assert!(sizing.requires_sparse());
        assert_eq!(select_preferred_strategy(sizing), WorkspaceStrategy::Sparse);
    }

    /// File count at exactly the 100K threshold → `Sparse` (inclusive
    /// boundary). Pins the `≥ 100K` half of the rule.
    #[test]
    fn select_switches_to_sparse_at_file_count_threshold() {
        let sizing = WorkspaceSizing {
            repo_size_bytes: 1024,
            worktree_file_count: SPARSE_FILE_COUNT_THRESHOLD,
        };
        assert!(sizing.requires_sparse());
        assert_eq!(select_preferred_strategy(sizing), WorkspaceStrategy::Sparse);
    }

    /// Either dimension over its threshold independently forces
    /// `Sparse` — the rule is an OR, not an AND. Covers both the
    /// "huge history, few files" and "many files, small history"
    /// shapes.
    #[test]
    fn select_uses_sparse_when_either_dimension_exceeds_threshold() {
        let big_history = WorkspaceSizing {
            repo_size_bytes: 8 * SPARSE_REPO_SIZE_THRESHOLD_BYTES,
            worktree_file_count: 50,
        };
        assert_eq!(
            select_preferred_strategy(big_history),
            WorkspaceStrategy::Sparse
        );

        let many_files = WorkspaceSizing {
            repo_size_bytes: 16 * 1024 * 1024,
            worktree_file_count: 2 * SPARSE_FILE_COUNT_THRESHOLD,
        };
        assert_eq!(
            select_preferred_strategy(many_files),
            WorkspaceStrategy::Sparse
        );
    }

    /// Full copy is gated on the explicit opt-in. `true` →
    /// `Some(FullCopy)`; `false` → `None` (caller must surface the real
    /// materialization error rather than silently full-copying).
    #[test]
    fn full_copy_fallback_requires_explicit_opt_in() {
        assert_eq!(
            resolve_full_copy_fallback(true),
            Some(WorkspaceStrategy::FullCopy)
        );
        assert_eq!(resolve_full_copy_fallback(false), None);
    }

    /// The selection function never emits `FullCopy` or `Blocked` —
    /// those are an opt-in fallback and a runtime scope-violation
    /// outcome respectively, not size-driven choices. Sweep a range of
    /// sizings and assert the output is always one of the two
    /// size-selected variants.
    #[test]
    fn select_never_emits_full_copy_or_blocked() {
        for repo_size_bytes in [
            0,
            1024,
            SPARSE_REPO_SIZE_THRESHOLD_BYTES - 1,
            SPARSE_REPO_SIZE_THRESHOLD_BYTES,
            64 * SPARSE_REPO_SIZE_THRESHOLD_BYTES,
        ] {
            for worktree_file_count in [
                0,
                10,
                SPARSE_FILE_COUNT_THRESHOLD - 1,
                SPARSE_FILE_COUNT_THRESHOLD,
                5 * SPARSE_FILE_COUNT_THRESHOLD,
            ] {
                let strategy = select_preferred_strategy(WorkspaceSizing {
                    repo_size_bytes,
                    worktree_file_count,
                });
                assert!(
                    matches!(
                        strategy,
                        WorkspaceStrategy::Worktree | WorkspaceStrategy::Sparse
                    ),
                    "select_preferred_strategy returned {strategy:?} for \
                     repo_size_bytes={repo_size_bytes}, \
                     worktree_file_count={worktree_file_count}",
                );
            }
        }
    }
}
