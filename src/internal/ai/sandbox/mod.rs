use std::{path::Path, process::Stdio, sync::Arc, time::Duration};

use tokio::{io::AsyncReadExt, process::Command, sync::Mutex};

pub mod policy;

pub use policy::{NetworkAccess, SandboxPermissions, SandboxPolicy, WritableRoot};

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
    let mut cmd = build_sandboxed_shell_command(command, cwd, sandbox.as_ref())?;
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

    let timeout_dur = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
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
    sandbox: Option<&ToolSandboxContext>,
) -> Result<Command, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let inner_command = vec![shell.clone(), "-c".to_string(), command.to_string()];

    let mut cmd = match sandbox {
        Some(context) if !context.permissions.requires_escalated_permissions() => {
            build_policy_command(inner_command, cwd, &context.policy)?
        }
        _ => {
            let mut cmd = Command::new(&shell);
            cmd.arg("-c").arg(command);
            cmd
        }
    };

    cmd.current_dir(cwd);
    Ok(cmd)
}

fn build_policy_command(
    command: Vec<String>,
    cwd: &Path,
    policy: &SandboxPolicy,
) -> Result<Command, String> {
    if policy.has_full_disk_write_access() && policy.has_full_network_access() {
        let mut cmd = Command::new(command.first().ok_or_else(|| "empty command".to_string())?);
        cmd.args(command.iter().skip(1));
        return Ok(cmd);
    }

    #[cfg(target_os = "macos")]
    {
        return build_macos_seatbelt_command(command, cwd, policy);
    }

    #[cfg(target_os = "linux")]
    {
        return build_linux_bwrap_command(command, cwd, policy);
    }

    #[allow(unreachable_code)]
    Err("sandboxed command execution is not supported on this platform".to_string())
}

#[cfg(target_os = "macos")]
fn build_macos_seatbelt_command(
    command: Vec<String>,
    cwd: &Path,
    policy: &SandboxPolicy,
) -> Result<Command, String> {
    const SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

    let profile = build_seatbelt_profile(policy, cwd)?;
    let mut cmd = Command::new(SEATBELT_EXECUTABLE);
    cmd.arg("-p").arg(profile).arg("--");
    cmd.args(command);
    Ok(cmd)
}

#[cfg(target_os = "macos")]
fn build_seatbelt_profile(policy: &SandboxPolicy, cwd: &Path) -> Result<String, String> {
    let mut lines = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        "(import \"system.sb\")".to_string(),
        "(allow process*)".to_string(),
        "(allow file-read*)".to_string(),
        "(allow file-write* (literal \"/dev/null\"))".to_string(),
    ];

    let writable_roots = policy.get_writable_roots_with_cwd(cwd);
    if policy.has_full_disk_write_access() {
        lines.push("(allow file-write* (regex #\"^/\"))".to_string());
    } else {
        for root in &writable_roots {
            let root = root
                .root
                .canonicalize()
                .unwrap_or_else(|_| root.root.clone())
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            lines.push(format!("(allow file-write* (subpath \"{root}\"))"));
        }
        for subpath in writable_roots
            .iter()
            .flat_map(|root| &root.read_only_subpaths)
        {
            if !subpath.exists() {
                continue;
            }
            let escaped = subpath
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            lines.push(format!("(deny file-write* (subpath \"{escaped}\"))"));
        }
    }

    if policy.has_full_network_access() {
        lines.push("(allow network*)".to_string());
    }

    Ok(lines.join("\n"))
}

#[cfg(target_os = "linux")]
fn build_linux_bwrap_command(
    command: Vec<String>,
    cwd: &Path,
    policy: &SandboxPolicy,
) -> Result<Command, String> {
    let mut args = Vec::<String>::new();
    args.push("--new-session".to_string());
    args.push("--die-with-parent".to_string());

    if !policy.has_full_disk_write_access() {
        args.push("--ro-bind".to_string());
        args.push("/".to_string());
        args.push("/".to_string());

        let writable_roots = policy.get_writable_roots_with_cwd(cwd);
        for root in &writable_roots {
            if !root.root.exists() {
                continue;
            }
            let root_path = root.root.to_string_lossy().to_string();
            args.push("--bind".to_string());
            args.push(root_path.clone());
            args.push(root_path);
        }

        for subpath in writable_roots
            .iter()
            .flat_map(|root| &root.read_only_subpaths)
        {
            if !subpath.exists() {
                continue;
            }
            let ro_path = subpath.to_string_lossy().to_string();
            args.push("--ro-bind".to_string());
            args.push(ro_path.clone());
            args.push(ro_path);
        }
    }

    if !policy.has_full_network_access() {
        args.push("--unshare-net".to_string());
    }
    args.push("--unshare-pid".to_string());
    args.push("--proc".to_string());
    args.push("/proc".to_string());
    args.push("--dev-bind".to_string());
    args.push("/dev/null".to_string());
    args.push("/dev/null".to_string());
    args.push("--".to_string());
    args.extend(command);

    let mut cmd = Command::new("bwrap");
    cmd.args(args);
    Ok(cmd)
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
