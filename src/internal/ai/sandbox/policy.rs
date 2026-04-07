use std::path::{Path, PathBuf};

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

    /// Returns writable roots resolved against the current working directory.
    /// Each writable root has protected subpaths (for example `.git`, `.libra`)
    /// that remain read-only.
    pub fn get_writable_roots_with_cwd(&self, cwd: &Path) -> Vec<WritableRoot> {
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
                    .into_iter()
                    .map(|root| WritableRoot {
                        read_only_subpaths: protected_subpaths(&root),
                        root,
                    })
                    .collect()
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
}
