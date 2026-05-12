//! Policy model for constraining AI tool execution inside a workspace sandbox.
//!
//! Boundary: policy parsing is conservative and treats missing or ambiguous allowlists
//! as denied operations. Hardening contract tests cover path traversal, shell command,
//! and workspace-scope boundaries.

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Controls whether command execution uses the configured sandbox policy
/// or bypasses it for an escalated run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPermissions {
    #[default]
    UseDefault,
    RequireEscalated,
}

impl SandboxPermissions {
    pub fn requires_escalated_permissions(self) -> bool {
        matches!(self, Self::RequireEscalated)
    }
}

/// Controls how strongly Libra requires an OS sandbox backend to be active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxEnforcement {
    Required,
    PreferStrict,
    #[default]
    BestEffort,
}

impl SandboxEnforcement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::PreferStrict => "prefer_strict",
            Self::BestEffort => "best_effort",
        }
    }

    pub fn requires_effective_sandbox(self) -> bool {
        matches!(self, Self::Required)
    }
}

impl std::str::FromStr for SandboxEnforcement {
    type Err = SandboxEnforcementParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "required" => Ok(Self::Required),
            "prefer_strict" | "prefer-strict" => Ok(Self::PreferStrict),
            "best_effort" | "best-effort" => Ok(Self::BestEffort),
            _ => Err(SandboxEnforcementParseError {
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "invalid sandbox enforcement '{value}'; expected one of: required, prefer_strict, best_effort"
)]
pub struct SandboxEnforcementParseError {
    value: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkAccess {
    #[default]
    Restricted,
    Enabled,
}

impl NetworkAccess {
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

pub fn sensitive_read_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(home) = home {
        for relative in [
            ".ssh",
            ".aws",
            ".gnupg",
            ".netrc",
            ".config/gcloud",
            ".kube",
            ".config/libra/vault",
        ] {
            paths.push(home.join(relative));
        }
    }

    paths.push(PathBuf::from("/etc/shadow"));
    paths
}

/// Runtime sandbox policy for shell-like tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SandboxPolicy {
    DangerFullAccess,
    ReadOnly,
    ExternalSandbox {
        #[serde(default)]
        network_access: NetworkAccess,
    },
    WorkspaceWrite {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        writable_roots: Vec<PathBuf>,
        #[serde(default)]
        network_access: bool,
        #[serde(default)]
        exclude_tmpdir_env_var: bool,
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SandboxPolicyError {
    #[error(
        "refusing writable_root '{root}' because {reason}; choose a non-privileged project directory, expose the tool through a narrow proxy, or rerun with explicit escalated permissions if host-level access is intentional"
    )]
    DangerousWritableRoot { root: PathBuf, reason: &'static str },
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self::WorkspaceWrite {
            writable_roots: vec![],
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WritableRoot {
    pub root: PathBuf,
    pub read_only_subpaths: Vec<PathBuf>,
}

impl WritableRoot {
    pub fn is_path_writable(&self, path: &Path) -> bool {
        if !path.starts_with(&self.root) {
            return false;
        }
        !self
            .read_only_subpaths
            .iter()
            .any(|subpath| path.starts_with(subpath))
    }
}

impl SandboxPolicy {
    pub fn new_read_only_policy() -> Self {
        Self::ReadOnly
    }

    pub fn new_workspace_write_policy() -> Self {
        Self::default()
    }

    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(self, Self::DangerFullAccess | Self::ExternalSandbox { .. })
    }

    pub fn has_full_network_access(&self) -> bool {
        match self {
            Self::DangerFullAccess => true,
            Self::ReadOnly => false,
            Self::ExternalSandbox { network_access } => network_access.is_enabled(),
            Self::WorkspaceWrite { network_access, .. } => *network_access,
        }
    }

    pub fn validate_writable_roots_with_cwd(&self, cwd: &Path) -> Result<(), SandboxPolicyError> {
        for root in self.writable_root_paths_with_cwd(cwd) {
            validate_writable_root(&root)?;
        }
        Ok(())
    }

    /// Returns writable roots resolved against the current working directory.
    /// Each writable root has protected subpaths (for example `.git`, `.libra`)
    /// that remain read-only.
    pub fn get_writable_roots_with_cwd(&self, cwd: &Path) -> Vec<WritableRoot> {
        self.writable_root_paths_with_cwd(cwd)
            .into_iter()
            .map(|root| WritableRoot {
                read_only_subpaths: protected_subpaths(&root),
                root,
            })
            .collect()
    }

    fn writable_root_paths_with_cwd(&self, cwd: &Path) -> Vec<PathBuf> {
        match self {
            Self::DangerFullAccess | Self::ExternalSandbox { .. } | Self::ReadOnly => Vec::new(),
            Self::WorkspaceWrite {
                writable_roots,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
                network_access: _,
            } => {
                let mut roots: Vec<PathBuf> = Vec::new();

                for root in writable_roots {
                    push_root_unique(&mut roots, resolve_root(root, cwd));
                }

                if roots.is_empty() {
                    push_root_unique(&mut roots, cwd.to_path_buf());
                }

                if cfg!(unix) && !exclude_slash_tmp {
                    let slash_tmp = PathBuf::from("/tmp");
                    if slash_tmp.is_dir() {
                        push_root_unique(&mut roots, slash_tmp);
                    }
                }

                if !exclude_tmpdir_env_var && let Some(tmpdir) = std::env::var_os("TMPDIR") {
                    let tmpdir_path = PathBuf::from(tmpdir);
                    if tmpdir_path.is_absolute() && tmpdir_path.is_dir() {
                        push_root_unique(&mut roots, tmpdir_path);
                    }
                }

                roots
            }
        }
    }
}

fn resolve_root(root: &Path, cwd: &Path) -> PathBuf {
    if root.is_absolute() {
        root.to_path_buf()
    } else {
        cwd.join(root)
    }
}

fn push_root_unique(roots: &mut Vec<PathBuf>, root: PathBuf) {
    let normalized = root.canonicalize().unwrap_or(root);
    if roots.iter().any(|existing| existing == &normalized) {
        return;
    }
    roots.push(normalized);
}

fn validate_writable_root(root: &Path) -> Result<(), SandboxPolicyError> {
    let lexical = normalize_path_lexically(root);
    if let Some(reason) = dangerous_writable_root_reason(&lexical) {
        return Err(SandboxPolicyError::DangerousWritableRoot {
            root: lexical,
            reason,
        });
    }

    if let Ok(canonical) = root.canonicalize() {
        let canonical = normalize_path_lexically(&canonical);
        if let Some(reason) = dangerous_writable_root_reason(&canonical) {
            return Err(SandboxPolicyError::DangerousWritableRoot {
                root: canonical,
                reason,
            });
        }
    }

    Ok(())
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn dangerous_writable_root_reason(path: &Path) -> Option<&'static str> {
    if path == Path::new("/") {
        return Some("it would make the whole host filesystem writable from the sandbox");
    }
    for sensitive_root in ["/proc", "/sys", "/dev"] {
        if path == Path::new(sensitive_root) || path.starts_with(sensitive_root) {
            return Some("kernel and device files can be used to escape or weaken the sandbox");
        }
    }
    if path.file_name() == Some(OsStr::new("docker.sock")) {
        return Some("Docker socket access is equivalent to host-level container control");
    }
    if path.file_name() == Some(OsStr::new("containerd.sock")) {
        return Some("containerd socket access is equivalent to host-level container control");
    }
    if path == Path::new("/run/containerd/containerd.sock")
        || path == Path::new("/var/run/containerd/containerd.sock")
    {
        return Some("containerd socket access is equivalent to host-level container control");
    }
    if path.starts_with("/var/run/libvirt") || path.starts_with("/run/libvirt") {
        return Some("libvirt control sockets can start privileged host resources");
    }
    None
}

fn protected_subpaths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for subdir in [".git", ".libra", ".codex", ".agents"] {
        paths.push(root.join(subdir));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_enforcement_accepts_stable_spellings() {
        assert_eq!(
            "required".parse::<SandboxEnforcement>(),
            Ok(SandboxEnforcement::Required)
        );
        assert_eq!(
            "prefer-strict".parse::<SandboxEnforcement>(),
            Ok(SandboxEnforcement::PreferStrict)
        );
        assert_eq!(
            "best_effort".parse::<SandboxEnforcement>(),
            Ok(SandboxEnforcement::BestEffort)
        );
    }

    #[test]
    fn sandbox_enforcement_rejects_unknown_values() {
        let error = "strict"
            .parse::<SandboxEnforcement>()
            .expect_err("unsupported enforcement names must be rejected");

        assert_eq!(
            error.to_string(),
            "invalid sandbox enforcement 'strict'; expected one of: required, prefer_strict, best_effort"
        );
    }

    #[test]
    fn sensitive_read_paths_include_home_credentials_and_system_shadow() {
        let paths = sensitive_read_paths(Some(Path::new("/home/tester")));

        assert!(paths.contains(&PathBuf::from("/home/tester/.ssh")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.aws")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.netrc")));
        assert!(paths.contains(&PathBuf::from("/etc/shadow")));
    }

    #[test]
    fn explicit_workspace_roots_do_not_expand_to_cwd() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("src/main.rs")],
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        let roots = policy.get_writable_roots_with_cwd(Path::new("/tmp/workspace"));

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].root, PathBuf::from("/tmp/workspace/src/main.rs"));
    }

    #[test]
    fn empty_workspace_roots_fall_back_to_cwd() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        let roots = policy.get_writable_roots_with_cwd(Path::new("/tmp/workspace"));

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].root, PathBuf::from("/tmp/workspace"));
    }

    #[test]
    fn dangerous_socket_writable_roots_are_rejected() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![
                PathBuf::from("/var/run/docker.sock"),
                PathBuf::from("/tmp/project"),
            ],
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        let error = policy
            .validate_writable_roots_with_cwd(Path::new("/tmp/workspace"))
            .expect_err("docker socket writable roots must be rejected");

        assert!(error.to_string().contains("Docker socket access"));
    }

    #[test]
    fn nested_docker_socket_roots_are_rejected() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("tools/docker.sock")],
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        let error = policy
            .validate_writable_roots_with_cwd(Path::new("/tmp/workspace"))
            .expect_err("glob-style docker.sock writable roots must be rejected");

        assert!(error.to_string().contains("Docker socket access"));
    }

    #[test]
    fn kernel_and_device_writable_roots_are_rejected() {
        for root in ["/", "/proc", "/proc/self", "/sys", "/dev", "/dev/null"] {
            let policy = SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![PathBuf::from(root)],
                network_access: false,
                exclude_tmpdir_env_var: true,
                exclude_slash_tmp: true,
            };

            assert!(
                policy
                    .validate_writable_roots_with_cwd(Path::new("/tmp/workspace"))
                    .is_err(),
                "{root} must not be accepted as a writable sandbox root",
            );
        }
    }

    #[test]
    fn safe_workspace_writable_roots_are_accepted() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("src")],
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        policy
            .validate_writable_roots_with_cwd(Path::new("/tmp/workspace"))
            .expect("ordinary workspace roots should be accepted");
    }
}
