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

    /// Every variant of [`SandboxEnforcement`] in declaration order
    /// (`Required`, `PreferStrict`, `BestEffort`). The fixed-length
    /// array makes the enumeration size part of the public API — a
    /// future fourth tier requires extending this list in the same
    /// patch, which forces the [`as_str`](Self::as_str) match arms,
    /// the [`FromStr`](std::str::FromStr) parser, and the
    /// [`SandboxEnforcementParseError`] expected-list error message
    /// to all be revisited.
    pub fn all() -> [Self; 3] {
        [Self::Required, Self::PreferStrict, Self::BestEffort]
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

/// Wire-protocol selector for a [`NetworkService`] allowlist entry.
///
/// Pre-positioned for Phase 7 (`docs/improvement/sandbox.md` §7.1) of the
/// sandbox network-three-state work. Until the full
/// `NetworkAccess::Allowlist { services }` migration lands this type is
/// only used by the new [`NetworkService`] schema and its validators
/// — it doesn't yet appear on the sandbox-policy wire envelope, but
/// shipping the schema early lets the `.libra/sandbox.toml` parser and
/// the future proxy stub be implemented against a stable contract.
///
/// `Tcp` is the default to match the sandbox.md spec
/// ("默认 tcp"); callers that need UDP-only allowlists (e.g. DNS, QUIC)
/// set `protocol = Some(NetworkProtocol::Udp)` on the service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkProtocol {
    /// TCP — the default for `https://`, `git://`, `ssh://` services.
    #[default]
    Tcp,
    /// UDP — used by DNS, QUIC, and proprietary peer-to-peer
    /// transports.
    Udp,
}

/// One entry in a sandbox network allowlist.
///
/// Pre-positioned for Phase 7 (`docs/improvement/sandbox.md` §7.1).
/// The shape matches the `.libra/sandbox.toml` `[[sandbox.network.services]]`
/// section:
///
/// ```toml
/// [[sandbox.network.services]]
/// host = "registry.npmjs.org"
/// ports = [443]
/// ```
///
/// Field semantics (mirrors sandbox.md §7.1):
/// - `host`: hostname or `*.subdomain` wildcard. Bare `"*"` (catch-all)
///   and the empty string are rejected by [`Self::validate`] because
///   they would silently turn an allowlist into a full-network grant.
/// - `ports`: empty = "every port allowed by the proxy". A non-empty
///   list restricts to the supplied ports. High-sensitivity ports
///   (22 / SSH, 3389 / RDP) are rejected by `validate` unless the
///   caller listed them explicitly — this catches a config that omits
///   `ports` for an entry whose hostname matches an SSH bastion, etc.
/// - `protocol`: `None` means "Tcp (the default)"; callers needing UDP
///   set `Some(NetworkProtocol::Udp)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetworkService {
    /// Hostname or `*.subdomain` wildcard; never bare `"*"` or empty.
    pub host: String,
    /// Allowed destination ports. Empty = any port on the host.
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Wire protocol; `None` = TCP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<NetworkProtocol>,
}

/// Validation error produced by [`NetworkService::validate`].
///
/// Each variant carries enough context to let
/// `.libra/sandbox.toml` parsers surface an actionable error to the
/// user without re-formatting the failure shape.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetworkServiceValidationError {
    /// `host` was the empty string. The allowlist parser must reject
    /// these because an empty host trivially matches nothing under
    /// the proxy and would otherwise be silently dropped.
    #[error("network service host must not be empty")]
    EmptyHost,
    /// `host` was the bare wildcard `"*"`. Treated as a config error
    /// because it turns an allowlist into a catch-all grant — the
    /// user almost certainly meant `NetworkAccess::Full`.
    #[error(
        "network service host must not be the bare wildcard '*'; use NetworkAccess::Full for a catch-all grant"
    )]
    BareWildcardHost,
    /// `ports` was empty but `host` matched a high-sensitivity port
    /// pattern (22 / SSH, 3389 / RDP). The validator demands those
    /// ports be listed explicitly so the user can't open SSH access
    /// by accidentally writing `{ host = "bastion.example.com" }`
    /// without `ports`.
    #[error(
        "network service '{host}' allows high-sensitivity port {port} via empty ports list; \
         list the ports explicitly to opt in"
    )]
    HighSensitivityPortRequiresExplicitList { host: String, port: u16 },
}

/// High-sensitivity ports that must NEVER be granted via an empty
/// `ports` list. Port 22 = SSH; port 3389 = RDP. The sandbox.md
/// spec at §7.1 line 336 mandates these be listed explicitly.
const HIGH_SENSITIVITY_PORTS: &[u16] = &[22, 3389];

impl NetworkService {
    /// Validate this service entry against the rules in
    /// `docs/improvement/sandbox.md` §7.1:
    ///
    /// - `host` must not be empty.
    /// - `host` must not be the bare wildcard `"*"`.
    /// - If `ports` is empty, the entry implicitly allows every port
    ///   — including the high-sensitivity SSH (22) / RDP (3389)
    ///   ports. The validator rejects the empty-ports form so the
    ///   user has to opt in explicitly.
    ///
    /// Returns `Ok(())` for a well-formed entry, or the matching
    /// [`NetworkServiceValidationError`] variant otherwise.
    pub fn validate(&self) -> Result<(), NetworkServiceValidationError> {
        if self.host.is_empty() {
            return Err(NetworkServiceValidationError::EmptyHost);
        }
        if self.host == "*" {
            return Err(NetworkServiceValidationError::BareWildcardHost);
        }
        if self.ports.is_empty()
            && let Some(&port) = HIGH_SENSITIVITY_PORTS.first()
        {
            // Empty `ports` means "any port", which silently includes
            // the high-sensitivity ports tracked in
            // [`HIGH_SENSITIVITY_PORTS`]. Force the caller to list
            // ports explicitly so an entry that omits `ports` for
            // (say) a hostname that resolves to an SSH bastion
            // can't open port 22 by accident. The error surfaces the
            // first sensitive port from the canonical list — that's
            // enough to point the user at the rule, and listing the
            // ports explicitly satisfies the validator regardless of
            // which sensitive port the host actually exposes.
            return Err(
                NetworkServiceValidationError::HighSensitivityPortRequiresExplicitList {
                    host: self.host.clone(),
                    port,
                },
            );
        }
        Ok(())
    }

    /// Effective protocol — `Tcp` when `protocol` is `None`. Avoids
    /// callers having to `unwrap_or(Tcp)` at every dispatch site once
    /// Phase 7.4's proxy starts routing per-service.
    pub fn effective_protocol(&self) -> NetworkProtocol {
        self.protocol.unwrap_or_default()
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

pub fn sensitive_read_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(home) = home {
        for relative in [
            ".ssh",
            ".aws",
            ".gnupg",
            ".netrc",
            ".azure",
            ".docker",
            ".npmrc",
            ".pypirc",
            ".cargo/credentials",
            ".cargo/credentials.toml",
            ".gem/credentials",
            ".config/gcloud",
            ".config/gh",
            ".config/hub",
            ".kube",
            ".config/libra/vault",
            ".mozilla/firefox",
            ".config/google-chrome",
            ".config/chromium",
            ".config/BraveSoftware/Brave-Browser",
            ".var/app/org.mozilla.firefox",
            "Library/Application Support/Google/Chrome",
            "Library/Application Support/Chromium",
            "Library/Application Support/BraveSoftware/Brave-Browser",
            "Library/Application Support/Firefox",
            "Library/Cookies",
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
        assert!(paths.contains(&PathBuf::from("/home/tester/.config/gh")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.docker")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.cargo/credentials.toml")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.config/google-chrome")));
        assert!(paths.contains(&PathBuf::from("/home/tester/.mozilla/firefox")));
        assert!(paths.contains(&PathBuf::from("/home/tester/Library/Cookies")));
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

    #[test]
    fn sandbox_policy_error_display_pins_dangerous_writable_root_template() {
        let err = SandboxPolicyError::DangerousWritableRoot {
            root: PathBuf::from("/etc"),
            reason: "is a system configuration directory",
        };
        assert_eq!(
            err.to_string(),
            "refusing writable_root '/etc' because is a system configuration directory; \
             choose a non-privileged project directory, expose the tool through a narrow \
             proxy, or rerun with explicit escalated permissions if host-level access is \
             intentional",
        );
    }

    /// `SandboxEnforcement::all()` enumerates every variant in
    /// declaration order, cross-checks each variant's `as_str()`
    /// against an exhaustive match (so a future fourth tier fails to
    /// compile here unless `all()` is also extended), and round-trips
    /// every canonical string through `FromStr`. Mirrors the
    /// v0.17.660+ `*::all()` + round-trip pattern.
    #[test]
    fn sandbox_enforcement_all_enumerates_every_variant_and_round_trips() {
        let variants = SandboxEnforcement::all();
        assert_eq!(variants.len(), 3);
        assert_eq!(
            variants,
            [
                SandboxEnforcement::Required,
                SandboxEnforcement::PreferStrict,
                SandboxEnforcement::BestEffort,
            ]
        );

        for variant in SandboxEnforcement::all() {
            let canonical = variant.as_str();
            let expected_canonical = match variant {
                SandboxEnforcement::Required => "required",
                SandboxEnforcement::PreferStrict => "prefer_strict",
                SandboxEnforcement::BestEffort => "best_effort",
            };
            assert_eq!(canonical, expected_canonical);

            let parsed: SandboxEnforcement = canonical
                .parse()
                .expect("canonical as_str() must round-trip through FromStr");
            assert_eq!(parsed, variant);
        }
    }

    /// `NetworkProtocol` must round-trip through serde as kebab-case
    /// (`"tcp"` / `"udp"`), default to `Tcp`, and `Hash` + `Eq` must
    /// hold so callers can index allowlists by protocol.
    #[test]
    fn network_protocol_serde_round_trip_and_defaults_to_tcp() {
        assert_eq!(NetworkProtocol::default(), NetworkProtocol::Tcp);
        for (variant, expected) in [
            (NetworkProtocol::Tcp, "\"tcp\""),
            (NetworkProtocol::Udp, "\"udp\""),
        ] {
            let serialised = serde_json::to_string(&variant).unwrap();
            assert_eq!(serialised, expected, "round-trip for {variant:?}");
            let back: NetworkProtocol = serde_json::from_str(&serialised).unwrap();
            assert_eq!(back, variant);
        }
    }

    /// `NetworkService::validate()` must reject the empty-host and
    /// bare-wildcard host shapes — both turn an allowlist into a
    /// silent grant. Pin the error variants explicitly so a future
    /// permissiveness in the validator fails the test rather than
    /// shipping an allowlist parser that accepts `host = ""`.
    #[test]
    fn network_service_validate_rejects_empty_and_bare_wildcard_hosts() {
        let empty = NetworkService {
            host: String::new(),
            ports: vec![443],
            protocol: None,
        };
        assert_eq!(
            empty.validate(),
            Err(NetworkServiceValidationError::EmptyHost),
        );

        let wildcard = NetworkService {
            host: "*".to_string(),
            ports: vec![443],
            protocol: None,
        };
        assert_eq!(
            wildcard.validate(),
            Err(NetworkServiceValidationError::BareWildcardHost),
        );
    }

    /// An empty `ports` list silently allows every destination port,
    /// which includes high-sensitivity ports (22 / SSH, 3389 / RDP).
    /// `validate()` must reject the empty-ports form so users have to
    /// opt in to those ports explicitly. Pin both the rejection AND
    /// the offending port surfaced in the error so a future relaxation
    /// of the list cannot drop SSH protection silently.
    #[test]
    fn network_service_validate_rejects_empty_ports_when_high_sensitivity_implied() {
        let no_ports = NetworkService {
            host: "bastion.example.com".to_string(),
            ports: vec![],
            protocol: None,
        };
        let err = no_ports
            .validate()
            .expect_err("empty ports must be rejected");
        match err {
            NetworkServiceValidationError::HighSensitivityPortRequiresExplicitList {
                host,
                port,
            } => {
                assert_eq!(host, "bastion.example.com");
                // Port 22 is the first high-sensitivity port returned
                // by the validator; the exact port asserted is part
                // of the rejection's diagnostic shape so the user can
                // see which gate fired.
                assert_eq!(port, 22);
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    /// Well-formed services pass validation: explicit hostname,
    /// non-empty ports list, protocol either set or defaulted.
    #[test]
    fn network_service_validate_accepts_well_formed_entries() {
        let https = NetworkService {
            host: "registry.npmjs.org".to_string(),
            ports: vec![443],
            protocol: None,
        };
        assert_eq!(https.validate(), Ok(()));
        assert_eq!(https.effective_protocol(), NetworkProtocol::Tcp);

        let ssh_explicit = NetworkService {
            host: "github.com".to_string(),
            ports: vec![22, 443],
            protocol: Some(NetworkProtocol::Tcp),
        };
        assert_eq!(ssh_explicit.validate(), Ok(()));

        let quic = NetworkService {
            host: "*.example.com".to_string(),
            ports: vec![443],
            protocol: Some(NetworkProtocol::Udp),
        };
        assert_eq!(quic.validate(), Ok(()));
        assert_eq!(quic.effective_protocol(), NetworkProtocol::Udp);
    }

    /// `NetworkService` must round-trip through serde with both the
    /// minimal form (`{host, ports}`, protocol omitted) and the
    /// fully-specified form. The minimal form is what
    /// `.libra/sandbox.toml` will produce; pin the parser-friendly
    /// shape so a future `serde(default)` change doesn't silently
    /// require `protocol` in the TOML.
    #[test]
    fn network_service_serde_round_trips_minimal_and_explicit_forms() {
        let minimal = NetworkService {
            host: "registry.npmjs.org".to_string(),
            ports: vec![443],
            protocol: None,
        };
        let serialised = serde_json::to_string(&minimal).unwrap();
        assert!(
            !serialised.contains("protocol"),
            "minimal form must skip protocol when None; got {serialised}",
        );
        let back: NetworkService = serde_json::from_str(&serialised).unwrap();
        assert_eq!(back, minimal);

        let explicit = NetworkService {
            host: "*.example.com".to_string(),
            ports: vec![443],
            protocol: Some(NetworkProtocol::Udp),
        };
        let serialised = serde_json::to_string(&explicit).unwrap();
        let back: NetworkService = serde_json::from_str(&serialised).unwrap();
        assert_eq!(back, explicit);
    }
}
