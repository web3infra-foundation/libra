use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, mpsc::UnboundedSender, oneshot},
};

pub mod policy;
pub mod runtime;

pub use policy::{NetworkAccess, SandboxPermissions, SandboxPolicy, WritableRoot};
pub use runtime::{
    CommandSpec, ExecEnv, SandboxManager, SandboxTransformError, SandboxTransformRequest,
    SandboxType,
};

/// Runtime sandbox configuration attached to a tool invocation.
#[derive(Clone, Debug)]
pub struct ToolSandboxContext {
    pub policy: SandboxPolicy,
    pub permissions: SandboxPermissions,
}

#[derive(Clone, Debug, Default)]
pub struct ToolRuntimeContext {
    pub sandbox: Option<ToolSandboxContext>,
    pub sandbox_runtime: Option<SandboxRuntimeConfig>,
    pub approval: Option<ToolApprovalContext>,
    pub max_output_bytes: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct SandboxRuntimeConfig {
    pub linux_sandbox_exe: Option<PathBuf>,
    pub use_linux_sandbox_bwrap: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AskForApproval {
    Never,
    OnFailure,
    #[default]
    OnRequest,
    UnlessTrusted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ApprovedForSession,
    #[default]
    Denied,
    Abort,
}

#[derive(Debug, Default)]
pub struct ApprovalStore {
    map: HashMap<String, ReviewDecision>,
}

impl ApprovalStore {
    pub fn get(&self, key: &str) -> Option<ReviewDecision> {
        self.map.get(key).copied()
    }

    pub fn put(&mut self, key: String, value: ReviewDecision) {
        self.map.insert(key, value);
    }
}

#[derive(Clone, Debug)]
pub struct ToolApprovalContext {
    pub policy: AskForApproval,
    pub request_tx: UnboundedSender<ExecApprovalRequest>,
    pub store: Arc<Mutex<ApprovalStore>>,
}

pub struct ExecApprovalRequest {
    pub call_id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub reason: Option<String>,
    pub is_retry: bool,
    pub response_tx: oneshot::Sender<ReviewDecision>,
}

impl std::fmt::Debug for ExecApprovalRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecApprovalRequest")
            .field("call_id", &self.call_id)
            .field("command", &self.command)
            .field("cwd", &self.cwd)
            .field("reason", &self.reason)
            .field("is_retry", &self.is_retry)
            .field("response_tx", &"<oneshot::Sender>")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct SandboxExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Clone, Debug)]
pub struct ShellCommandRequest {
    pub call_id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: usize,
    pub sandbox: Option<ToolSandboxContext>,
    pub sandbox_runtime: Option<SandboxRuntimeConfig>,
    pub approval: Option<ToolApprovalContext>,
    pub justification: Option<String>,
}

#[derive(Default, Clone)]
struct StreamState {
    bytes: Vec<u8>,
    truncated: bool,
}

const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const TIMEOUT_EXIT_CODE: i32 = 124;
const STREAM_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
const SANDBOX_DENIED_KEYWORDS: [&str; 7] = [
    "operation not permitted",
    "permission denied",
    "read-only file system",
    "seccomp",
    "sandbox",
    "landlock",
    "failed to write file",
];
const QUICK_REJECT_EXIT_CODES: [i32; 3] = [2, 126, 127];

pub async fn run_shell_command(
    command: &str,
    cwd: &Path,
    timeout_ms: Option<u64>,
    max_output_bytes: usize,
    sandbox: Option<ToolSandboxContext>,
    sandbox_runtime: Option<&SandboxRuntimeConfig>,
) -> Result<SandboxExecOutput, String> {
    let spec = CommandSpec::shell(
        command,
        cwd.to_path_buf(),
        timeout_ms,
        sandbox
            .as_ref()
            .map(|context| context.permissions)
            .unwrap_or(SandboxPermissions::UseDefault),
        None,
    );
    run_command_spec(spec, max_output_bytes, sandbox, sandbox_runtime).await
}

pub async fn run_shell_command_with_approval(
    request: ShellCommandRequest,
) -> Result<SandboxExecOutput, String> {
    let ShellCommandRequest {
        call_id,
        command,
        cwd,
        timeout_ms,
        max_output_bytes,
        sandbox,
        sandbox_runtime,
        approval,
        justification,
    } = request;

    let spec = CommandSpec::shell(
        &command,
        cwd.clone(),
        timeout_ms,
        sandbox
            .as_ref()
            .map(|context| context.permissions)
            .unwrap_or(SandboxPermissions::UseDefault),
        justification.clone(),
    );

    let requirement = approval
        .as_ref()
        .map(|ctx| {
            default_exec_approval_requirement(ctx.policy, sandbox.as_ref().map(|s| &s.policy))
        })
        .unwrap_or(ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
        });

    let mut already_approved = false;
    if let Some(approval_ctx) = approval.as_ref() {
        match requirement {
            ExecApprovalRequirement::Skip { .. } => {}
            ExecApprovalRequirement::NeedsApproval { ref reason } => {
                let decision = request_exec_approval(
                    approval_ctx,
                    &call_id,
                    &command,
                    &cwd,
                    reason.clone().or_else(|| {
                        justification
                            .as_deref()
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                            .map(ToString::to_string)
                    }),
                    spec.sandbox_permissions,
                    false,
                )
                .await;

                match decision {
                    ReviewDecision::Approved | ReviewDecision::ApprovedForSession => {
                        already_approved = true;
                    }
                    ReviewDecision::Denied => return Err("rejected by user".to_string()),
                    ReviewDecision::Abort => return Err("aborted by user".to_string()),
                }
            }
        }
    }

    let first_attempt_is_sandboxed = sandbox.is_some()
        && !spec.sandbox_permissions.requires_escalated_permissions()
        && !matches!(
            requirement,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: true
            }
        );
    let first_attempt_sandbox = if first_attempt_is_sandboxed {
        sandbox.clone()
    } else {
        None
    };

    let first_output = run_command_spec(
        spec.clone(),
        max_output_bytes,
        first_attempt_sandbox,
        sandbox_runtime.as_ref(),
    )
    .await?;

    if !first_attempt_is_sandboxed || !is_likely_sandbox_denied(&first_output) {
        return Ok(first_output);
    }

    let Some(approval_ctx) = approval.as_ref() else {
        return Ok(first_output);
    };
    if !wants_no_sandbox_approval(approval_ctx.policy) {
        return Ok(first_output);
    }

    if !should_bypass_approval(approval_ctx.policy, already_approved) {
        let decision = request_exec_approval(
            approval_ctx,
            &call_id,
            &command,
            &cwd,
            Some(build_denial_reason_from_output(&first_output)),
            spec.sandbox_permissions,
            true,
        )
        .await;

        match decision {
            ReviewDecision::Approved | ReviewDecision::ApprovedForSession => {}
            ReviewDecision::Denied => return Err("rejected by user".to_string()),
            ReviewDecision::Abort => return Err("aborted by user".to_string()),
        }
    }

    run_command_spec(spec, max_output_bytes, None, sandbox_runtime.as_ref()).await
}

pub async fn run_command_spec(
    spec: CommandSpec,
    max_output_bytes: usize,
    sandbox: Option<ToolSandboxContext>,
    sandbox_runtime: Option<&SandboxRuntimeConfig>,
) -> Result<SandboxExecOutput, String> {
    let (mut cmd, timeout_override) =
        build_command_from_spec(spec, sandbox.as_ref(), sandbox_runtime)?;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn shell: {e}"))?;

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;

    let stdout_state = Arc::new(Mutex::new(StreamState::default()));
    let stderr_state = Arc::new(Mutex::new(StreamState::default()));
    let stdout_task = tokio::spawn(drain_reader(
        stdout_pipe,
        max_output_bytes,
        Arc::clone(&stdout_state),
    ));
    let stderr_task = tokio::spawn(drain_reader(
        stderr_pipe,
        max_output_bytes,
        Arc::clone(&stderr_state),
    ));

    let timeout_dur = Duration::from_millis(timeout_override.unwrap_or(DEFAULT_TIMEOUT_MS));
    let (exit_code, timed_out) = tokio::select! {
        status = child.wait() => {
            let code = status
                .map_err(|e| format!("wait failed: {e}"))?
                .code()
                .unwrap_or(-1);
            (code, false)
        }
        _ = tokio::time::sleep(timeout_dur) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (TIMEOUT_EXIT_CODE, true)
        }
    };

    let (mut stdout, stdout_truncated, stdout_incomplete) =
        collect_stream(stdout_task, stdout_state).await;
    let (mut stderr, stderr_truncated, stderr_incomplete) =
        collect_stream(stderr_task, stderr_state).await;

    if stdout_truncated {
        stdout.push_str("\n[stdout truncated]");
    }
    if stderr_truncated {
        stderr.push_str("\n[stderr truncated]");
    }
    if stdout_incomplete {
        stdout.push_str("\n[stdout stream incomplete]");
    }
    if stderr_incomplete {
        stderr.push_str("\n[stderr stream incomplete]");
    }

    Ok(SandboxExecOutput {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

fn build_command_from_spec(
    spec: CommandSpec,
    sandbox: Option<&ToolSandboxContext>,
    sandbox_runtime: Option<&SandboxRuntimeConfig>,
) -> Result<(tokio::process::Command, Option<u64>), String> {
    let sandbox_policy_cwd = spec.cwd.clone();
    let linux_sandbox_exe = sandbox_runtime
        .and_then(|config| config.linux_sandbox_exe.clone())
        .or_else(|| std::env::var_os("LIBRA_LINUX_SANDBOX_EXE").map(PathBuf::from));
    let use_linux_sandbox_bwrap = sandbox_runtime
        .map(|config| config.use_linux_sandbox_bwrap)
        .unwrap_or_else(|| env_flag_enabled("LIBRA_USE_LINUX_SANDBOX_BWRAP"));
    let manager = SandboxManager::new();
    let exec_env = manager
        .transform(SandboxTransformRequest {
            spec,
            policy: sandbox.map(|context| &context.policy),
            sandbox_policy_cwd: &sandbox_policy_cwd,
            linux_sandbox_exe: linux_sandbox_exe.as_ref(),
            use_linux_sandbox_bwrap,
        })
        .map_err(|err| err.to_string())?;
    exec_env.into_command()
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        matches!(value.as_str(), "1" | "true" | "yes" | "on")
    })
}

async fn request_exec_approval(
    ctx: &ToolApprovalContext,
    call_id: &str,
    command: &str,
    cwd: &Path,
    reason: Option<String>,
    sandbox_permissions: SandboxPermissions,
    is_retry: bool,
) -> ReviewDecision {
    let cache_key = shell_approval_key(command, cwd, sandbox_permissions);
    let already_approved = {
        let store = ctx.store.lock().await;
        matches!(
            store.get(&cache_key),
            Some(ReviewDecision::ApprovedForSession)
        )
    };
    if already_approved {
        return ReviewDecision::ApprovedForSession;
    }

    let (response_tx, response_rx) = oneshot::channel();
    let request = ExecApprovalRequest {
        call_id: call_id.to_string(),
        command: command.to_string(),
        cwd: cwd.to_path_buf(),
        reason,
        is_retry,
        response_tx,
    };

    if ctx.request_tx.send(request).is_err() {
        return ReviewDecision::Denied;
    }

    let decision = response_rx.await.unwrap_or_default();
    if matches!(decision, ReviewDecision::ApprovedForSession) {
        let mut store = ctx.store.lock().await;
        store.put(cache_key, ReviewDecision::ApprovedForSession);
    }
    decision
}

fn shell_approval_key(command: &str, cwd: &Path, sandbox_permissions: SandboxPermissions) -> String {
    format!(
        "{}|{}|{}",
        command,
        cwd.display(),
        match sandbox_permissions {
            SandboxPermissions::UseDefault => "use_default",
            SandboxPermissions::RequireEscalated => "require_escalated",
        }
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExecApprovalRequirement {
    Skip { bypass_sandbox: bool },
    NeedsApproval { reason: Option<String> },
}

fn default_exec_approval_requirement(
    policy: AskForApproval,
    sandbox_policy: Option<&SandboxPolicy>,
) -> ExecApprovalRequirement {
    let needs_approval = match policy {
        AskForApproval::Never | AskForApproval::OnFailure => false,
        AskForApproval::OnRequest => sandbox_policy.is_some_and(|policy| {
            !matches!(
                policy,
                SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
            )
        }),
        AskForApproval::UnlessTrusted => true,
    };

    if needs_approval {
        ExecApprovalRequirement::NeedsApproval { reason: None }
    } else {
        ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
        }
    }
}

fn should_bypass_approval(policy: AskForApproval, already_approved: bool) -> bool {
    if already_approved {
        return true;
    }
    matches!(policy, AskForApproval::Never)
}

fn wants_no_sandbox_approval(policy: AskForApproval) -> bool {
    !matches!(policy, AskForApproval::Never | AskForApproval::OnRequest)
}

fn build_denial_reason_from_output(_output: &SandboxExecOutput) -> String {
    "command failed; retry without sandbox?".to_string()
}

fn is_likely_sandbox_denied(output: &SandboxExecOutput) -> bool {
    if output.exit_code == 0 || output.timed_out {
        return false;
    }

    let has_sandbox_keyword = [&output.stderr, &output.stdout].into_iter().any(|section| {
        let lower = section.to_ascii_lowercase();
        SANDBOX_DENIED_KEYWORDS
            .iter()
            .any(|needle| lower.contains(needle))
    });
    if has_sandbox_keyword {
        return true;
    }

    !QUICK_REJECT_EXIT_CODES.contains(&output.exit_code)
}

async fn drain_reader(
    mut reader: impl AsyncReadExt + Unpin,
    max_bytes: usize,
    state: Arc<Mutex<StreamState>>,
) {
    let mut tmp = [0u8; 8192];
    loop {
        match reader.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut guard = state.lock().await;
                append_chunk(&mut guard, &tmp[..n], max_bytes);
            }
        }
    }
}

fn append_chunk(state: &mut StreamState, chunk: &[u8], max_bytes: usize) {
    let remaining = max_bytes.saturating_sub(state.bytes.len());
    let to_take = remaining.min(chunk.len());
    if to_take > 0 {
        state.bytes.extend_from_slice(&chunk[..to_take]);
    }
    if to_take < chunk.len() {
        state.truncated = true;
    }
}

async fn collect_stream(
    mut task: tokio::task::JoinHandle<()>,
    state: Arc<Mutex<StreamState>>,
) -> (String, bool, bool) {
    let completed = tokio::time::timeout(STREAM_DRAIN_TIMEOUT, &mut task)
        .await
        .is_ok();
    if !completed {
        task.abort();
        let _ = task.await;
    }

    let snapshot = state.lock().await.clone();
    (
        String::from_utf8_lossy(&snapshot.bytes).into_owned(),
        snapshot.truncated,
        !completed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_request_requires_approval_in_workspace_write() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let requirement = default_exec_approval_requirement(AskForApproval::OnRequest, Some(&policy));
        assert!(matches!(requirement, ExecApprovalRequirement::NeedsApproval { .. }));
    }

    #[test]
    fn on_request_skips_approval_in_danger_full_access() {
        let requirement = default_exec_approval_requirement(
            AskForApproval::OnRequest,
            Some(&SandboxPolicy::DangerFullAccess),
        );
        assert!(matches!(
            requirement,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: false
            }
        ));
    }

    #[test]
    fn sandbox_denied_keywords_trigger_detection() {
        let output = SandboxExecOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "operation not permitted".to_string(),
            timed_out: false,
        };
        assert!(is_likely_sandbox_denied(&output));
    }
}
