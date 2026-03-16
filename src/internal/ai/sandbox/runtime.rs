use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{SandboxPermissions, SandboxPolicy};

pub const LIBRA_SANDBOX_NETWORK_DISABLED_ENV_VAR: &str = "LIBRA_SANDBOX_NETWORK_DISABLED";
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
    #[error("failed to serialize sandbox policy for linux sandbox: {0}")]
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
        } = request;

        #[cfg(not(target_os = "linux"))]
        let _ = use_linux_sandbox_bwrap;
        #[cfg(not(target_os = "linux"))]
        let _ = linux_sandbox_exe;

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

        let sandbox = self.select_initial(policy, spec.sandbox_permissions);
        let (command, arg0, effective_sandbox) = match sandbox {
            SandboxType::None => (command, None, SandboxType::None),
            SandboxType::MacosSeatbelt => {
                #[cfg(target_os = "macos")]
                {
                    let policy = policy.ok_or(SandboxTransformError::UnsupportedPlatform)?;
                    let mut seatbelt_args =
                        create_seatbelt_command_args(command, policy, sandbox_policy_cwd);
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
                    } else {
                        tracing::warn!(
                            "linux sandbox executable not configured; running command without linux sandbox"
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
        })
    }
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
    let full_policy = format!(
        "{SEATBELT_BASE_POLICY}\n{file_read_policy}\n{file_write_policy}\n{network_policy}"
    );

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
            network_access: false,
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
            network_access: super::super::NetworkAccess::Restricted,
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
            network_access: false,
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
                network_access: false,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }),
            sandbox_policy_cwd: &cwd,
            linux_sandbox_exe: None,
            use_linux_sandbox_bwrap: false,
        };

        let transformed = manager
            .transform(request)
            .expect("transform should fallback");
        assert_eq!(transformed.sandbox, SandboxType::None);
        assert!(!transformed.command.is_empty());
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
