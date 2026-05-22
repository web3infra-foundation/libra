//! Runtime enforcement for sandboxed AI tool execution.
//!
//! Boundary: this module applies parsed policy to concrete process/file operations and
//! must preserve explicit denial reasons for user-facing diagnostics. Hardening tests
//! cover denied commands, allowed commands, and path escape attempts.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[cfg(test)]
use super::NetworkAccess;
#[cfg(target_os = "macos")]
use super::sensitive_read_paths;
use super::{SandboxEnforcement, SandboxPermissions, SandboxPolicy, SandboxPolicyError};
#[cfg(unix)]
use crate::utils::fuse;

pub const LIBRA_SANDBOX_NETWORK_DISABLED_ENV_VAR: &str = "LIBRA_SANDBOX_NETWORK_DISABLED";
const CARGO_TARGET_DIR_ENV_VAR: &str = "CARGO_TARGET_DIR";
#[cfg(target_os = "macos")]
const MACOS_PATH_TO_SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxType {
    None,
    MacosSeatbelt,
    LinuxSeccomp,
    WindowsRestrictedToken,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
    pub sandbox_permissions: SandboxPermissions,
    pub justification: Option<String>,
}

impl CommandSpec {
    pub fn shell(
        command: impl Into<String>,
        cwd: PathBuf,
        timeout_ms: Option<u64>,
        sandbox_permissions: SandboxPermissions,
        justification: Option<String>,
    ) -> Self {
        Self::shell_inner(
            command,
            cwd,
            timeout_ms,
            sandbox_permissions,
            justification,
            std::env::var_os(CARGO_TARGET_DIR_ENV_VAR).is_some(),
        )
    }

    /// Inner constructor that accepts the ambient-env flag explicitly so tests
    /// can pin the FUSE-injection branch without relying on whatever
    /// `CARGO_TARGET_DIR` happens to be exported by the surrounding shell or
    /// CI runner.
    fn shell_inner(
        command: impl Into<String>,
        cwd: PathBuf,
        timeout_ms: Option<u64>,
        sandbox_permissions: SandboxPermissions,
        justification: Option<String>,
        ambient_cargo_target_dir_is_set: bool,
    ) -> Self {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut env = HashMap::new();
        apply_fuse_workspace_env_overrides(&cwd, &mut env, ambient_cargo_target_dir_is_set);
        Self {
            program: shell,
            args: vec!["-c".to_string(), command.into()],
            cwd,
            env,
            timeout_ms,
            sandbox_permissions,
            justification,
        }
    }
}

/// Inject env-var overrides for commands launched inside FUSE-backed task
/// worktrees.
///
/// The libfuse-fs overlay rejects some directory-creation calls with `EPERM`,
/// which breaks `cargo` (it cannot create `./target` inside the worktree).
/// Pointing `CARGO_TARGET_DIR` to a stable path outside the FUSE mount lets
/// builds and gate checks succeed without modifying workspace contents.
///
/// Existing values are preserved: if the caller has already set
/// `CARGO_TARGET_DIR` in `env` or the ambient process environment exposes one
/// (per `ambient_cargo_target_dir_is_set`), we leave the choice to the
/// user/operator.
fn apply_fuse_workspace_env_overrides(
    cwd: &Path,
    env: &mut HashMap<String, String>,
    ambient_cargo_target_dir_is_set: bool,
) {
    #[cfg(not(unix))]
    {
        let _ = (cwd, env, ambient_cargo_target_dir_is_set);
    }

    #[cfg(unix)]
    {
        if env.contains_key(CARGO_TARGET_DIR_ENV_VAR) || ambient_cargo_target_dir_is_set {
            return;
        }
        let Some(target_dir) = fuse::fuse_workspace_cargo_target_dir(cwd) else {
            return;
        };
        env.insert(
            CARGO_TARGET_DIR_ENV_VAR.to_string(),
            target_dir.to_string_lossy().into_owned(),
        );
    }
}

#[derive(Debug, Clone)]
pub struct ExecEnv {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
    pub sandbox: SandboxType,
    pub sandbox_permissions: SandboxPermissions,
    pub justification: Option<String>,
    pub arg0: Option<String>,
    pub new_session: bool,
}

impl ExecEnv {
    pub fn into_command(self) -> Result<(Command, Option<u64>), String> {
        let (program, args) = self
            .command
            .split_first()
            .ok_or_else(|| "missing command program".to_string())?;

        let mut command = Command::new(program);
        command.args(args);
        command.current_dir(self.cwd);
        command.envs(self.env);
        if self.new_session {
            configure_new_session(&mut command);
        }
        Ok((command, self.timeout_ms))
    }
}

#[cfg(unix)]
fn configure_new_session(command: &mut Command) {
    // SAFETY: `pre_exec` runs in the child after fork and before exec. The
    // closure only invokes the async-signal-safe `setsid(2)` syscall and
    // converts its errno into an owned `std::io::Error`.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn configure_new_session(_command: &mut Command) {}

pub struct SandboxTransformRequest<'a> {
    pub spec: CommandSpec,
    pub policy: Option<&'a SandboxPolicy>,
    pub sandbox_policy_cwd: &'a Path,
    pub linux_sandbox_exe: Option<&'a PathBuf>,
    pub use_linux_sandbox_bwrap: bool,
    pub enforcement: SandboxEnforcement,
    pub deny_read_paths: &'a [PathBuf],
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxTransformError {
    #[error("missing command program")]
    MissingProgram,
    #[error("failed to serialize sandbox policy for linux sandbox: {0}")]
    LinuxPolicySerialize(#[from] serde_json::Error),
    #[error("missing linux sandbox executable path")]
    MissingLinuxSandboxExecutable,
    #[error("windows restricted sandbox is not implemented yet")]
    WindowsSandboxNotImplemented,
    #[error("sandboxed command execution is not supported on this platform")]
    UnsupportedPlatform,
    #[error("sandbox enforcement failed: {reason}")]
    EnforcementFailed { reason: String },
    /// `NetworkAccess::Allowlist` was requested but the
    /// per-allowlist proxy is unavailable, and
    /// [`SandboxEnforcement::Required`] forbids degrading to
    /// `Denied`. Surfaced ahead of Phase 7's full proxy wire-up so
    /// the runtime has a stable error shape to fail closed with — see
    /// `docs/improvement/sandbox.md` §7.4 line 341.
    ///
    /// `reason` carries an actionable hint (which proxy backend was
    /// expected, why it didn't start, etc.) so users can recover
    /// without having to re-derive the failure from the surrounding
    /// transform context. Audit consumers may surface this verbatim
    /// in the `ToolInvocation[E]` evidence record.
    #[error("network enforcement failed: {reason}")]
    NetworkEnforcementFailed { reason: String },
    #[error(transparent)]
    InvalidPolicy(#[from] SandboxPolicyError),
}

#[derive(Default)]
pub struct SandboxManager;

impl SandboxManager {
    pub fn new() -> Self {
        Self
    }

    pub fn select_initial(
        &self,
        policy: Option<&SandboxPolicy>,
        permissions: SandboxPermissions,
    ) -> SandboxType {
        if permissions.requires_escalated_permissions() {
            return SandboxType::None;
        }

        let Some(policy) = policy else {
            return SandboxType::None;
        };

        if matches!(
            policy,
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
        ) {
            return SandboxType::None;
        }

        #[cfg(target_os = "macos")]
        {
            SandboxType::MacosSeatbelt
        }
        #[cfg(target_os = "linux")]
        {
            SandboxType::LinuxSeccomp
        }
        #[cfg(target_os = "windows")]
        {
            SandboxType::WindowsRestrictedToken
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            SandboxType::None
        }
    }

    pub fn transform(
        &self,
        request: SandboxTransformRequest<'_>,
    ) -> Result<ExecEnv, SandboxTransformError> {
        let SandboxTransformRequest {
            spec,
            policy,
            sandbox_policy_cwd,
            linux_sandbox_exe,
            use_linux_sandbox_bwrap,
            enforcement,
            deny_read_paths,
        } = request;

        #[cfg(not(target_os = "linux"))]
        let _ = use_linux_sandbox_bwrap;
        #[cfg(not(target_os = "linux"))]
        let _ = linux_sandbox_exe;
        #[cfg(not(target_os = "macos"))]
        let _ = deny_read_paths;
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = sandbox_policy_cwd;

        if spec.program.is_empty() {
            return Err(SandboxTransformError::MissingProgram);
        }

        if !spec.sandbox_permissions.requires_escalated_permissions()
            && let Some(policy) = policy
        {
            policy.validate_writable_roots_with_cwd(sandbox_policy_cwd)?;
        }

        let mut env = spec.env;
        if policy.is_some_and(|sandbox_policy| !sandbox_policy.has_full_network_access()) {
            env.insert(
                LIBRA_SANDBOX_NETWORK_DISABLED_ENV_VAR.to_string(),
                "1".to_string(),
            );
        }

        let mut command = Vec::with_capacity(1 + spec.args.len());
        command.push(spec.program.clone());
        command.extend(spec.args.clone());

        let sandbox = self.select_initial(policy, spec.sandbox_permissions);
        if matches!(sandbox, SandboxType::None)
            && enforcement.requires_effective_sandbox()
            && internal_sandbox_required(policy, spec.sandbox_permissions)
        {
            return Err(SandboxTransformError::EnforcementFailed {
                reason: "sandbox enforcement is required, but this platform has no supported internal sandbox backend for the selected policy".to_string(),
            });
        }

        let (command, arg0, effective_sandbox) = match sandbox {
            SandboxType::None => (command, None, SandboxType::None),
            SandboxType::MacosSeatbelt => {
                #[cfg(target_os = "macos")]
                {
                    let policy = policy.ok_or(SandboxTransformError::UnsupportedPlatform)?;
                    let mut seatbelt_args = create_seatbelt_command_args(
                        command,
                        policy,
                        sandbox_policy_cwd,
                        deny_read_paths,
                    );
                    let mut full = Vec::with_capacity(1 + seatbelt_args.len());
                    full.push(MACOS_PATH_TO_SEATBELT_EXECUTABLE.to_string());
                    full.append(&mut seatbelt_args);
                    (full, None, SandboxType::MacosSeatbelt)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return Err(SandboxTransformError::UnsupportedPlatform);
                }
            }
            SandboxType::LinuxSeccomp => {
                #[cfg(target_os = "linux")]
                {
                    let policy = policy.ok_or(SandboxTransformError::UnsupportedPlatform)?;
                    if let Some(linux_sandbox_exe) = linux_sandbox_exe {
                        let mut sandbox_args = create_linux_sandbox_command_args(
                            command,
                            policy,
                            sandbox_policy_cwd,
                            use_linux_sandbox_bwrap,
                        )?;
                        let mut full = Vec::with_capacity(1 + sandbox_args.len());
                        full.push(linux_sandbox_exe.to_string_lossy().to_string());
                        full.append(&mut sandbox_args);
                        (
                            full,
                            Some("libra-linux-sandbox".to_string()),
                            SandboxType::LinuxSeccomp,
                        )
                    } else if let Some(bwrap_path) = locate_bwrap_binary() {
                        // OC-Phase 7 P0 #2: built-in bwrap real-execution
                        // path. When the external `libra-linux-sandbox`
                        // helper is not configured but `bwrap` is on the
                        // host PATH, construct the bwrap command directly
                        // via `create_bwrap_command_args` instead of
                        // falling back to no-sandbox. This closes the
                        // doc gap at sandbox.md L26 ("内建 bwrap 执行
                        // 选择...仍未进入主干实现") and the previous
                        // EnforcementFailed reason string that
                        // explicitly named "the built-in bwrap sandbox
                        // is not available yet".
                        let mut bwrap_args = create_bwrap_command_args(
                            command,
                            policy,
                            sandbox_policy_cwd,
                            deny_read_paths,
                        );
                        let mut full = Vec::with_capacity(1 + bwrap_args.len());
                        full.push(bwrap_path.to_string_lossy().into_owned());
                        full.append(&mut bwrap_args);
                        tracing::info!(
                            bwrap = %bwrap_path.display(),
                            "using built-in bwrap sandbox (LIBRA_LINUX_SANDBOX_EXE unset; helper-less path)",
                        );
                        (
                            full,
                            Some("libra-linux-sandbox-bwrap".to_string()),
                            SandboxType::LinuxSeccomp,
                        )
                    } else {
                        if enforcement.requires_effective_sandbox() {
                            return Err(SandboxTransformError::EnforcementFailed {
                                reason: "Linux sandbox enforcement is required, but LIBRA_LINUX_SANDBOX_EXE is not configured and `bwrap` was not found on PATH; install bubblewrap (apt install bubblewrap / dnf install bubblewrap) or set LIBRA_LINUX_SANDBOX_EXE to the helper path".to_string(),
                            });
                        }
                        tracing::warn!(
                            "linux sandbox executable not configured and bwrap not on PATH; running command without linux sandbox"
                        );
                        (command, None, SandboxType::None)
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    return Err(SandboxTransformError::UnsupportedPlatform);
                }
            }
            SandboxType::WindowsRestrictedToken => {
                #[cfg(target_os = "windows")]
                {
                    return Err(SandboxTransformError::WindowsSandboxNotImplemented);
                }
                #[cfg(not(target_os = "windows"))]
                {
                    return Err(SandboxTransformError::UnsupportedPlatform);
                }
            }
        };

        Ok(ExecEnv {
            command,
            cwd: spec.cwd,
            env,
            timeout_ms: spec.timeout_ms,
            sandbox: effective_sandbox,
            sandbox_permissions: spec.sandbox_permissions,
            justification: spec.justification,
            arg0,
            new_session: matches!(
                effective_sandbox,
                SandboxType::MacosSeatbelt | SandboxType::LinuxSeccomp
            ),
        })
    }
}

fn internal_sandbox_required(
    policy: Option<&SandboxPolicy>,
    permissions: SandboxPermissions,
) -> bool {
    if permissions.requires_escalated_permissions() {
        return false;
    }

    matches!(
        policy,
        Some(SandboxPolicy::ReadOnly | SandboxPolicy::WorkspaceWrite { .. })
    )
}

#[cfg(target_os = "linux")]
fn create_linux_sandbox_command_args(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    sandbox_policy_cwd: &Path,
    use_bwrap_sandbox: bool,
) -> Result<Vec<String>, SandboxTransformError> {
    let mut args = vec![
        "--sandbox-policy-cwd".to_string(),
        sandbox_policy_cwd.to_string_lossy().to_string(),
        "--sandbox-policy".to_string(),
        serde_json::to_string(sandbox_policy)?,
    ];
    if use_bwrap_sandbox {
        args.push("--use-bwrap-sandbox".to_string());
    }
    args.push("--".to_string());
    args.extend(command);
    Ok(args)
}

pub fn create_bwrap_command_args(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    sandbox_policy_cwd: &Path,
    deny_read_paths: &[PathBuf],
) -> Vec<String> {
    let mut args = vec![
        "--unshare-all".to_string(),
        "--die-with-parent".to_string(),
        "--new-session".to_string(),
    ];
    if sandbox_policy.has_full_network_access() {
        args.push("--share-net".to_string());
    } else {
        args.push("--unshare-net".to_string());
    }

    args.extend([
        "--proc".to_string(),
        "/proc".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
    ]);

    for path in bwrap_read_only_host_paths() {
        push_bwrap_mount(&mut args, "--ro-bind", &path);
    }

    let writable_roots = sandbox_policy.get_writable_roots_with_cwd(sandbox_policy_cwd);
    if writable_roots.is_empty() {
        push_bwrap_mount(&mut args, "--ro-bind", sandbox_policy_cwd);
    } else {
        for writable_root in writable_roots {
            push_bwrap_mount(&mut args, "--bind", &writable_root.root);
            for read_only_subpath in writable_root.read_only_subpaths {
                push_bwrap_mount(&mut args, "--ro-bind", &read_only_subpath);
            }
        }
    }

    for path in deny_read_paths {
        args.push("--tmpfs".to_string());
        args.push(path.to_string_lossy().into_owned());
    }

    args.push("--".to_string());
    args.extend(command);
    args
}

/// Locate `bwrap` on the host PATH and return the resolved
/// absolute path. Used by the built-in (helper-less) Linux
/// sandbox path in [`SandboxManager::transform`] — when
/// `LIBRA_LINUX_SANDBOX_EXE` is unset, the runtime falls back to
/// constructing a `bwrap …` command directly via
/// [`create_bwrap_command_args`].
///
/// The first non-empty `PATH` entry that contains an executable
/// `bwrap` file wins. The probe is intentionally **not** cached:
/// `which`-style discovery is cheap (one stat per PATH entry,
/// typically <10 lookups) and a stale cache would mislead a user
/// that just installed bubblewrap mid-session. Tests that need a
/// deterministic answer can set `LIBRA_BWRAP_BINARY` to bypass
/// the PATH walk entirely.
///
/// Returns `None` on non-Linux platforms (the built-in bwrap
/// path is Linux-only) or when no `bwrap` is reachable. Callers
/// must handle the `None` case before deciding whether to fail
/// closed or fall back to the unsandboxed path.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn locate_bwrap_binary() -> Option<PathBuf> {
    if let Some(override_path) = std::env::var_os("LIBRA_BWRAP_BINARY") {
        let path = PathBuf::from(override_path);
        if path.is_absolute() && path.exists() {
            return Some(path);
        }
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let path_env = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join("bwrap");
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// `true` when `path` exists, is a regular file, and the
/// owner/group/world execute bit is set. Used by
/// [`locate_bwrap_binary`] to filter PATH entries that exist as
/// names but are not invocable (a hand-edited symlink, a stale
/// init artifact, etc.).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if !metadata.is_file() {
            return false;
        }
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn bwrap_read_only_host_paths() -> Vec<PathBuf> {
    [
        "/bin",
        "/usr",
        "/lib",
        "/lib64",
        "/etc/hosts",
        "/etc/resolv.conf",
        "/etc/ssl",
        "/etc/ca-certificates",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| path.exists())
    .collect()
}

fn push_bwrap_mount(args: &mut Vec<String>, flag: &str, path: &Path) {
    let path = path.to_string_lossy().into_owned();
    args.push(flag.to_string());
    args.push(path.clone());
    args.push(path);
}

#[cfg(target_os = "macos")]
fn create_seatbelt_command_args(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    sandbox_policy_cwd: &Path,
    deny_read_paths: &[PathBuf],
) -> Vec<String> {
    const SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");
    const SEATBELT_NETWORK_POLICY: &str = include_str!("seatbelt_network_policy.sbpl");

    let (file_write_policy, file_write_params) =
        build_macos_file_write_policy(sandbox_policy, sandbox_policy_cwd);
    let file_read_policy = "; allow read-only file operations\n(allow file-read*)";
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let (sensitive_read_policy, sensitive_read_params) =
        build_macos_sensitive_read_policy(home.as_deref(), deny_read_paths);
    let network_policy = if sandbox_policy.has_full_network_access() {
        SEATBELT_NETWORK_POLICY
    } else {
        ""
    };
    let full_policy = format!(
        "{SEATBELT_BASE_POLICY}\n{file_read_policy}\n{sensitive_read_policy}{file_write_policy}\n{network_policy}"
    );

    let mut seatbelt_args = vec!["-p".to_string(), full_policy];
    let dir_params = [file_write_params, sensitive_read_params, macos_dir_params()].concat();
    seatbelt_args.extend(
        dir_params
            .into_iter()
            .map(|(key, value)| format!("-D{key}={}", value.to_string_lossy())),
    );
    seatbelt_args.push("--".to_string());
    seatbelt_args.extend(command);
    seatbelt_args
}

#[cfg(target_os = "macos")]
fn build_macos_sensitive_read_policy(
    home: Option<&Path>,
    deny_read_paths: &[PathBuf],
) -> (String, Vec<(String, PathBuf)>) {
    let mut paths = sensitive_read_paths(home);
    for path in deny_read_paths {
        if !paths.iter().any(|existing| existing == path) {
            paths.push(path.clone());
        }
    }
    let mut params = Vec::with_capacity(paths.len());
    let mut policy = String::from("; deny sensitive host credential reads\n");

    for (index, path) in paths.into_iter().enumerate() {
        let param = format!("SENSITIVE_READ_{index}");
        params.push((param.clone(), path));
        policy.push_str(&format!(
            "(deny file-read* (subpath (param \"{param}\")))\n"
        ));
    }

    (policy, params)
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
        for (subpath_index, read_only_subpath) in
            writable_root.read_only_subpaths.iter().enumerate()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_initial_uses_none_for_escalated_permissions() {
        let manager = SandboxManager::new();
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: NetworkAccess::Denied,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };

        assert_eq!(
            manager.select_initial(Some(&policy), SandboxPermissions::RequireEscalated),
            SandboxType::None
        );
    }

    #[test]
    fn select_initial_uses_none_for_external_sandbox() {
        let manager = SandboxManager::new();
        let policy = SandboxPolicy::ExternalSandbox {
            network_access: super::super::NetworkAccess::Denied,
        };
        assert_eq!(
            manager.select_initial(Some(&policy), SandboxPermissions::UseDefault),
            SandboxType::None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn select_initial_uses_linux_seccomp_when_sandboxed() {
        let manager = SandboxManager::new();
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: NetworkAccess::Denied,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        assert_eq!(
            manager.select_initial(Some(&policy), SandboxPermissions::UseDefault),
            SandboxType::LinuxSeccomp
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn transform_linux_seccomp_falls_back_when_helper_is_missing() {
        let manager = SandboxManager::new();
        let cwd = std::env::temp_dir();
        let request = SandboxTransformRequest {
            spec: CommandSpec::shell(
                "echo ok",
                cwd.clone(),
                Some(1_000),
                SandboxPermissions::UseDefault,
                None,
            ),
            policy: Some(&SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![],
                network_access: NetworkAccess::Denied,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }),
            sandbox_policy_cwd: &cwd,
            linux_sandbox_exe: None,
            use_linux_sandbox_bwrap: false,
            enforcement: SandboxEnforcement::BestEffort,
            deny_read_paths: &[],
        };

        let transformed = manager
            .transform(request)
            .expect("transform should fallback");
        assert_eq!(transformed.sandbox, SandboxType::None);
        assert!(!transformed.new_session);
        assert!(!transformed.command.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn transform_linux_required_enforcement_rejects_missing_helper() {
        let manager = SandboxManager::new();
        let cwd = std::env::temp_dir();
        let request = SandboxTransformRequest {
            spec: CommandSpec::shell(
                "echo ok",
                cwd.clone(),
                Some(1_000),
                SandboxPermissions::UseDefault,
                None,
            ),
            policy: Some(&SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![],
                network_access: NetworkAccess::Denied,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }),
            sandbox_policy_cwd: &cwd,
            linux_sandbox_exe: None,
            use_linux_sandbox_bwrap: false,
            enforcement: SandboxEnforcement::Required,
            deny_read_paths: &[],
        };

        let error = manager
            .transform(request)
            .expect_err("required enforcement must not silently fall back");
        assert!(
            error
                .to_string()
                .contains("Linux sandbox enforcement is required"),
            "unexpected error: {error}",
        );
    }

    #[test]
    fn create_bwrap_command_args_denies_network_by_default_with_unshare_net() {
        let cwd = Path::new("/tmp/libra-sandbox-workspace");
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: NetworkAccess::Denied,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        let args = create_bwrap_command_args(command(), &policy, cwd, &[]);

        assert!(args.contains(&"--unshare-all".to_string()));
        assert!(args.contains(&"--unshare-net".to_string()));
        assert!(!args.contains(&"--share-net".to_string()));
        assert_option_value(&args, "--tmpfs", "/tmp");
    }

    #[test]
    fn create_bwrap_command_args_allows_full_network_with_share_net() {
        let cwd = Path::new("/tmp/libra-sandbox-workspace");
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: NetworkAccess::Full,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        let args = create_bwrap_command_args(command(), &policy, cwd, &[]);

        assert!(args.contains(&"--share-net".to_string()));
        assert!(!args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn create_bwrap_command_args_includes_new_session_and_die_with_parent() {
        let cwd = Path::new("/tmp/libra-sandbox-workspace");

        let args = create_bwrap_command_args(command(), &SandboxPolicy::ReadOnly, cwd, &[]);

        assert!(args.contains(&"--new-session".to_string()));
        assert!(args.contains(&"--die-with-parent".to_string()));
    }

    #[test]
    fn create_bwrap_command_args_binds_workspace_roots_and_protected_subpaths() {
        let cwd = Path::new("/tmp/libra-sandbox-workspace");
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("src")],
            network_access: NetworkAccess::Denied,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };
        let writable_root = cwd.join("src");

        let args = create_bwrap_command_args(command(), &policy, cwd, &[]);

        assert_mount(&args, "--bind", &writable_root);
        assert_mount(&args, "--ro-bind", &writable_root.join(".git"));
        assert_mount(&args, "--ro-bind", &writable_root.join(".libra"));
    }

    #[test]
    fn create_bwrap_command_args_masks_sensitive_read_paths_with_tmpfs() {
        let cwd = Path::new("/tmp/libra-sandbox-workspace");
        let secret = PathBuf::from("/home/tester/.ssh");

        let args = create_bwrap_command_args(command(), &SandboxPolicy::ReadOnly, cwd, &[secret]);

        assert_option_value(&args, "--tmpfs", "/home/tester/.ssh");
    }

    /// `locate_bwrap_binary` honours the `LIBRA_BWRAP_BINARY`
    /// override env var: an absolute path that exists is returned
    /// verbatim; a non-existent override returns None even when
    /// `bwrap` is on PATH. Pins the test escape hatch so a CI
    /// matrix can run the built-in bwrap path without relying on
    /// host-installed bubblewrap.
    #[test]
    fn locate_bwrap_binary_honours_override_env_var() {
        let tmpdir = tempfile::tempdir().expect("tempdir for override probe");
        let fake_bwrap = tmpdir.path().join("bwrap");
        std::fs::write(&fake_bwrap, b"#!/bin/sh\nexit 0\n").expect("write fake bwrap");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_bwrap).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_bwrap, perms).unwrap();
        }

        // SAFETY: test-only env mutation; restored after the
        // probe so other tests in the suite remain isolated.
        let prior = std::env::var_os("LIBRA_BWRAP_BINARY");
        unsafe {
            std::env::set_var("LIBRA_BWRAP_BINARY", &fake_bwrap);
        }
        let found = locate_bwrap_binary();
        unsafe {
            if let Some(value) = prior {
                std::env::set_var("LIBRA_BWRAP_BINARY", value);
            } else {
                std::env::remove_var("LIBRA_BWRAP_BINARY");
            }
        }
        assert_eq!(found.as_deref(), Some(fake_bwrap.as_path()));

        // Non-existent override: probe returns None even though
        // bwrap might be on PATH (we explicitly opt out of PATH
        // walking when the override is set).
        let prior = std::env::var_os("LIBRA_BWRAP_BINARY");
        unsafe {
            std::env::set_var("LIBRA_BWRAP_BINARY", "/does/not/exist/bwrap");
        }
        let found = locate_bwrap_binary();
        unsafe {
            if let Some(value) = prior {
                std::env::set_var("LIBRA_BWRAP_BINARY", value);
            } else {
                std::env::remove_var("LIBRA_BWRAP_BINARY");
            }
        }
        assert!(found.is_none());
    }

    /// `is_executable_file` returns true only for regular files
    /// with the execute bit set; rejects directories and
    /// permission-less files. Pin the predicate so the probe's
    /// PATH walk doesn't accidentally accept a `bwrap` directory
    /// or a non-executable shim.
    #[cfg(unix)]
    #[test]
    fn is_executable_file_filters_directories_and_non_exec() {
        use std::os::unix::fs::PermissionsExt;

        let tmpdir = tempfile::tempdir().expect("tempdir for exec probe");

        // Directory: not executable in the sense `which` uses
        // (it's not a regular file).
        let dir = tmpdir.path().join("bwrap-dir");
        std::fs::create_dir(&dir).expect("create bwrap-dir");
        assert!(!is_executable_file(&dir));

        // Regular file without execute bit.
        let no_exec = tmpdir.path().join("bwrap-noexec");
        std::fs::write(&no_exec, b"not exec").unwrap();
        let mut perms = std::fs::metadata(&no_exec).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&no_exec, perms).unwrap();
        assert!(!is_executable_file(&no_exec));

        // Regular file with execute bit.
        let exec = tmpdir.path().join("bwrap-exec");
        std::fs::write(&exec, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&exec).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&exec, perms).unwrap();
        assert!(is_executable_file(&exec));
    }

    /// Pin the built-in bwrap real-execution path: when
    /// `LIBRA_LINUX_SANDBOX_EXE` is unset and the override probe
    /// finds a `bwrap` binary, `transform` constructs the bwrap
    /// command directly (no external helper) and returns
    /// `SandboxType::LinuxSeccomp` with the bwrap path as the
    /// program. Linux-only because the bwrap fallback branch is
    /// gated behind `#[cfg(target_os = "linux")]`.
    #[cfg(target_os = "linux")]
    #[test]
    fn transform_uses_built_in_bwrap_when_helper_is_missing_but_bwrap_is_available() {
        let tmpdir = tempfile::tempdir().expect("tempdir for built-in bwrap test");
        let fake_bwrap = tmpdir.path().join("bwrap");
        std::fs::write(&fake_bwrap, b"#!/bin/sh\nexit 0\n").expect("write fake bwrap");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake_bwrap).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_bwrap, perms).unwrap();

        let prior = std::env::var_os("LIBRA_BWRAP_BINARY");
        // SAFETY: test-only env mutation; restored below.
        unsafe {
            std::env::set_var("LIBRA_BWRAP_BINARY", &fake_bwrap);
        }

        let manager = SandboxManager::new();
        let cwd = std::env::temp_dir();
        let request = SandboxTransformRequest {
            spec: CommandSpec::shell(
                "echo ok",
                cwd.clone(),
                Some(1_000),
                SandboxPermissions::UseDefault,
                None,
            ),
            policy: Some(&SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![],
                network_access: NetworkAccess::Denied,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }),
            sandbox_policy_cwd: &cwd,
            linux_sandbox_exe: None,
            use_linux_sandbox_bwrap: false,
            enforcement: SandboxEnforcement::Required,
            deny_read_paths: &[],
        };

        let result = manager.transform(request);

        unsafe {
            if let Some(value) = prior {
                std::env::set_var("LIBRA_BWRAP_BINARY", value);
            } else {
                std::env::remove_var("LIBRA_BWRAP_BINARY");
            }
        }

        let env = result.expect("built-in bwrap path must succeed");
        assert_eq!(env.sandbox, SandboxType::LinuxSeccomp);
        let program = env
            .command
            .first()
            .cloned()
            .expect("transform must yield a non-empty command vector");
        assert_eq!(program, fake_bwrap.to_string_lossy());
        assert!(
            env.command.iter().any(|arg| arg == "--unshare-all"),
            "built-in bwrap command must include --unshare-all",
        );
    }

    #[test]
    fn create_bwrap_command_args_appends_command_after_delimiter() {
        let cwd = Path::new("/tmp/libra-sandbox-workspace");
        let args = create_bwrap_command_args(command(), &SandboxPolicy::ReadOnly, cwd, &[]);
        let delimiter = args
            .iter()
            .position(|arg| arg == "--")
            .expect("bwrap args must include command delimiter");

        assert_eq!(
            &args[(delimiter + 1)..],
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo ok".to_string()
            ]
        );
    }

    fn command() -> Vec<String> {
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo ok".to_string(),
        ]
    }

    fn assert_mount(args: &[String], flag: &str, path: &Path) {
        let value = path.to_string_lossy();
        assert!(
            args.windows(3).any(|window| {
                window[0] == flag && window[1] == value.as_ref() && window[2] == value.as_ref()
            }),
            "missing {flag} pair for {value}; args: {args:?}",
        );
    }

    fn assert_option_value(args: &[String], flag: &str, value: &str) {
        assert!(
            args.windows(2)
                .any(|window| window[0] == flag && window[1] == value),
            "missing {flag} value {value}; args: {args:?}",
        );
    }

    #[test]
    fn transform_rejects_dangerous_writable_roots_before_execution() {
        let manager = SandboxManager::new();
        let cwd = std::env::temp_dir();
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("/var/run/docker.sock")],
            network_access: NetworkAccess::Denied,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };
        let request = SandboxTransformRequest {
            spec: CommandSpec::shell(
                "echo ok",
                cwd.clone(),
                Some(1_000),
                SandboxPermissions::UseDefault,
                None,
            ),
            policy: Some(&policy),
            sandbox_policy_cwd: &cwd,
            linux_sandbox_exe: None,
            use_linux_sandbox_bwrap: false,
            enforcement: SandboxEnforcement::BestEffort,
            deny_read_paths: &[],
        };

        let error = manager
            .transform(request)
            .expect_err("dangerous writable roots must be rejected");

        assert!(
            error.to_string().contains("Docker socket access"),
            "unexpected error: {error}",
        );
    }

    #[test]
    fn escalated_transform_bypasses_dangerous_writable_root_validation() {
        let manager = SandboxManager::new();
        let cwd = std::env::temp_dir();
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("/var/run/docker.sock")],
            network_access: NetworkAccess::Denied,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };
        let request = SandboxTransformRequest {
            spec: CommandSpec::shell(
                "echo ok",
                cwd.clone(),
                Some(1_000),
                SandboxPermissions::RequireEscalated,
                None,
            ),
            policy: Some(&policy),
            sandbox_policy_cwd: &cwd,
            linux_sandbox_exe: None,
            use_linux_sandbox_bwrap: false,
            enforcement: SandboxEnforcement::Required,
            deny_read_paths: &[],
        };

        let transformed = manager
            .transform(request)
            .expect("explicit escalation should bypass sandbox policy validation");
        assert_eq!(transformed.sandbox, SandboxType::None);
        assert!(!transformed.new_session);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_env_new_session_runs_child_as_session_leader() {
        let env = ExecEnv {
            command: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 5".to_string(),
            ],
            cwd: std::env::temp_dir(),
            env: HashMap::new(),
            timeout_ms: Some(1_000),
            sandbox: SandboxType::MacosSeatbelt,
            sandbox_permissions: SandboxPermissions::UseDefault,
            justification: None,
            arg0: None,
            new_session: true,
        };
        let (mut command, _) = env.into_command().expect("exec env should build");
        let mut child = command.spawn().expect("child should spawn");
        let pid = child.id().expect("spawned child should expose a pid");
        // SAFETY: `getsid` only reads kernel process metadata for the spawned
        // child PID while it is still alive.
        let sid = unsafe { libc::getsid(pid as libc::pid_t) };
        let _ = child.kill().await;
        assert_eq!(
            sid, pid as libc::pid_t,
            "setsid should make the child its session leader; sid={sid} pid={pid}"
        );
    }

    #[test]
    fn shell_command_spec_uses_current_shell() {
        let cwd = std::env::temp_dir();
        let spec = CommandSpec::shell(
            "echo ok",
            cwd.clone(),
            Some(1_000),
            SandboxPermissions::UseDefault,
            None,
        );

        assert_eq!(spec.cwd, cwd);
        assert_eq!(spec.args, vec!["-c".to_string(), "echo ok".to_string()]);
        assert!(!spec.program.is_empty());
    }

    #[test]
    fn apply_fuse_workspace_env_overrides_sets_cargo_target_dir_inside_fuse_worktree() {
        let cwd =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace/src");
        let mut env = HashMap::new();
        apply_fuse_workspace_env_overrides(cwd, &mut env, false);
        let target = env
            .get(CARGO_TARGET_DIR_ENV_VAR)
            .expect("CARGO_TARGET_DIR should be set inside FUSE worktree");
        let expected = std::env::temp_dir()
            .join("libra-fuse-cargo-target")
            .join("libra-task-worktree-fuse-7-019d");
        assert_eq!(target, &expected.to_string_lossy().into_owned());
    }

    #[test]
    fn apply_fuse_workspace_env_overrides_skips_when_caller_already_set_target_dir() {
        let cwd =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace");
        let mut env = HashMap::new();
        env.insert(
            CARGO_TARGET_DIR_ENV_VAR.to_string(),
            "/explicit/target".to_string(),
        );
        apply_fuse_workspace_env_overrides(cwd, &mut env, false);
        assert_eq!(
            env.get(CARGO_TARGET_DIR_ENV_VAR).map(String::as_str),
            Some("/explicit/target")
        );
    }

    #[test]
    fn apply_fuse_workspace_env_overrides_skips_when_ambient_env_has_target_dir() {
        let cwd =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace");
        let mut env = HashMap::new();
        apply_fuse_workspace_env_overrides(cwd, &mut env, true);
        assert!(
            !env.contains_key(CARGO_TARGET_DIR_ENV_VAR),
            "must not override an operator-supplied CARGO_TARGET_DIR"
        );
    }

    #[test]
    fn apply_fuse_workspace_env_overrides_noops_outside_fuse_worktree() {
        let mut env = HashMap::new();
        apply_fuse_workspace_env_overrides(Path::new("/repo/src"), &mut env, false);
        assert!(env.is_empty());
    }

    #[test]
    fn shell_command_spec_injects_cargo_target_dir_inside_fuse_workspace() {
        // Production wrapper test: drives the inner constructor with an
        // explicit ambient-env flag so we don't rely on whatever the test
        // runner exports for `CARGO_TARGET_DIR`. This covers the wiring from
        // `CommandSpec::shell{,_inner}` through `apply_fuse_workspace_env_overrides`.
        let cwd =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-9-019e/workspace");
        let spec = CommandSpec::shell_inner(
            "cargo build",
            cwd.to_path_buf(),
            None,
            SandboxPermissions::UseDefault,
            None,
            false,
        );
        let expected = std::env::temp_dir()
            .join("libra-fuse-cargo-target")
            .join("libra-task-worktree-fuse-9-019e")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            spec.env.get(CARGO_TARGET_DIR_ENV_VAR).map(String::as_str),
            Some(expected.as_str()),
            "CommandSpec::shell must redirect cargo's target dir for FUSE workspaces"
        );
    }

    #[test]
    fn shell_command_spec_skips_injection_when_ambient_env_has_target_dir() {
        // When the operator has `CARGO_TARGET_DIR` exported the inner
        // constructor must respect that choice, even inside a FUSE worktree.
        let cwd =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-9-019e/workspace");
        let spec = CommandSpec::shell_inner(
            "cargo build",
            cwd.to_path_buf(),
            None,
            SandboxPermissions::UseDefault,
            None,
            true,
        );
        assert!(
            !spec.env.contains_key(CARGO_TARGET_DIR_ENV_VAR),
            "CommandSpec::shell must defer to the ambient CARGO_TARGET_DIR"
        );
    }

    #[test]
    fn shell_command_spec_does_not_inject_cargo_target_dir_outside_fuse_workspace() {
        let cwd = std::env::temp_dir();
        let spec = CommandSpec::shell_inner(
            "echo ok",
            cwd,
            None,
            SandboxPermissions::UseDefault,
            None,
            false,
        );
        assert!(
            !spec.env.contains_key(CARGO_TARGET_DIR_ENV_VAR),
            "CommandSpec::shell must not redirect cargo target dir outside FUSE workspaces"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sensitive_read_policy_denies_home_credentials() {
        let extra = vec![PathBuf::from("/Users/tester/Library/Cookies")];
        let (policy, params) =
            build_macos_sensitive_read_policy(Some(Path::new("/Users/tester")), &extra);

        assert!(policy.contains("(deny file-read*"));
        assert!(policy.contains("SENSITIVE_READ_0"));
        assert!(
            params
                .iter()
                .any(|(_, path)| path == Path::new("/Users/tester/.ssh"))
        );
        assert!(
            params
                .iter()
                .any(|(_, path)| path == Path::new("/Users/tester/.aws"))
        );
        assert!(
            params
                .iter()
                .any(|(_, path)| path == Path::new("/etc/shadow"))
        );
        assert!(
            params
                .iter()
                .any(|(_, path)| path == Path::new("/Users/tester/Library/Cookies"))
        );
    }

    #[test]
    fn sandbox_transform_error_display_pins_owned_variants() {
        assert_eq!(
            SandboxTransformError::MissingProgram.to_string(),
            "missing command program",
        );
        assert_eq!(
            SandboxTransformError::MissingLinuxSandboxExecutable.to_string(),
            "missing linux sandbox executable path",
        );
        assert_eq!(
            SandboxTransformError::WindowsSandboxNotImplemented.to_string(),
            "windows restricted sandbox is not implemented yet",
        );
        assert_eq!(
            SandboxTransformError::UnsupportedPlatform.to_string(),
            "sandboxed command execution is not supported on this platform",
        );
        assert_eq!(
            SandboxTransformError::EnforcementFailed {
                reason: "process spawn refused".to_string(),
            }
            .to_string(),
            "sandbox enforcement failed: process spawn refused",
        );
        // v0.17.683: NetworkEnforcementFailed is the network-side
        // sibling of EnforcementFailed. Pin its Display so a future
        // change to the message template doesn't silently shift the
        // audit-record substring downstream consumers grep for.
        assert_eq!(
            SandboxTransformError::NetworkEnforcementFailed {
                reason: "allowlist proxy unavailable in Required mode".to_string(),
            }
            .to_string(),
            "network enforcement failed: allowlist proxy unavailable in Required mode",
        );
    }
}
