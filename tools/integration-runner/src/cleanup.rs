use std::{cell::Cell, process::Command};

/// RAII guard for Wave 3 GitHub temp repo cleanup. On drop (panic, early return, etc) it
/// attempts `gh repo delete <owner/repo> --yes` unless explicitly disarmed after successful
/// explicit delete. Never prints tokens. Matches the "cleanup_guard" requirement.
pub(crate) struct GhRepoCleanupGuard {
    owner_repo: String,
    deleted: Cell<bool>,
}

impl GhRepoCleanupGuard {
    pub(crate) fn new(owner_repo: String) -> Self {
        Self {
            owner_repo,
            deleted: Cell::new(false),
        }
    }

    /// Call after a successful explicit `gh repo delete ... --yes` so Drop does not re-delete.
    pub(crate) fn disarm(&self) {
        self.deleted.set(true);
    }
}

impl Drop for GhRepoCleanupGuard {
    fn drop(&mut self) {
        if !self.deleted.get() {
            // Best-effort cleanup; ignore status (we may be in unwind). Log to stderr for visibility.
            eprintln!(
                "[INTEG-CLEANUP] gh repo delete {} --yes (auto from guard)",
                self.owner_repo
            );
            let _ = Command::new("gh")
                .args(["repo", "delete", &self.owner_repo, "--yes"])
                .status();
            self.deleted.set(true);
        }
    }
}
