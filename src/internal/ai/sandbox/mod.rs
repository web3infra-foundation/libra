use std::{path::Path, process::Stdio, sync::Arc, time::Duration};

use tokio::{io::AsyncReadExt, process::Command, sync::Mutex};

pub mod policy;

pub use policy::{NetworkAccess, SandboxPermissions, SandboxPolicy, WritableRoot};

#[cfg(target_os = "macos")]
use std::path::PathBuf;

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

    let args = build_macos_seatbelt_args(command, policy, cwd);
    let mut cmd = Command::new(SEATBELT_EXECUTABLE);
    cmd.args(args);
    Ok(cmd)
}

#[cfg(target_os = "macos")]
fn build_macos_seatbelt_args(command: Vec<String>, policy: &SandboxPolicy, cwd: &Path) -> Vec<String> {
    const SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");
    const SEATBELT_NETWORK_POLICY: &str = include_str!("seatbelt_network_policy.sbpl");

    let (file_write_policy, file_write_params) = build_macos_file_write_policy(policy, cwd);
    let network_policy = if policy.has_full_network_access() {
        SEATBELT_NETWORK_POLICY
    } else {
        ""
    };

    let full_policy = format!(
        "{SEATBELT_BASE_POLICY}\n; allow read-only file operations\n(allow file-read*)\n{file_write_policy}\n{network_policy}"
    );

    let mut args = vec!["-p".to_string(), full_policy];
    let dir_params = [file_write_params, macos_dir_params()].concat();
    args.extend(
        dir_params
            .into_iter()
            .map(|(key, value)| format!("-D{key}={}", value.to_string_lossy())),
    );
    args.push("--".to_string());
    args.extend(command);
    args
}

#[cfg(target_os = "macos")]
fn build_macos_file_write_policy(
    policy: &SandboxPolicy,
    cwd: &Path,
) -> (String, Vec<(String, PathBuf)>) {
    if policy.has_full_disk_write_access() {
        return (
            r#"(allow file-write* (regex #"^/"))"#.to_string(),
            Vec::new(),
        );
    }

    let writable_roots = policy.get_writable_roots_with_cwd(cwd);
    let mut writable_folder_policies = Vec::new();
    let mut file_write_params = Vec::new();

    for (index, writable_root) in writable_roots.iter().enumerate() {
        let canonical_root = writable_root
            .root
            .canonicalize()
            .unwrap_or_else(|_| writable_root.root.clone());
        let root_param = format!("WRITABLE_ROOT_{index}");
        file_write_params.push((root_param.clone(), canonical_root));

        if writable_root.read_only_subpaths.is_empty() {
            writable_folder_policies.push(format!("(subpath (param \"{root_param}\"))"));
            continue;
        }

        let mut require_parts = vec![format!("(subpath (param \"{root_param}\"))")];
        for (subpath_index, read_only_subpath) in writable_root.read_only_subpaths.iter().enumerate()
        {
            let canonical_read_only_subpath = read_only_subpath
                .canonicalize()
                .unwrap_or_else(|_| read_only_subpath.clone());
            let read_only_param = format!("WRITABLE_ROOT_{index}_RO_{subpath_index}");
            file_write_params.push((read_only_param.clone(), canonical_read_only_subpath));
            require_parts.push(format!(
                "(require-not (subpath (param \"{read_only_param}\")))"
            ));
        }
        writable_folder_policies.push(format!("(require-all {} )", require_parts.join(" ")));
    }

    if writable_folder_policies.is_empty() {
        ("".to_string(), file_write_params)
    } else {
        (
            format!(
                "(allow file-write*\n{}\n)",
                writable_folder_policies.join(" ")
            ),
            file_write_params,
        )
    }
}

#[cfg(target_os = "macos")]
fn macos_dir_params() -> Vec<(String, PathBuf)> {
    if let Some(path) = std::env::var_os("DARWIN_USER_CACHE_DIR")
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok().or(Some(path)))
    {
        return vec![("DARWIN_USER_CACHE_DIR".to_string(), path)];
    }

    if let Some(path) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library").join("Caches"))
        .and_then(|path| path.canonicalize().ok().or(Some(path)))
    {
        return vec![("DARWIN_USER_CACHE_DIR".to_string(), path)];
    }

    Vec::new()
}

#[cfg(target_os = "linux")]
fn build_linux_bwrap_command(
    command: Vec<String>,
    cwd: &Path,
    policy: &SandboxPolicy,
) -> Result<Command, String> {
    let mut args = Vec::new();
    args.push("--new-session".to_string());
    args.push("--die-with-parent".to_string());
    args.extend(build_linux_filesystem_args(policy, cwd)?);
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

#[cfg(target_os = "linux")]
fn build_linux_filesystem_args(policy: &SandboxPolicy, cwd: &Path) -> Result<Vec<String>, String> {
    if policy.has_full_disk_write_access() {
        return Ok(Vec::new());
    }

    let writable_roots = policy.get_writable_roots_with_cwd(cwd);
    ensure_linux_mount_targets_exist(&writable_roots)?;
    let allowed_write_paths: Vec<std::path::PathBuf> = writable_roots
        .iter()
        .map(|root| root.root.clone())
        .collect();

    let mut args = Vec::new();
    args.push("--ro-bind".to_string());
    args.push("/".to_string());
    args.push("/".to_string());

    for writable_root in &writable_roots {
        let root = writable_root.root.to_string_lossy().to_string();
        args.push("--bind".to_string());
        args.push(root.clone());
        args.push(root);
    }

    for subpath in collect_linux_read_only_subpaths(&writable_roots) {
        if let Some(symlink_path) = find_linux_symlink_in_path(&subpath, &allowed_write_paths) {
            let symlink = symlink_path.to_string_lossy().to_string();
            args.push("--ro-bind".to_string());
            args.push("/dev/null".to_string());
            args.push(symlink);
            continue;
        }

        if !subpath.exists() {
            if let Some(first_missing) = find_linux_first_nonexistent_component(&subpath)
                && is_within_linux_allowed_write_paths(&first_missing, &allowed_write_paths)
            {
                let missing = first_missing.to_string_lossy().to_string();
                args.push("--ro-bind".to_string());
                args.push("/dev/null".to_string());
                args.push(missing);
            }
            continue;
        }

        if is_within_linux_allowed_write_paths(&subpath, &allowed_write_paths) {
            let read_only_path = subpath.to_string_lossy().to_string();
            args.push("--ro-bind".to_string());
            args.push(read_only_path.clone());
            args.push(read_only_path);
        }
    }

    Ok(args)
}

#[cfg(target_os = "linux")]
fn ensure_linux_mount_targets_exist(writable_roots: &[WritableRoot]) -> Result<(), String> {
    for writable_root in writable_roots {
        if !writable_root.root.exists() {
            return Err(format!(
                "sandbox expected writable root {}, but it does not exist",
                writable_root.root.display()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn collect_linux_read_only_subpaths(writable_roots: &[WritableRoot]) -> Vec<std::path::PathBuf> {
    use std::collections::BTreeSet;
    let mut subpaths = BTreeSet::new();
    for writable_root in writable_roots {
        for subpath in &writable_root.read_only_subpaths {
            subpaths.insert(subpath.clone());
        }
    }
    subpaths.into_iter().collect()
}

#[cfg(target_os = "linux")]
fn is_within_linux_allowed_write_paths(
    path: &Path,
    allowed_write_paths: &[std::path::PathBuf],
) -> bool {
    allowed_write_paths.iter().any(|root| path.starts_with(root))
}

#[cfg(target_os = "linux")]
fn find_linux_symlink_in_path(
    target_path: &Path,
    allowed_write_paths: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    use std::path::{Component, PathBuf};

    let mut current = PathBuf::new();
    for component in target_path.components() {
        match component {
            Component::RootDir => {
                current.push(Path::new("/"));
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => continue,
        }

        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => break,
        };
        if metadata.file_type().is_symlink()
            && is_within_linux_allowed_write_paths(&current, allowed_write_paths)
        {
            return Some(current);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn find_linux_first_nonexistent_component(target_path: &Path) -> Option<std::path::PathBuf> {
    use std::path::{Component, PathBuf};

    let mut current = PathBuf::new();
    for component in target_path.components() {
        match component {
            Component::RootDir => {
                current.push(Path::new("/"));
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => continue,
        }

        if !current.exists() {
            return Some(current);
        }
    }
    None
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
