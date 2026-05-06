//! Sandbox subsystem for AI tool calls.
//!
//! Boundary: exposes policy parsing, command-safety checks, and runtime enforcement;
//! it does not decide workflow phase state. AI hardening contract tests exercise the
//! public guarantees of this module.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use ring::digest::{SHA256, digest};
const DEFAULT_APPROVAL_SCOPE: &str = "interactive";
pub const DEFAULT_APPROVAL_TTL: Duration = Duration::from_secs(300);

use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, mpsc::UnboundedSender, oneshot},
};

use super::runtime::hardening::{SafetyDecision, SafetyDisposition};

mod command_safety;
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
    pub file_history: Option<FileHistoryRuntimeContext>,
    pub max_output_bytes: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct SandboxRuntimeConfig {
    pub linux_sandbox_exe: Option<PathBuf>,
    pub use_linux_sandbox_bwrap: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHistoryRuntimeContext {
    pub session_root: PathBuf,
    pub batch_id: String,
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
    ApprovedForTtl,
    ApprovedForDirectoryTtl,
    ApprovedForPatternTtl,
    ApprovedForAllCommands,
    #[default]
    Denied,
    Abort,
}

impl ReviewDecision {
    fn is_approved(self) -> bool {
        matches!(
            self,
            Self::Approved
                | Self::ApprovedForSession
                | Self::ApprovedForTtl
                | Self::ApprovedForDirectoryTtl
                | Self::ApprovedForPatternTtl
                | Self::ApprovedForAllCommands
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalSensitivityTier {
    Strict,
    Directory,
    Pattern,
}

impl ApprovalSensitivityTier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Directory => "directory",
            Self::Pattern => "pattern",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalScope {
    Session,
    Project,
    User,
}

impl ApprovalScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalCachePolicy {
    pub protected_branches: Vec<String>,
    pub allowed_network_domains: Vec<String>,
    pub no_cache_unknown_network: bool,
}

impl Default for ApprovalCachePolicy {
    fn default() -> Self {
        Self {
            protected_branches: vec![
                "main".to_string(),
                "master".to_string(),
                "trunk".to_string(),
                "develop".to_string(),
                "release/*".to_string(),
            ],
            allowed_network_domains: Vec::new(),
            no_cache_unknown_network: false,
        }
    }
}

impl ApprovalCachePolicy {
    fn disabled_reason_for_command(&self, command: &str) -> Option<String> {
        if let Some(branch) = protected_branch_in_command(command, &self.protected_branches) {
            return Some(format!(
                "approval cache disabled because command references protected branch `{branch}`"
            ));
        }

        if let Some(domain) = non_allowlisted_network_domain(
            command,
            &self.allowed_network_domains,
            self.no_cache_unknown_network,
        ) {
            return Some(format!(
                "approval cache disabled because command references non-allowlisted domain `{domain}`"
            ));
        }

        None
    }
}

fn protected_branch_in_command(command: &str, protected_branches: &[String]) -> Option<String> {
    if protected_branches.is_empty() {
        return None;
    }
    let parts = shell_words(command);
    protected_branches
        .iter()
        .map(|branch| branch.trim())
        .filter(|branch| !branch.is_empty())
        .find(|branch| {
            parts.iter().any(|part| {
                protected_branch_pattern_matches(part, branch)
                    || part
                        .strip_prefix("origin/")
                        .is_some_and(|short| protected_branch_pattern_matches(short, branch))
                    || part
                        .strip_prefix("refs/heads/")
                        .is_some_and(|short| protected_branch_pattern_matches(short, branch))
            })
        })
        .map(ToString::to_string)
}

fn protected_branch_pattern_matches(part: &str, branch: &str) -> bool {
    if let Some(prefix) = branch.strip_suffix("/*") {
        return part
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'));
    }
    part == branch
}

fn non_allowlisted_network_domain(
    command: &str,
    allowed_network_domains: &[String],
    no_cache_unknown_network: bool,
) -> Option<String> {
    let domains = extract_network_domains(command);
    if domains.is_empty() {
        return None;
    }
    if allowed_network_domains.is_empty() && !no_cache_unknown_network {
        return None;
    }
    domains.into_iter().find(|domain| {
        !allowed_network_domains
            .iter()
            .any(|allowed| domain_matches_allowed(domain, allowed))
    })
}

fn extract_network_domains(command: &str) -> Vec<String> {
    shell_words(command)
        .into_iter()
        .filter_map(|part| network_domain_from_token(&part))
        .collect()
}

fn network_domain_from_token(token: &str) -> Option<String> {
    let has_scheme = token.contains("://");
    let host_port_path = if let Some((_, rest)) = token.split_once("://") {
        rest.split('@').next_back().unwrap_or(rest)
    } else {
        token
    };
    let host = host_port_path
        .trim_start_matches('[')
        .split(['/', ':', '?', '#', ']'])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    let is_ascii_domain = host.contains('.')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.'))
        && !host.starts_with('-')
        && !host.ends_with('-');
    if is_ascii_domain {
        return Some(host.to_ascii_lowercase());
    }
    // A scheme-qualified URL with a non-ASCII / IDN host shouldn't silently
    // bypass network policy — return a sentinel that never matches an
    // allowlist entry so `no_cache_unknown_network` and the allowlist gate
    // both treat the request as untrusted. Bare tokens without a scheme
    // (paths, args) still fall through to None.
    if has_scheme && host.contains('.') {
        Some(format!("__non_ascii__:{host}"))
    } else {
        None
    }
}

fn domain_matches_allowed(domain: &str, allowed: &str) -> bool {
    let allowed = allowed.trim().trim_end_matches('.').to_ascii_lowercase();
    !allowed.is_empty() && (domain == allowed || domain.ends_with(&format!(".{allowed}")))
}

fn shell_words(command: &str) -> Vec<String> {
    shlex::split(command)
        .filter(|parts| !parts.is_empty())
        .unwrap_or_else(|| {
            command
                .split_whitespace()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalMemo {
    pub key: String,
    pub decision: ReviewDecision,
    pub expires_at: Option<DateTime<Utc>>,
    pub scope: ApprovalScope,
    pub sensitivity_tier: ApprovalSensitivityTier,
}

impl ApprovalMemo {
    fn session(
        key: String,
        decision: ReviewDecision,
        scope: ApprovalScope,
        sensitivity_tier: ApprovalSensitivityTier,
    ) -> Self {
        Self {
            key,
            decision,
            expires_at: None,
            scope,
            sensitivity_tier,
        }
    }

    fn ttl(
        key: String,
        decision: ReviewDecision,
        scope: ApprovalScope,
        sensitivity_tier: ApprovalSensitivityTier,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> Self {
        // A pathological caller could pass a TTL that overflows
        // `chrono::Duration` or `now + ttl`. Without a fallback, `expires_at`
        // would be `None`, which `is_active_at` treats as "never expires" —
        // silently turning a TTL memo into a session-permanent one. Substitute
        // a 7-day cap on the overflow paths so the memo still expires; honest
        // in-range TTLs flow through unchanged.
        const OVERFLOW_FALLBACK_HOURS: i64 = 24 * 7;
        let fallback = chrono::Duration::hours(OVERFLOW_FALLBACK_HOURS);
        let bounded = chrono::Duration::from_std(ttl).unwrap_or(fallback);
        let expires_at = now
            .checked_add_signed(bounded)
            .or_else(|| now.checked_add_signed(fallback));
        Self {
            key,
            decision,
            expires_at,
            scope,
            sensitivity_tier,
        }
    }

    fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|expires_at| expires_at > now)
    }
}

#[derive(Debug, Default)]
pub struct ApprovalStore {
    map: HashMap<String, ApprovalMemo>,
    allow_all_commands_scopes: HashSet<String>,
}

impl ApprovalStore {
    pub fn get(&self, key: &str) -> Option<ReviewDecision> {
        self.get_at(key, Utc::now())
    }

    pub fn get_at(&self, key: &str, now: DateTime<Utc>) -> Option<ReviewDecision> {
        self.map
            .get(key)
            .filter(|memo| memo.is_active_at(now))
            .map(|memo| memo.decision)
    }

    pub fn put(&mut self, key: String, value: ReviewDecision) {
        if !matches!(value, ReviewDecision::ApprovedForSession) {
            return;
        }
        self.map.insert(
            key.clone(),
            ApprovalMemo::session(
                key,
                value,
                ApprovalScope::Session,
                ApprovalSensitivityTier::Strict,
            ),
        );
    }

    pub fn put_ttl(
        &mut self,
        key: String,
        value: ReviewDecision,
        scope: ApprovalScope,
        sensitivity_tier: ApprovalSensitivityTier,
        now: DateTime<Utc>,
        ttl: Duration,
    ) {
        if !matches!(value, ReviewDecision::ApprovedForTtl) {
            return;
        }
        self.map.insert(
            key.clone(),
            ApprovalMemo::ttl(key, value, scope, sensitivity_tier, now, ttl),
        );
    }

    pub fn revoke(&mut self, key: &str) -> bool {
        self.map.remove(key).is_some()
    }

    /// Drop the broad "approve every command in this scope" decision so the
    /// next matching command falls back to the regular prompt path. Returns
    /// `true` if a record was removed. Without this, an `ApprovedForAllCommands`
    /// answer would persist for the rest of the session with no revoke surface.
    pub fn revoke_allow_all_for_scope(&mut self, scope: &str) -> bool {
        self.allow_all_commands_scopes
            .remove(normalized_approval_scope(scope).as_str())
    }

    /// Snapshot of every active scope that currently has the allow-all
    /// decision recorded. Sorted for deterministic UI output.
    pub fn active_allow_all_scopes(&self) -> Vec<String> {
        let mut scopes: Vec<String> = self.allow_all_commands_scopes.iter().cloned().collect();
        scopes.sort();
        scopes
    }

    pub fn active_memos_at(&self, now: DateTime<Utc>) -> Vec<ApprovalMemo> {
        let mut memos = self
            .map
            .values()
            .filter(|memo| memo.is_active_at(now))
            .cloned()
            .collect::<Vec<_>>();
        memos.sort_by(|a, b| a.key.cmp(&b.key));
        memos
    }

    pub fn allow_all_commands(&self) -> bool {
        self.allow_all_commands_for_scope(DEFAULT_APPROVAL_SCOPE)
    }

    pub fn approve_all_commands(&mut self) {
        self.approve_all_commands_for_scope(DEFAULT_APPROVAL_SCOPE);
    }

    pub fn allow_all_commands_for_scope(&self, scope: &str) -> bool {
        self.allow_all_commands_scopes
            .contains(normalized_approval_scope(scope).as_str())
    }

    pub fn approve_all_commands_for_scope(&mut self, scope: &str) {
        self.allow_all_commands_scopes
            .insert(normalized_approval_scope(scope));
    }
}

pub async fn request_cached_approval_with_keys<F>(
    ctx: &ToolApprovalContext,
    keys: &[String],
    build_request: F,
) -> ReviewDecision
where
    F: FnOnce(oneshot::Sender<ReviewDecision>) -> ExecApprovalRequest,
{
    request_cached_approval_with_cache_keys(
        ctx,
        ApprovalCacheKeys::strict(keys.to_vec()),
        None,
        |response_tx, _cache_disabled_reason| build_request(response_tx),
    )
    .await
}

async fn request_cached_approval_with_cache_keys<F>(
    ctx: &ToolApprovalContext,
    keys: ApprovalCacheKeys,
    cache_disabled_reason: Option<String>,
    build_request: F,
) -> ReviewDecision
where
    F: FnOnce(oneshot::Sender<ReviewDecision>, Option<String>) -> ExecApprovalRequest,
{
    let scope = ctx
        .scope_key_prefix
        .as_deref()
        .unwrap_or(DEFAULT_APPROVAL_SCOPE);
    let scoped_keys = keys.scoped(scope);
    if cache_disabled_reason.is_none() {
        let store = ctx.store.lock().await;
        if store.allow_all_commands_for_scope(scope) {
            tracing::debug!(
                target: "libra::internal::ai::sandbox",
                key_count = scoped_keys.lookup.len(),
                approval_scope = scope,
                "approval request skipped by allow-all-commands session decision"
            );
            return ReviewDecision::ApprovedForAllCommands;
        }
    }

    let cached_decision = if cache_disabled_reason.is_some() || scoped_keys.lookup.is_empty() {
        None
    } else {
        let store = ctx.store.lock().await;
        if scoped_keys.require_all_lookup {
            cached_approval_decision(&store, &scoped_keys.lookup)
        } else {
            cached_any_approval_decision(&store, &scoped_keys.lookup)
        }
    };
    if let Some(decision) = cached_decision {
        tracing::debug!(
            target: "libra::internal::ai::sandbox",
            key_count = scoped_keys.lookup.len(),
            approval_scope = scope,
            decision = ?decision,
            "approval request skipped by matching cached approval"
        );
        return decision;
    }

    let (response_tx, response_rx) = oneshot::channel();
    let request = build_request(response_tx, cache_disabled_reason.clone());
    if ctx.request_tx.send(request).is_err() {
        return ReviewDecision::Denied;
    }

    let decision = response_rx.await.unwrap_or_default();
    if cache_disabled_reason.is_some() {
        return if decision.is_approved() {
            ReviewDecision::Approved
        } else {
            decision
        };
    }

    if matches!(decision, ReviewDecision::ApprovedForAllCommands) {
        let mut store = ctx.store.lock().await;
        store.approve_all_commands_for_scope(scope);
        tracing::debug!(
            target: "libra::internal::ai::sandbox",
            approval_scope = scope,
            "approval decision cached as allow-all-commands for this session"
        );
    } else if matches!(decision, ReviewDecision::ApprovedForSession)
        && !scoped_keys.strict.is_empty()
    {
        let mut store = ctx.store.lock().await;
        for key in &scoped_keys.strict {
            store.put(key.clone(), ReviewDecision::ApprovedForSession);
        }
        tracing::debug!(
            target: "libra::internal::ai::sandbox",
            key_count = scoped_keys.strict.len(),
            approval_scope = scope,
            "approval decision cached for matching commands"
        );
    } else if matches!(decision, ReviewDecision::ApprovedForTtl) && !scoped_keys.strict.is_empty() {
        let mut store = ctx.store.lock().await;
        let now = Utc::now();
        for key in &scoped_keys.strict {
            store.put_ttl(
                key.clone(),
                ReviewDecision::ApprovedForTtl,
                ApprovalScope::Session,
                ApprovalSensitivityTier::Strict,
                now,
                ctx.approval_ttl,
            );
        }
        tracing::debug!(
            target: "libra::internal::ai::sandbox",
            key_count = scoped_keys.strict.len(),
            approval_scope = scope,
            ttl_secs = ctx.approval_ttl.as_secs(),
            "approval decision cached with ttl for matching commands"
        );
    } else if matches!(decision, ReviewDecision::ApprovedForDirectoryTtl)
        && !scoped_keys.directory_ttl.is_empty()
    {
        let mut store = ctx.store.lock().await;
        let now = Utc::now();
        for key in &scoped_keys.directory_ttl {
            store.put_ttl(
                key.clone(),
                ReviewDecision::ApprovedForTtl,
                ApprovalScope::Session,
                ApprovalSensitivityTier::Directory,
                now,
                ctx.approval_ttl,
            );
        }
        tracing::debug!(
            target: "libra::internal::ai::sandbox",
            key_count = scoped_keys.directory_ttl.len(),
            approval_scope = scope,
            ttl_secs = ctx.approval_ttl.as_secs(),
            "approval decision cached with directory ttl for matching commands"
        );
    } else if matches!(decision, ReviewDecision::ApprovedForPatternTtl)
        && !scoped_keys.pattern_ttl.is_empty()
    {
        let mut store = ctx.store.lock().await;
        let now = Utc::now();
        for key in &scoped_keys.pattern_ttl {
            store.put_ttl(
                key.clone(),
                ReviewDecision::ApprovedForTtl,
                ApprovalScope::Session,
                ApprovalSensitivityTier::Pattern,
                now,
                ctx.approval_ttl,
            );
        }
        tracing::debug!(
            target: "libra::internal::ai::sandbox",
            key_count = scoped_keys.pattern_ttl.len(),
            approval_scope = scope,
            ttl_secs = ctx.approval_ttl.as_secs(),
            "approval decision cached with pattern ttl for matching commands"
        );
    }
    decision
}

fn cached_approval_decision(
    store: &ApprovalStore,
    scoped_keys: &[String],
) -> Option<ReviewDecision> {
    let mut saw_ttl = false;
    for key in scoped_keys {
        match store.get(key) {
            Some(ReviewDecision::ApprovedForSession) => {}
            Some(ReviewDecision::ApprovedForTtl) => saw_ttl = true,
            _ => return None,
        }
    }

    if saw_ttl {
        Some(ReviewDecision::ApprovedForTtl)
    } else {
        Some(ReviewDecision::ApprovedForSession)
    }
}

fn cached_any_approval_decision(
    store: &ApprovalStore,
    scoped_keys: &[String],
) -> Option<ReviewDecision> {
    let mut saw_ttl = false;
    for key in scoped_keys {
        match store.get(key) {
            Some(ReviewDecision::ApprovedForSession) => {
                return Some(ReviewDecision::ApprovedForSession);
            }
            Some(ReviewDecision::ApprovedForTtl) => saw_ttl = true,
            _ => {}
        }
    }
    saw_ttl.then_some(ReviewDecision::ApprovedForTtl)
}

fn normalized_approval_scope(scope: &str) -> String {
    let trimmed = scope.trim();
    if trimmed.is_empty() {
        DEFAULT_APPROVAL_SCOPE.to_string()
    } else {
        trimmed.to_string()
    }
}

fn scoped_approval_keys(scope: &str, keys: &[String]) -> Vec<String> {
    let scope = normalized_approval_scope(scope);
    if scope == DEFAULT_APPROVAL_SCOPE {
        return keys.to_vec();
    }
    keys.iter()
        .map(|key| format!("{scope}:{key}"))
        .collect::<Vec<_>>()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ApprovalCacheKeys {
    lookup: Vec<String>,
    strict: Vec<String>,
    directory_ttl: Vec<String>,
    pattern_ttl: Vec<String>,
    require_all_lookup: bool,
}

impl ApprovalCacheKeys {
    fn strict(keys: Vec<String>) -> Self {
        Self {
            lookup: keys.clone(),
            strict: keys,
            directory_ttl: Vec::new(),
            pattern_ttl: Vec::new(),
            require_all_lookup: true,
        }
    }

    fn shell(command: &str, cwd: &Path, sandbox_permissions: SandboxPermissions) -> Self {
        let strict = shell_approval_key_with_scope(
            command,
            cwd,
            sandbox_permissions,
            ApprovalScope::Session,
            ApprovalSensitivityTier::Strict,
        );
        let directory = shell_approval_key_with_scope(
            command,
            cwd,
            sandbox_permissions,
            ApprovalScope::Session,
            ApprovalSensitivityTier::Directory,
        );
        let pattern = shell_approval_key_with_scope(
            command,
            cwd,
            sandbox_permissions,
            ApprovalScope::Session,
            ApprovalSensitivityTier::Pattern,
        );
        Self {
            lookup: vec![strict.clone(), directory.clone(), pattern.clone()],
            strict: vec![strict],
            directory_ttl: vec![directory],
            pattern_ttl: vec![pattern],
            require_all_lookup: false,
        }
    }

    fn scoped(&self, scope: &str) -> Self {
        Self {
            lookup: scoped_approval_keys(scope, &self.lookup),
            strict: scoped_approval_keys(scope, &self.strict),
            directory_ttl: scoped_approval_keys(scope, &self.directory_ttl),
            pattern_ttl: scoped_approval_keys(scope, &self.pattern_ttl),
            require_all_lookup: self.require_all_lookup,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolApprovalContext {
    pub policy: AskForApproval,
    pub request_tx: UnboundedSender<ExecApprovalRequest>,
    pub store: Arc<Mutex<ApprovalStore>>,
    pub scope_key_prefix: Option<String>,
    pub approval_ttl: Duration,
    pub cache_policy: ApprovalCachePolicy,
}

pub struct ExecApprovalRequest {
    pub call_id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub reason: Option<String>,
    pub is_retry: bool,
    pub sandbox_label: String,
    pub network_access: bool,
    pub writable_roots: Vec<PathBuf>,
    pub cache_disabled_reason: Option<String>,
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
            .field("sandbox_label", &self.sandbox_label)
            .field("network_access", &self.network_access)
            .field("writable_roots", &self.writable_roots)
            .field("cache_disabled_reason", &self.cache_disabled_reason)
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
    pub safety_decision: Option<SafetyDecision>,
}

#[derive(Default, Clone)]
struct StreamState {
    bytes: Vec<u8>,
    truncated: bool,
}

const DEFAULT_TIMEOUT_MS: u64 = 60_000;
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
const QUICK_REJECT_EXIT_CODES: [i32; 3] = [129, 126, 127];

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
        safety_decision,
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
            shell_exec_approval_requirement(
                ctx.policy,
                sandbox.as_ref().map(|s| &s.policy),
                &command,
                spec.sandbox_permissions,
                safety_decision.as_ref(),
            )
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
                    ExecApprovalPrompt {
                        call_id: &call_id,
                        command: &command,
                        cwd: &cwd,
                        reason: reason.clone().or_else(|| {
                            justification
                                .as_deref()
                                .map(str::trim)
                                .filter(|text| !text.is_empty())
                                .map(ToString::to_string)
                        }),
                        sandbox_policy: sandbox.as_ref().map(|s| &s.policy),
                        sandbox_permissions: spec.sandbox_permissions,
                        is_retry: false,
                    },
                )
                .await;

                if decision.is_approved() {
                    already_approved = true;
                } else {
                    match decision {
                        ReviewDecision::Denied => return Err("rejected by user".to_string()),
                        ReviewDecision::Abort => return Err("aborted by user".to_string()),
                        _ => {}
                    }
                }
            }
            ExecApprovalRequirement::Forbidden { ref reason } => {
                return Err(reason.clone());
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
            ExecApprovalPrompt {
                call_id: &call_id,
                command: &command,
                cwd: &cwd,
                reason: Some(build_denial_reason_from_output(&first_output)),
                sandbox_policy: sandbox.as_ref().map(|s| &s.policy),
                sandbox_permissions: spec.sandbox_permissions,
                is_retry: true,
            },
        )
        .await;

        if !decision.is_approved() {
            match decision {
                ReviewDecision::Denied => return Err("rejected by user".to_string()),
                ReviewDecision::Abort => return Err("aborted by user".to_string()),
                _ => {}
            }
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
    request: ExecApprovalPrompt<'_>,
) -> ReviewDecision {
    let ExecApprovalPrompt {
        call_id,
        command,
        cwd,
        reason,
        sandbox_policy,
        sandbox_permissions,
        is_retry,
    } = request;
    let (sandbox_label, network_access, writable_roots) =
        approval_request_context(sandbox_policy, cwd, sandbox_permissions, is_retry);
    let keys = ApprovalCacheKeys::shell(command, cwd, sandbox_permissions);
    let cache_disabled_reason = ctx.cache_policy.disabled_reason_for_command(command);
    request_cached_approval_with_cache_keys(
        ctx,
        keys,
        cache_disabled_reason,
        |response_tx, cache_disabled_reason| ExecApprovalRequest {
            call_id: call_id.to_string(),
            command: command.to_string(),
            cwd: cwd.to_path_buf(),
            reason,
            is_retry,
            sandbox_label,
            network_access,
            writable_roots,
            cache_disabled_reason,
            response_tx,
        },
    )
    .await
}

struct ExecApprovalPrompt<'a> {
    call_id: &'a str,
    command: &'a str,
    cwd: &'a Path,
    reason: Option<String>,
    sandbox_policy: Option<&'a SandboxPolicy>,
    sandbox_permissions: SandboxPermissions,
    is_retry: bool,
}

fn approval_request_context(
    sandbox_policy: Option<&SandboxPolicy>,
    cwd: &Path,
    sandbox_permissions: SandboxPermissions,
    is_retry: bool,
) -> (String, bool, Vec<PathBuf>) {
    if sandbox_permissions.requires_escalated_permissions() || is_retry {
        return ("outside sandbox".to_string(), true, Vec::new());
    }

    match sandbox_policy {
        Some(SandboxPolicy::DangerFullAccess) => {
            ("danger-full-access".to_string(), true, Vec::new())
        }
        Some(SandboxPolicy::ExternalSandbox { network_access }) => (
            "external-sandbox".to_string(),
            network_access.is_enabled(),
            Vec::new(),
        ),
        Some(SandboxPolicy::ReadOnly) => ("read-only".to_string(), false, Vec::new()),
        Some(policy @ SandboxPolicy::WorkspaceWrite { network_access, .. }) => (
            "workspace-write".to_string(),
            *network_access,
            policy
                .get_writable_roots_with_cwd(cwd)
                .into_iter()
                .map(|root| root.root)
                .collect(),
        ),
        None => ("no sandbox".to_string(), true, Vec::new()),
    }
}

pub fn shell_approval_key(
    command: &str,
    cwd: &Path,
    sandbox_permissions: SandboxPermissions,
) -> String {
    shell_approval_key_with_scope(
        command,
        cwd,
        sandbox_permissions,
        ApprovalScope::Session,
        ApprovalSensitivityTier::Strict,
    )
}

pub fn shell_approval_key_with_scope(
    command: &str,
    cwd: &Path,
    sandbox_permissions: SandboxPermissions,
    scope: ApprovalScope,
    sensitivity_tier: ApprovalSensitivityTier,
) -> String {
    let material = [
        format!("sensitivity_tier={}", sensitivity_tier.as_str()),
        format!("scope={}", scope.as_str()),
        "tool_name=shell".to_string(),
        format!(
            "canonical_args={}",
            canonical_shell_args_for_tier(command, sensitivity_tier)
        ),
        format!("cwd={}", cwd.display()),
        format!(
            "sandbox_scope={}",
            match sandbox_permissions {
                SandboxPermissions::UseDefault => "use_default",
                SandboxPermissions::RequireEscalated => "require_escalated",
            }
        ),
        "target_path=".to_string(),
        "protected_branch=".to_string(),
        "source_slug=".to_string(),
        "network_domain=".to_string(),
        "workspace_id=".to_string(),
    ]
    .join("\n");

    hex::encode(digest(&SHA256, material.as_bytes()).as_ref())
}

fn canonical_shell_args_for_tier(
    command: &str,
    sensitivity_tier: ApprovalSensitivityTier,
) -> String {
    let mut parts = shlex::split(command)
        .filter(|parts| !parts.is_empty())
        .unwrap_or_else(|| {
            command
                .split_whitespace()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        });
    if parts.is_empty() {
        return String::new();
    }

    let argv0 = parts.remove(0);
    let mut flags = Vec::new();
    let mut args = Vec::new();
    for part in parts {
        if part.starts_with('-') {
            flags.push(part);
        } else {
            args.push(part);
        }
    }
    flags.sort();

    match sensitivity_tier {
        ApprovalSensitivityTier::Strict => format!(
            "argv0={};flags={};args={}",
            length_prefixed_list(&[argv0]),
            length_prefixed_list(&flags),
            length_prefixed_list(&args)
        ),
        ApprovalSensitivityTier::Directory => format!(
            "argv0={};flags={};args=<same-cwd>",
            length_prefixed_list(&[argv0]),
            length_prefixed_list(&flags)
        ),
        ApprovalSensitivityTier::Pattern => {
            let arg_patterns = args
                .iter()
                .map(|arg| shell_arg_pattern(arg))
                .collect::<Vec<_>>();
            format!(
                "argv0={};flags={};arg_patterns={}",
                length_prefixed_list(&[argv0]),
                length_prefixed_list(&flags),
                length_prefixed_list(&arg_patterns)
            )
        }
    }
}

fn length_prefixed_list(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("{}:{value}", value.len()))
        .collect::<Vec<_>>()
        .join(",")
}

fn shell_arg_pattern(value: &str) -> String {
    if value.parse::<f64>().is_ok() {
        return "number".to_string();
    }
    if value.contains('/') || value.starts_with('.') || value.starts_with('~') {
        return "path".to_string();
    }
    if value.contains('*') || value.contains('?') || value.contains('[') {
        return "glob".to_string();
    }
    "value".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExecApprovalRequirement {
    Skip { bypass_sandbox: bool },
    NeedsApproval { reason: Option<String> },
    Forbidden { reason: String },
}

fn shell_exec_approval_requirement(
    policy: AskForApproval,
    sandbox_policy: Option<&SandboxPolicy>,
    command: &str,
    sandbox_permissions: SandboxPermissions,
    safety_decision: Option<&SafetyDecision>,
) -> ExecApprovalRequirement {
    if let Some(decision) = safety_decision {
        match decision.disposition {
            SafetyDisposition::Deny => {
                return ExecApprovalRequirement::Forbidden {
                    reason: shell_safety_decision_reason("rejected", decision),
                };
            }
            SafetyDisposition::NeedsHuman => {
                return if matches!(policy, AskForApproval::Never) {
                    ExecApprovalRequirement::Forbidden {
                        reason: shell_safety_decision_reason(
                            "requires human approval but approval policy is never",
                            decision,
                        ),
                    }
                } else {
                    ExecApprovalRequirement::NeedsApproval {
                        reason: Some(shell_safety_decision_reason("needs review", decision)),
                    }
                };
            }
            SafetyDisposition::Allow if !sandbox_permissions.requires_escalated_permissions() => {
                return ExecApprovalRequirement::Skip {
                    bypass_sandbox: false,
                };
            }
            SafetyDisposition::Allow => {}
        }
    }

    if !sandbox_permissions.requires_escalated_permissions()
        && command_safety::is_known_safe_shell_command(command)
    {
        return ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
        };
    }

    let runtime_sandbox_is_weak = cfg!(windows)
        && sandbox_policy.is_some_and(|policy| matches!(policy, SandboxPolicy::ReadOnly));
    if command_safety::shell_command_might_be_dangerous(command) || runtime_sandbox_is_weak {
        return if matches!(policy, AskForApproval::Never) {
            ExecApprovalRequirement::Forbidden {
                reason: "dangerous command rejected by approval policy".to_string(),
            }
        } else {
            ExecApprovalRequirement::NeedsApproval { reason: None }
        };
    }

    match policy {
        AskForApproval::Never | AskForApproval::OnFailure => ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
        },
        AskForApproval::UnlessTrusted => ExecApprovalRequirement::NeedsApproval { reason: None },
        AskForApproval::OnRequest => match sandbox_policy {
            Some(SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. })
            | None => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
            },
            Some(SandboxPolicy::ReadOnly | SandboxPolicy::WorkspaceWrite { .. }) => {
                if sandbox_permissions.requires_escalated_permissions() {
                    ExecApprovalRequirement::NeedsApproval { reason: None }
                } else {
                    ExecApprovalRequirement::Skip {
                        bypass_sandbox: false,
                    }
                }
            }
        },
    }
}

fn shell_safety_decision_reason(prefix: &str, decision: &SafetyDecision) -> String {
    format!(
        "shell safety {prefix}: rule={} blast_radius={} reason={}",
        decision.rule_name, decision.blast_radius, decision.reason
    )
}

pub fn approval_required(policy: AskForApproval, sandbox_policy: Option<&SandboxPolicy>) -> bool {
    match policy {
        AskForApproval::Never | AskForApproval::OnFailure => false,
        AskForApproval::OnRequest => sandbox_policy.is_some_and(|policy| {
            !matches!(
                policy,
                SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
            )
        }),
        AskForApproval::UnlessTrusted => true,
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
    use tokio::sync::mpsc::error::TryRecvError;

    use super::*;

    #[test]
    fn on_request_requires_approval_in_workspace_write() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let requirement = shell_exec_approval_requirement(
            AskForApproval::OnRequest,
            Some(&policy),
            "python script.py",
            SandboxPermissions::RequireEscalated,
            None,
        );
        assert!(matches!(
            requirement,
            ExecApprovalRequirement::NeedsApproval { .. }
        ));
    }

    #[test]
    fn on_request_skips_approval_for_sandboxed_commands() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let requirement = shell_exec_approval_requirement(
            AskForApproval::OnRequest,
            Some(&policy),
            "python script.py",
            SandboxPermissions::UseDefault,
            None,
        );
        assert!(matches!(
            requirement,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: false
            }
        ));
    }

    #[test]
    fn on_request_skips_approval_in_danger_full_access() {
        let requirement = shell_exec_approval_requirement(
            AskForApproval::OnRequest,
            Some(&SandboxPolicy::DangerFullAccess),
            "python script.py",
            SandboxPermissions::RequireEscalated,
            None,
        );
        assert!(matches!(
            requirement,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: false
            }
        ));
    }

    #[test]
    fn unless_trusted_allows_known_safe_commands() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let requirement = shell_exec_approval_requirement(
            AskForApproval::UnlessTrusted,
            Some(&policy),
            "ls -la",
            SandboxPermissions::UseDefault,
            None,
        );
        assert!(matches!(
            requirement,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: false
            }
        ));
    }

    #[test]
    fn never_forbids_dangerous_commands() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let requirement = shell_exec_approval_requirement(
            AskForApproval::Never,
            Some(&policy),
            "git reset --hard",
            SandboxPermissions::UseDefault,
            None,
        );
        assert!(matches!(
            requirement,
            ExecApprovalRequirement::Forbidden { .. }
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

    #[test]
    fn default_timeout_allows_typical_build_commands() {
        assert_eq!(DEFAULT_TIMEOUT_MS, 60_000);
    }

    #[test]
    fn approval_context_reports_workspace_write_details() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("src")],
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        let (sandbox_label, network_access, writable_roots) = approval_request_context(
            Some(&policy),
            Path::new("/tmp/workspace"),
            SandboxPermissions::UseDefault,
            false,
        );

        assert_eq!(sandbox_label, "workspace-write");
        assert!(!network_access);
        assert_eq!(writable_roots, vec![PathBuf::from("/tmp/workspace/src")]);
    }

    #[test]
    fn approval_context_marks_retry_as_outside_sandbox() {
        let (sandbox_label, network_access, writable_roots) = approval_request_context(
            Some(&SandboxPolicy::ReadOnly),
            Path::new("/tmp/workspace"),
            SandboxPermissions::UseDefault,
            true,
        );

        assert_eq!(sandbox_label, "outside sandbox");
        assert!(network_access);
        assert!(writable_roots.is_empty());
    }

    #[tokio::test]
    async fn cached_approval_skips_prompt_when_all_keys_are_preapproved() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let store = Arc::new(tokio::sync::Mutex::new(ApprovalStore::default()));
        {
            let mut guard = store.lock().await;
            guard.put("k1".to_string(), ReviewDecision::ApprovedForSession);
            guard.put("k2".to_string(), ReviewDecision::ApprovedForSession);
        }
        let ctx = ToolApprovalContext {
            policy: AskForApproval::OnRequest,
            request_tx: tx,
            store: Arc::clone(&store),
            scope_key_prefix: None,
            approval_ttl: DEFAULT_APPROVAL_TTL,
            cache_policy: ApprovalCachePolicy::default(),
        };
        let keys = vec!["k1".to_string(), "k2".to_string()];

        let decision =
            request_cached_approval_with_keys(&ctx, &keys, |response_tx| ExecApprovalRequest {
                call_id: "call-1".to_string(),
                command: "echo hi".to_string(),
                cwd: PathBuf::from("/tmp"),
                reason: None,
                is_retry: false,
                sandbox_label: "workspace-write".to_string(),
                network_access: false,
                writable_roots: vec![PathBuf::from("/tmp")],
                cache_disabled_reason: None,
                response_tx,
            })
            .await;

        assert_eq!(decision, ReviewDecision::ApprovedForSession);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn approved_for_session_decision_is_cached_for_each_key() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let store = Arc::new(tokio::sync::Mutex::new(ApprovalStore::default()));
        let ctx = ToolApprovalContext {
            policy: AskForApproval::OnRequest,
            request_tx: tx,
            store: Arc::clone(&store),
            scope_key_prefix: None,
            approval_ttl: DEFAULT_APPROVAL_TTL,
            cache_policy: ApprovalCachePolicy::default(),
        };
        let keys = vec!["a".to_string(), "b".to_string()];

        let responder = tokio::spawn(async move {
            let request = rx.recv().await.expect("approval request expected");
            let _ = request.response_tx.send(ReviewDecision::ApprovedForSession);
        });

        let decision =
            request_cached_approval_with_keys(&ctx, &keys, |response_tx| ExecApprovalRequest {
                call_id: "call-2".to_string(),
                command: "apply_patch".to_string(),
                cwd: PathBuf::from("/tmp"),
                reason: Some("test".to_string()),
                is_retry: false,
                sandbox_label: "workspace-write".to_string(),
                network_access: false,
                writable_roots: vec![PathBuf::from("/tmp")],
                cache_disabled_reason: None,
                response_tx,
            })
            .await;

        responder.await.expect("responder task failed");
        assert_eq!(decision, ReviewDecision::ApprovedForSession);
        let guard = store.lock().await;
        assert_eq!(guard.get("a"), Some(ReviewDecision::ApprovedForSession));
        assert_eq!(guard.get("b"), Some(ReviewDecision::ApprovedForSession));
    }

    #[tokio::test]
    async fn approved_for_all_commands_decision_skips_later_prompts() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let store = Arc::new(tokio::sync::Mutex::new(ApprovalStore::default()));
        let ctx = ToolApprovalContext {
            policy: AskForApproval::OnRequest,
            request_tx: tx,
            store: Arc::clone(&store),
            scope_key_prefix: None,
            approval_ttl: DEFAULT_APPROVAL_TTL,
            cache_policy: ApprovalCachePolicy::default(),
        };

        let responder = tokio::spawn(async move {
            let request = rx.recv().await.expect("approval request expected");
            let _ = request
                .response_tx
                .send(ReviewDecision::ApprovedForAllCommands);
            assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
            rx
        });

        let first_keys = vec!["first".to_string()];
        let first_decision = request_cached_approval_with_keys(&ctx, &first_keys, |response_tx| {
            ExecApprovalRequest {
                call_id: "call-allow-all".to_string(),
                command: "cargo test".to_string(),
                cwd: PathBuf::from("/tmp"),
                reason: None,
                is_retry: false,
                sandbox_label: "workspace-write".to_string(),
                network_access: false,
                writable_roots: vec![PathBuf::from("/tmp")],
                cache_disabled_reason: None,
                response_tx,
            }
        })
        .await;

        assert_eq!(first_decision, ReviewDecision::ApprovedForAllCommands);
        assert!(store.lock().await.allow_all_commands());
        let mut rx = responder.await.expect("responder task failed");

        let second_keys = vec!["different-command".to_string()];
        let second_decision =
            request_cached_approval_with_keys(&ctx, &second_keys, |response_tx| {
                ExecApprovalRequest {
                    call_id: "call-skipped".to_string(),
                    command: "git status".to_string(),
                    cwd: PathBuf::from("/tmp/other"),
                    reason: None,
                    is_retry: false,
                    sandbox_label: "workspace-write".to_string(),
                    network_access: false,
                    writable_roots: vec![PathBuf::from("/tmp/other")],
                    cache_disabled_reason: None,
                    response_tx,
                }
            })
            .await;

        assert_eq!(second_decision, ReviewDecision::ApprovedForAllCommands);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn directory_ttl_approval_reuses_for_same_command_family_in_cwd() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let store = Arc::new(tokio::sync::Mutex::new(ApprovalStore::default()));
        let ctx = ToolApprovalContext {
            policy: AskForApproval::OnRequest,
            request_tx: tx,
            store: Arc::clone(&store),
            scope_key_prefix: None,
            approval_ttl: DEFAULT_APPROVAL_TTL,
            cache_policy: ApprovalCachePolicy::default(),
        };
        let cwd = Path::new("/tmp/workspace");
        let first_keys =
            ApprovalCacheKeys::shell("touch generated-a.txt", cwd, SandboxPermissions::UseDefault);

        let responder = tokio::spawn(async move {
            let request = rx.recv().await.expect("approval request expected");
            let _ = request
                .response_tx
                .send(ReviewDecision::ApprovedForDirectoryTtl);
            rx
        });

        let first_decision = request_cached_approval_with_cache_keys(
            &ctx,
            first_keys,
            None,
            |response_tx, cache_disabled_reason| {
                test_exec_request(
                    "touch generated-a.txt",
                    cwd,
                    response_tx,
                    cache_disabled_reason,
                )
            },
        )
        .await;
        assert_eq!(first_decision, ReviewDecision::ApprovedForDirectoryTtl);

        let mut rx = responder.await.expect("responder task failed");
        let second_keys =
            ApprovalCacheKeys::shell("touch generated-b.txt", cwd, SandboxPermissions::UseDefault);
        let second_decision = request_cached_approval_with_cache_keys(
            &ctx,
            second_keys,
            None,
            |response_tx, cache_disabled_reason| {
                test_exec_request(
                    "touch generated-b.txt",
                    cwd,
                    response_tx,
                    cache_disabled_reason,
                )
            },
        )
        .await;

        assert_eq!(second_decision, ReviewDecision::ApprovedForTtl);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn protected_branch_policy_disables_approval_cache_reuse() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let store = Arc::new(tokio::sync::Mutex::new(ApprovalStore::default()));
        let ctx = ToolApprovalContext {
            policy: AskForApproval::OnRequest,
            request_tx: tx,
            store: Arc::clone(&store),
            scope_key_prefix: None,
            approval_ttl: DEFAULT_APPROVAL_TTL,
            cache_policy: ApprovalCachePolicy {
                protected_branches: vec!["main".to_string()],
                allowed_network_domains: Vec::new(),
                no_cache_unknown_network: false,
            },
        };

        let responder = tokio::spawn(async move {
            let first = rx.recv().await.expect("first approval request expected");
            assert!(
                first
                    .cache_disabled_reason
                    .as_deref()
                    .unwrap_or_default()
                    .contains("protected branch `main`")
            );
            let _ = first.response_tx.send(ReviewDecision::ApprovedForTtl);

            let second = rx.recv().await.expect("second approval request expected");
            assert!(second.cache_disabled_reason.is_some());
            let _ = second.response_tx.send(ReviewDecision::Denied);
        });

        let first = request_exec_approval(
            &ctx,
            ExecApprovalPrompt {
                call_id: "call-main-1",
                command: "libra switch main",
                cwd: Path::new("/tmp/workspace"),
                reason: None,
                sandbox_policy: None,
                sandbox_permissions: SandboxPermissions::UseDefault,
                is_retry: false,
            },
        )
        .await;
        assert_eq!(first, ReviewDecision::Approved);

        let second = request_exec_approval(
            &ctx,
            ExecApprovalPrompt {
                call_id: "call-main-2",
                command: "libra switch main",
                cwd: Path::new("/tmp/workspace"),
                reason: None,
                sandbox_policy: None,
                sandbox_permissions: SandboxPermissions::UseDefault,
                is_retry: false,
            },
        )
        .await;
        assert_eq!(second, ReviewDecision::Denied);

        responder.await.expect("responder task failed");
        assert!(store.lock().await.active_memos_at(Utc::now()).is_empty());
    }

    #[test]
    fn approval_cache_policy_flags_non_allowlisted_network_domains() {
        let policy = ApprovalCachePolicy {
            protected_branches: Vec::new(),
            allowed_network_domains: vec!["github.com".to_string()],
            no_cache_unknown_network: true,
        };

        assert!(
            policy
                .disabled_reason_for_command("curl https://api.github.com/repos")
                .is_none()
        );
        assert!(
            policy
                .disabled_reason_for_command("curl example.com/path")
                .unwrap()
                .contains("example.com")
        );
    }

    #[tokio::test]
    async fn scoped_approval_does_not_inherit_interactive_session_cache() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let store = Arc::new(tokio::sync::Mutex::new(ApprovalStore::default()));
        {
            let mut guard = store.lock().await;
            guard.put(
                "shell:/tmp/workspace".to_string(),
                ReviewDecision::ApprovedForSession,
            );
            guard.approve_all_commands();
        }
        let automation_ctx = ToolApprovalContext {
            policy: AskForApproval::OnRequest,
            request_tx: tx,
            store: Arc::clone(&store),
            scope_key_prefix: Some("automation:thread-1".to_string()),
            approval_ttl: DEFAULT_APPROVAL_TTL,
            cache_policy: ApprovalCachePolicy::default(),
        };
        let keys = vec!["shell:/tmp/workspace".to_string()];

        let responder = tokio::spawn(async move {
            let request = rx
                .recv()
                .await
                .expect("automation approval request expected");
            let _ = request.response_tx.send(ReviewDecision::Denied);
        });
        let decision = request_cached_approval_with_keys(&automation_ctx, &keys, |response_tx| {
            ExecApprovalRequest {
                call_id: "call-automation".to_string(),
                command: "cargo test".to_string(),
                cwd: PathBuf::from("/tmp/workspace"),
                reason: None,
                is_retry: false,
                sandbox_label: "workspace-write".to_string(),
                network_access: false,
                writable_roots: vec![PathBuf::from("/tmp/workspace")],
                cache_disabled_reason: None,
                response_tx,
            }
        })
        .await;

        responder.await.expect("responder task failed");
        assert_eq!(decision, ReviewDecision::Denied);
        assert!(store.lock().await.allow_all_commands());
        assert!(
            !store
                .lock()
                .await
                .allow_all_commands_for_scope("automation:thread-1")
        );
    }

    fn test_exec_request(
        command: &str,
        cwd: &Path,
        response_tx: tokio::sync::oneshot::Sender<ReviewDecision>,
        cache_disabled_reason: Option<String>,
    ) -> ExecApprovalRequest {
        ExecApprovalRequest {
            call_id: "call-test".to_string(),
            command: command.to_string(),
            cwd: cwd.to_path_buf(),
            reason: None,
            is_retry: false,
            sandbox_label: "workspace-write".to_string(),
            network_access: false,
            writable_roots: vec![cwd.to_path_buf()],
            cache_disabled_reason,
            response_tx,
        }
    }
}
