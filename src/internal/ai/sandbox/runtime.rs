use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{SandboxPermissions, SandboxPolicy};
#[cfg(target_os = "linux")]
use super::WritableRoot;

pub const LIBRA_SANDBOX_NETWORK_DISABLED_ENV_VAR: &str = "LIBRA_SANDBOX_NETWORK_DISABLED";
const MACOS_PATH_TO_SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxType {
    None,
    MacosSeatbelt,
    LinuxBwrap,
    LinuxHelper,
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
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Self {
            program: shell,
            args: vec!["-c".to_string(), command.into()],
            cwd,
            env: HashMap::new(),
            timeout_ms,
            sandbox_permissions,
            justification,
        }
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
        Ok((command, self.timeout_ms))
    }
}

pub struct SandboxTransformRequest<'a> {
    pub spec: CommandSpec,
    pub policy: Option<&'a SandboxPolicy>,
    pub sandbox_policy_cwd: &'a Path,
    pub linux_sandbox_exe: Option<&'a PathBuf>,
    pub use_linux_sandbox_bwrap: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxTransformError {
    #[error("missing command program")]
    MissingProgram,
    #[error("sandbox expected writable root {0}, but it does not exist")]
    MissingWritableRoot(PathBuf),
    #[error("failed to serialize sandbox policy for linux helper: {0}")]
    LinuxPolicySerialize(#[from] serde_json::Error),
    #[error("missing linux sandbox executable path")]
    MissingLinuxSandboxExecutable,
    #[error("windows restricted sandbox is not implemented yet")]
    WindowsSandboxNotImplemented,
    #[error("sandboxed command execution is not supported on this platform")]
    UnsupportedPlatform,
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
        _linux_sandbox_exe: Option<&PathBuf>,
    ) -> SandboxType {
        if permissions.requires_escalated_permissions() {
            return SandboxType::None;
        }

        let Some(policy) = policy else {
            return SandboxType::None;
        };

        if policy.has_full_disk_write_access() && policy.has_full_network_access() {
            return SandboxType::None;
        }

        #[cfg(target_os = "macos")]
        {
            SandboxType::MacosSeatbelt
        }
        #[cfg(target_os = "linux")]
        {
            if _linux_sandbox_exe.is_some() {
                SandboxType::LinuxHelper
            } else {
                SandboxType::LinuxBwrap
            }
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
        } = request;

        #[cfg(not(target_os = "linux"))]
        let _ = use_linux_sandbox_bwrap;

        if spec.program.is_empty() {
            return Err(SandboxTransformError::MissingProgram);
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

        let sandbox = self.select_initial(policy, spec.sandbox_permissions, linux_sandbox_exe);
        let (command, arg0) = match sandbox {
            SandboxType::None => (command, None),
            SandboxType::MacosSeatbelt => {
                #[cfg(target_os = "macos")]
                {
                    let policy = policy.ok_or(SandboxTransformError::UnsupportedPlatform)?;
                    let mut seatbelt_args =
                        create_seatbelt_command_args(command, policy, sandbox_policy_cwd);
                    let mut full = Vec::with_capacity(1 + seatbelt_args.len());
                    full.push(MACOS_PATH_TO_SEATBELT_EXECUTABLE.to_string());
                    full.append(&mut seatbelt_args);
                    (full, None)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return Err(SandboxTransformError::UnsupportedPlatform);
                }
            }
            SandboxType::LinuxBwrap => {
                #[cfg(target_os = "linux")]
                {
                    let policy = policy.ok_or(SandboxTransformError::UnsupportedPlatform)?;
                    let mut bwrap_args =
                        create_bwrap_command_args(command, policy, sandbox_policy_cwd)?;
                    let mut full = Vec::with_capacity(1 + bwrap_args.len());
                    full.push("bwrap".to_string());
                    full.append(&mut bwrap_args);
                    (full, None)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    return Err(SandboxTransformError::UnsupportedPlatform);
                }
            }
            SandboxType::LinuxHelper => {
                #[cfg(target_os = "linux")]
                {
                    let policy = policy.ok_or(SandboxTransformError::UnsupportedPlatform)?;
                    let linux_sandbox_exe = linux_sandbox_exe
                        .ok_or(SandboxTransformError::MissingLinuxSandboxExecutable)?;
                    let mut helper_args = create_linux_sandbox_helper_args(
                        command,
                        policy,
                        sandbox_policy_cwd,
                        use_linux_sandbox_bwrap,
                    )?;
                    let mut full = Vec::with_capacity(1 + helper_args.len());
                    full.push(linux_sandbox_exe.to_string_lossy().to_string());
                    full.append(&mut helper_args);
                    (full, Some("libra-linux-sandbox".to_string()))
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
            sandbox,
            sandbox_permissions: spec.sandbox_permissions,
            justification: spec.justification,
            arg0,
        })
    }
}

#[cfg(target_os = "linux")]
fn create_linux_sandbox_helper_args(
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

#[cfg(target_os = "macos")]
fn create_seatbelt_command_args(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    sandbox_policy_cwd: &Path,
) -> Vec<String> {
    const SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");
    const SEATBELT_NETWORK_POLICY: &str = include_str!("seatbelt_network_policy.sbpl");

    let (file_write_policy, file_write_params) =
        build_macos_file_write_policy(sandbox_policy, sandbox_policy_cwd);
    let file_read_policy = "; allow read-only file operations\n(allow file-read*)";
    let network_policy = if sandbox_policy.has_full_network_access() {
        SEATBELT_NETWORK_POLICY
    } else {
        ""
    };
    let full_policy =
        format!("{SEATBELT_BASE_POLICY}\n{file_read_policy}\n{file_write_policy}\n{network_policy}");

    let mut seatbelt_args = vec!["-p".to_string(), full_policy];
    let dir_params = [file_write_params, macos_dir_params()].concat();
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
fn create_bwrap_command_args(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> Result<Vec<String>, SandboxTransformError> {
    if sandbox_policy.has_full_disk_write_access() {
        return Ok(command);
    }

    let writable_roots = sandbox_policy.get_writable_roots_with_cwd(cwd);
    ensure_mount_targets_exist(&writable_roots)?;
    let allowed_write_paths: Vec<PathBuf> = writable_roots.iter().map(|wr| wr.root.clone()).collect();

    let mut args = vec![
        "--new-session".to_string(),
        "--die-with-parent".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
    ];

    for writable_root in &writable_roots {
        let root = writable_root.root.to_string_lossy().to_string();
        args.push("--bind".to_string());
        args.push(root.clone());
        args.push(root);
    }

    for subpath in collect_read_only_subpaths(&writable_roots) {
        if let Some(symlink_path) = find_symlink_in_path(&subpath, &allowed_write_paths) {
            args.push("--ro-bind".to_string());
            args.push("/dev/null".to_string());
            args.push(symlink_path.to_string_lossy().to_string());
            continue;
        }

        if !subpath.exists() {
            if let Some(first_missing) = find_first_nonexistent_component(&subpath)
                && is_within_allowed_write_paths(&first_missing, &allowed_write_paths)
            {
                args.push("--ro-bind".to_string());
                args.push("/dev/null".to_string());
                args.push(first_missing.to_string_lossy().to_string());
            }
            continue;
        }

        if is_within_allowed_write_paths(&subpath, &allowed_write_paths) {
            let path = subpath.to_string_lossy().to_string();
            args.push("--ro-bind".to_string());
            args.push(path.clone());
            args.push(path);
        }
    }

    if !sandbox_policy.has_full_network_access() {
        args.push("--unshare-net".to_string());
    }

    args.extend([
        "--unshare-pid".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--dev-bind".to_string(),
        "/dev/null".to_string(),
        "/dev/null".to_string(),
        "--".to_string(),
    ]);
    args.extend(command);
    Ok(args)
}

#[cfg(target_os = "linux")]
fn ensure_mount_targets_exist(writable_roots: &[WritableRoot]) -> Result<(), SandboxTransformError> {
    for writable_root in writable_roots {
        let root = writable_root.root.as_path();
        if !root.exists() {
            return Err(SandboxTransformError::MissingWritableRoot(root.to_path_buf()));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn collect_read_only_subpaths(writable_roots: &[WritableRoot]) -> Vec<PathBuf> {
    use std::collections::BTreeSet;

    let mut subpaths: BTreeSet<PathBuf> = BTreeSet::new();
    for writable_root in writable_roots {
        for subpath in &writable_root.read_only_subpaths {
            subpaths.insert(subpath.clone());
        }
    }
    subpaths.into_iter().collect()
}

#[cfg(target_os = "linux")]
fn is_within_allowed_write_paths(path: &Path, allowed_write_paths: &[PathBuf]) -> bool {
    allowed_write_paths.iter().any(|root| path.starts_with(root))
}

#[cfg(target_os = "linux")]
fn find_symlink_in_path(target_path: &Path, allowed_write_paths: &[PathBuf]) -> Option<PathBuf> {
    use std::path::Component;

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
            && is_within_allowed_write_paths(&current, allowed_write_paths)
        {
            return Some(current);
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn find_first_nonexistent_component(target_path: &Path) -> Option<PathBuf> {
    use std::path::Component;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_initial_uses_none_for_escalated_permissions() {
        let manager = SandboxManager::new();
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };

        assert_eq!(
            manager.select_initial(
                Some(&policy),
                SandboxPermissions::RequireEscalated,
                None
            ),
            SandboxType::None
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
}
