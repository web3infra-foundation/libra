use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use tokio::{io::AsyncReadExt, sync::Mutex};

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
    pub max_output_bytes: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct SandboxExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Default, Clone)]
struct StreamState {
    bytes: Vec<u8>,
    truncated: bool,
}

const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const TIMEOUT_EXIT_CODE: i32 = 124;
const STREAM_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

pub async fn run_shell_command(
    command: &str,
    cwd: &Path,
    timeout_ms: Option<u64>,
    max_output_bytes: usize,
    sandbox: Option<ToolSandboxContext>,
) -> Result<SandboxExecOutput, String> {
    let (mut cmd, timeout_override) =
        build_sandboxed_shell_command(command, cwd, timeout_ms, sandbox.as_ref())?;
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

    let timeout_dur =
        Duration::from_millis(timeout_override.or(timeout_ms).unwrap_or(DEFAULT_TIMEOUT_MS));
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

fn build_sandboxed_shell_command(
    command: &str,
    cwd: &Path,
    timeout_ms: Option<u64>,
    sandbox: Option<&ToolSandboxContext>,
) -> Result<(tokio::process::Command, Option<u64>), String> {
    let linux_sandbox_exe = std::env::var_os("LIBRA_LINUX_SANDBOX_EXE").map(PathBuf::from);
    let use_linux_sandbox_bwrap = env_flag_enabled("LIBRA_USE_LINUX_SANDBOX_BWRAP");
    let manager = SandboxManager::new();
    let spec = CommandSpec::shell(
        command,
        cwd.to_path_buf(),
        timeout_ms,
        sandbox
            .map(|context| context.permissions)
            .unwrap_or(SandboxPermissions::UseDefault),
        None,
    );
    let exec_env = manager
        .transform(SandboxTransformRequest {
            spec,
            policy: sandbox.map(|context| &context.policy),
            sandbox_policy_cwd: cwd,
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
