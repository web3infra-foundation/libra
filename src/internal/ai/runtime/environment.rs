//! Execution environment boundary for scheduler-owned task attempts.

use std::{
    io,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::internal::ai::{
    orchestrator::{
        types::TaskWorkspaceBackend,
        workspace::{
            FuseAttemptOutcome, FuseProvisionState, SyncBackReport, TaskWorktree,
            WorkspaceSyncError, cleanup_task_worktree, prepare_task_worktree,
            sync_task_worktree_back,
        },
    },
    workspace_snapshot::WorkspaceSnapshot,
};

#[derive(Clone, Debug, Default)]
pub struct ExecutionEnvironmentProvider;

pub struct TaskExecutionEnvironment {
    worktree: TaskWorktree,
    fuse_outcome: FuseAttemptOutcome,
}

impl TaskExecutionEnvironment {
    pub fn root(&self) -> &Path {
        &self.worktree.root
    }

    pub(crate) fn baseline_snapshot(&self) -> WorkspaceSnapshot {
        self.worktree.baseline.clone()
    }

    pub(crate) fn backend(&self) -> TaskWorkspaceBackend {
        self.worktree.backend()
    }

    /// Outcome of the FUSE mount attempt during provisioning. Callers use
    /// `JustDisabled` to emit a one-time TUI hint after the first failure.
    pub fn fuse_outcome(&self) -> &FuseAttemptOutcome {
        &self.fuse_outcome
    }
}

#[derive(Clone, Debug)]
pub struct SyncBackRequest {
    pub main_working_dir: PathBuf,
    pub touch_files: Vec<String>,
    pub scope_in: Vec<String>,
    pub scope_out: Vec<String>,
}

impl ExecutionEnvironmentProvider {
    pub async fn provision_task_worktree(
        &self,
        main_working_dir: PathBuf,
        task_id: Uuid,
        fuse_state: FuseProvisionState,
    ) -> io::Result<TaskExecutionEnvironment> {
        let (worktree, fuse_outcome) = tokio::task::spawn_blocking(move || {
            prepare_task_worktree(&main_working_dir, task_id, &fuse_state)
        })
        .await
        .map_err(|err| io::Error::other(format!("failed to prepare task worktree: {err}")))??;
        Ok(TaskExecutionEnvironment {
            worktree,
            fuse_outcome,
        })
    }

    pub(crate) async fn sync_back(
        &self,
        environment: &TaskExecutionEnvironment,
        request: SyncBackRequest,
    ) -> Result<SyncBackReport, WorkspaceSyncError> {
        let task_worktree_dir = environment.root().to_path_buf();
        let baseline = environment.worktree.baseline.clone();
        tokio::task::spawn_blocking(move || {
            sync_task_worktree_back(
                &request.main_working_dir,
                &task_worktree_dir,
                &baseline,
                &request.touch_files,
                &request.scope_in,
                &request.scope_out,
            )
        })
        .await
        .map_err(|err| WorkspaceSyncError::HardConflict {
            path: None,
            reason: format!("sync worker failed: {err}"),
        })?
    }

    pub async fn cleanup(&self, environment: TaskExecutionEnvironment) -> io::Result<()> {
        tokio::task::spawn_blocking(move || cleanup_task_worktree(environment.worktree))
            .await
            .map_err(|err| io::Error::other(format!("cleanup worker failed: {err}")))?
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn provider_provisions_and_cleans_task_worktree() {
        let main = TempDir::new().unwrap();
        std::fs::write(main.path().join("README.md"), "hello").unwrap();
        let provider = ExecutionEnvironmentProvider;

        let environment = provider
            .provision_task_worktree(
                main.path().to_path_buf(),
                Uuid::new_v4(),
                FuseProvisionState::default(),
            )
            .await
            .expect("provision task worktree");
        assert!(environment.root().join("README.md").exists());
        let root = environment.root().to_path_buf();

        provider
            .cleanup(environment)
            .await
            .expect("cleanup worktree");
        assert!(!root.exists());
    }
}
