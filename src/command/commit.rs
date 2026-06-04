//! Commit command that collects staged changes, builds tree and commit objects, validates messages (including GPG), and updates HEAD/refs.

use std::{
    collections::HashSet,
    io::{IsTerminal, Write},
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::{
        index::{Index, IndexEntry},
        object::{
            ObjectTrait,
            blob::Blob,
            commit::Commit,
            signature::{Signature, SignatureType},
            tree::{Tree, TreeItem, TreeItemMode},
            types::ObjectType,
        },
    },
};
use sea_orm::ConnectionTrait;
use serde::Serialize;

use crate::{
    command::{load_object, save_object_to_storage, status},
    common_utils::{check_conventional_commits_message, format_commit_msg},
    internal::{
        ai::automation::{VCS_EVENT_POST_COMMIT, dispatch_current_repo_vcs_event_to_history},
        branch::Branch,
        config::{LocalIdentityTarget, read_cascaded_config_value, resolve_user_identity_sources},
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        lfs,
        object_ext::BlobExt,
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

/// Create a new commit from staged changes.
///
/// See `libra commit --help` for the same examples rendered through clap.
// GitHub Issues URL surfaced on internal-invariant bug paths
// (`CommitError::TreeCreation`) so users can report unexpected
// tree-build failures. Mirrors push.rs / tag.rs's hint pattern per
// Cross-Cutting G.
const ISSUE_URL: &str = "https://github.com/web3infra-foundation/libra/issues";

/// `--help` examples shown in `libra commit --help` output.
///
/// Per `docs/improvement/commit.md`, the commit command exposes nine
/// representative scenarios so users see the most common invocations
/// without having to read the doc. Keep this list and the rustdoc
/// snippet in `commit.md` in sync.
pub const COMMIT_EXAMPLES: &str = "\
EXAMPLES:
    libra commit -m 'Add new feature'                Create a commit with message
    libra commit -m 'feat: add login' --conventional Validate conventional commit format
    libra commit                                     Write the message in your editor
    libra commit -e -m 'wip'                         Open the editor seeded with a message
    libra commit --amend                             Amend the last commit
    libra commit --amend --no-edit                   Amend without changing the message
    libra commit -a -m 'Fix typo'                    Auto-stage tracked changes and commit
    libra commit -F message.txt                      Read commit message from file
    libra commit -t template.txt                     Seed the message from a template file
    libra commit --cleanup=verbatim -m '#keep'       Keep comment lines verbatim
    libra commit -s -m 'Add feature'                 Add Signed-off-by trailer
    libra commit -S -m 'Signed release'              Force a vault-backed GPG signature
    libra commit --no-gpg-sign -m 'Quick fix'        Suppress signing for this commit
    libra commit --allow-empty -m 'Trigger CI'       Create an empty commit
    libra commit --json -m 'Add feature'             Structured JSON output for agents";

/// Commit-message cleanup mode, mirroring `git commit --cleanup=<mode>`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum CleanupMode {
    /// Strip leading/trailing empty lines, trailing whitespace, and `#` comment lines.
    Strip,
    /// Only trim trailing whitespace and leading/trailing empty lines (keeps `#` lines).
    Whitespace,
    /// Do not change the message at all.
    Verbatim,
    /// Truncate at the scissors line (`# ----- >8 -----`), then apply `whitespace`.
    Scissors,
    /// `strip` when an editor was used, `whitespace` otherwise.
    #[default]
    Default,
}

#[derive(Parser, Debug, Default)]
#[command(after_help = COMMIT_EXAMPLES)]
pub struct CommitArgs {
    /// Commit message body
    #[arg(short, long)]
    pub message: Option<String>,

    /// read message from file
    #[arg(short = 'F', long)]
    pub file: Option<String>,

    /// allow commit with empty index
    #[arg(long)]
    pub allow_empty: bool,

    /// check if the commit message follows conventional commits
    #[arg(long)]
    pub conventional: bool,

    /// amend the last commit
    #[arg(long)]
    pub amend: bool,

    /// Force launching the editor to edit the message, even when one is supplied
    #[arg(short = 'e', long, conflicts_with = "no_edit")]
    pub edit: bool,

    /// Do not launch the editor; use the supplied/template/amended message as-is
    #[arg(long)]
    pub no_edit: bool,

    /// How to clean up the commit message before committing
    #[arg(long, value_enum, default_value_t = CleanupMode::Default)]
    pub cleanup: CleanupMode,

    /// Use the contents of <file> as the initial commit message
    #[arg(short = 't', long, value_name = "file")]
    pub template: Option<String>,

    /// add signed-off-by line at the end of the commit message
    #[arg(short = 's', long)]
    pub signoff: bool,

    /// GPG/Vault-sign the commit (forces signing regardless of config)
    #[arg(short = 'S', long = "gpg-sign")]
    pub gpg_sign: bool,

    /// Do not GPG/Vault-sign the commit (overrides commit.gpgSign / vault.signing)
    #[arg(long = "no-gpg-sign", conflicts_with = "gpg_sign")]
    pub no_gpg_sign: bool,

    /// Skip pre-commit hooks for this invocation (narrower than --no-verify, which also skips commit-msg hooks)
    #[arg(long)]
    pub disable_pre: bool,

    /// Automatically stage tracked files that have been modified or deleted
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Skip all pre-commit and commit-msg hooks/validations (align with Git --no-verify)
    #[arg(long = "no-verify")]
    pub no_verify: bool,

    /// Override the commit author. Specify an explicit author using the standard A U Thor <author@example.com> format.
    #[arg(long)]
    pub author: Option<String>,
}

// ---------------------------------------------------------------------------
// Structured error types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    #[error("failed to load index: {0}")]
    IndexLoad(String),

    #[error("failed to save index: {0}")]
    IndexSave(String),

    #[error("nothing to commit, working tree clean")]
    NothingToCommit,

    #[error("nothing to commit (create/copy files and use 'libra add' to track)")]
    NothingToCommitNoTracked,

    #[error("{0}")]
    IdentityMissing(String),

    #[error("there is no commit to amend")]
    NoCommitToAmend,

    #[error("amend is not supported for merge commits with multiple parents")]
    AmendUnsupported,

    #[error("invalid author format: {0}")]
    InvalidAuthor(String),

    #[error("failed to read message file '{path}': {detail}")]
    MessageFileRead { path: String, detail: String },

    #[error("aborting commit due to empty commit message")]
    EmptyMessage,

    #[error("failed to create tree: {0}")]
    TreeCreation(String),

    #[error("failed to store commit object: {0}")]
    ObjectStorage(String),

    #[error("failed to load parent commit '{commit_id}': {detail}")]
    ParentCommitLoad { commit_id: String, detail: String },

    #[error("failed to update HEAD: {0}")]
    HeadUpdate(String),

    #[error("pre-commit hook failed: {0}")]
    PreCommitHook(String),

    #[error("conventional commit validation failed: {0}")]
    ConventionalCommit(String),

    #[error("failed to sign commit: {0}")]
    VaultSign(String),

    #[error("failed to auto-stage tracked changes: {0}")]
    AutoStage(String),

    #[error("failed to calculate staged changes: {0}")]
    StagedChanges(String),

    #[error("commit-msg hook failed: {0}")]
    CommitMsgHook(String),

    #[error("commit message editor failed: {0}")]
    EditorFailed(String),

    #[error(
        "cannot open an editor for the commit message without a terminal; provide a message with -m/-F or use --no-edit"
    )]
    NoEditorAvailable,
}

impl From<CommitError> for CliError {
    fn from(error: CommitError) -> Self {
        match &error {
            CommitError::IndexLoad(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("the index file may be corrupted; try 'libra status' to verify"),
            CommitError::IndexSave(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            CommitError::NothingToCommit => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("use 'libra add' to stage changes")
                .with_hint("use 'libra status' to see what changed"),
            CommitError::NothingToCommitNoTracked => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("create/copy files and use 'libra add' to track"),
            CommitError::IdentityMissing(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::AuthMissingCredentials)
                .with_hint("run 'libra config --global user.name \"Your Name\"' and 'libra config --global user.email \"you@example.com\"'")
                .with_hint("omit '--global' to set the identity only in this repository."),
            CommitError::NoCommitToAmend => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("create a commit before using --amend"),
            CommitError::AmendUnsupported => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("create a new commit instead of amending a merge commit"),
            CommitError::InvalidAuthor(..) => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("expected format: 'Name <email>'"),
            CommitError::MessageFileRead { .. } => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            CommitError::EmptyMessage => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("use -m to provide a commit message"),
            CommitError::TreeCreation(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::InternalInvariant)
                .with_hint(format!("this is a bug; please report it at {ISSUE_URL}")),
            CommitError::ObjectStorage(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            CommitError::ParentCommitLoad { .. } => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("the parent commit is missing or corrupted"),
            CommitError::HeadUpdate(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            CommitError::PreCommitHook(..) => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("use --no-verify to bypass the hook"),
            CommitError::ConventionalCommit(..) => CliError::command_usage(error.to_string())
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("see https://www.conventionalcommits.org for format rules"),
            CommitError::VaultSign(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::AuthMissingCredentials)
                .with_hint("check vault configuration with 'libra config --list'"),
            CommitError::AutoStage(..) => {
                CliError::fatal(error.to_string()).with_stable_code(StableErrorCode::IoReadFailed)
            }
            CommitError::StagedChanges(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoCorrupt)
                .with_hint("failed to compute staged changes"),
            CommitError::CommitMsgHook(..) => CliError::failure(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("fix the commit message, or use --no-verify to bypass the hook"),
            CommitError::EditorFailed(..) => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("set $EDITOR / core.editor, or provide the message with -m/-F"),
            CommitError::NoEditorAvailable => CliError::fatal(error.to_string())
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("provide a message with -m/-F, or run in a terminal with an editor"),
        }
    }
}

// ---------------------------------------------------------------------------
// Structured output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FilesChanged {
    pub total: usize,
    pub new: usize,
    pub modified: usize,
    pub deleted: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitOutput {
    /// Branch name or "detached" (backward-compatible with existing JSON consumers)
    pub head: String,
    /// Explicit branch indicator: Some(name) if on branch, None if detached HEAD
    pub branch: Option<String>,
    /// Full commit hash
    pub commit: String,
    /// Short commit hash (7 chars)
    pub short_id: String,
    /// First line of commit message
    pub subject: String,
    /// Whether this is a root commit (no parents)
    pub root_commit: bool,
    /// Whether this was an amend operation
    pub amend: bool,
    /// File change statistics
    pub files_changed: FilesChanged,
    /// Whether Signed-off-by trailer was appended
    pub signoff: bool,
    /// Conventional commit validation result: Some(true) if validated, None if not requested
    pub conventional: Option<bool>,
    /// Whether the commit was vault-GPG-signed
    pub signed: bool,
}

/// Parse author string in format "Name <email>" and return (name, email)
/// If parsing fails, return an error message
fn parse_author(author: &str) -> Result<(String, String), CommitError> {
    let author = author.trim();

    // Try to parse "Name <email>" format
    if let Some(start_idx) = author.find('<')
        && let Some(end_idx) = author[start_idx..].find('>')
    {
        let end_idx = start_idx + end_idx;
        if start_idx < end_idx && end_idx == author.len() - 1 {
            let name = author[..start_idx].trim().to_string();
            let email = author[start_idx + 1..end_idx].trim().to_string();

            if !name.is_empty() && !email.is_empty() {
                return Ok((name, email));
            }
        }
    }

    Err(CommitError::InvalidAuthor(format!(
        "'{author}'. Expected format: 'Name <email>'"
    )))
}

/// A user's name + email pair used for commit authoring and committing.
#[derive(Clone, Debug)]
struct UserIdentity {
    name: String,
    email: String,
}

async fn get_user_config_value(key: &str) -> Option<String> {
    read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, &format!("user.{key}"))
        .await
        .ok()
        .flatten()
}

fn missing_identity_error(name_missing: bool, email_missing: bool) -> CommitError {
    let detail = match (name_missing, email_missing) {
        (true, true) => "author identity unknown: name and email are not configured",
        (true, false) => "author identity unknown: name is not configured",
        (false, true) => "author identity unknown: email is not configured",
        (false, false) => "author identity unknown",
    };
    CommitError::IdentityMissing(detail.to_string())
}

async fn resolve_committer_identity() -> Result<UserIdentity, CommitError> {
    let identity_sources = resolve_user_identity_sources(LocalIdentityTarget::CurrentRepo)
        .await
        .map_err(|error| CommitError::IdentityMissing(error.to_string()))?;

    // Step 2: check user.useConfigOnly BEFORE falling back to env vars.
    // When useConfigOnly is true, only config values are acceptable — env vars are
    // skipped so the user is forced to configure identity
    // explicitly.  This is stricter than Git (which still honours GIT_AUTHOR_*
    // env vars) and prevents silent identity leakage from server environments.
    let use_config_only = get_user_config_value("useConfigOnly")
        .await
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false);

    if use_config_only {
        if let (Some(name), Some(email)) = (
            identity_sources.config_name.clone(),
            identity_sources.config_email.clone(),
        ) {
            return Ok(UserIdentity { name, email });
        }
        // Report which field(s) are missing — using *config-only* perspective.
        // Reuse the already-fetched values instead of querying config again.
        let name_missing = identity_sources.config_name.is_none();
        let email_missing = identity_sources.config_email.is_none();
        return Err(missing_identity_error(name_missing, email_missing));
    }

    // Step 3: env-var fallback (GIT_COMMITTER_*, GIT_AUTHOR_*, EMAIL, LIBRA_COMMITTER_*)
    let name = identity_sources.config_name.or(identity_sources.env_name);
    let email = identity_sources.config_email.or(identity_sources.env_email);

    if let (Some(name), Some(email)) = (name.clone(), email.clone()) {
        return Ok(UserIdentity { name, email });
    }

    Err(missing_identity_error(name.is_none(), email.is_none()))
}

/// Create author and committer signatures based on the provided arguments
async fn create_commit_signatures(
    author_override: Option<&str>,
) -> Result<(Signature, Signature, UserIdentity), CommitError> {
    let committer_identity = resolve_committer_identity().await?;

    // Create author signature (use override if provided)
    let author = if let Some(author_str) = author_override {
        let (name, email) = parse_author(author_str)?;
        Signature::new(SignatureType::Author, name, email)
    } else {
        Signature::new(
            SignatureType::Author,
            committer_identity.name.clone(),
            committer_identity.email.clone(),
        )
    };

    // Committer always uses default user info
    let committer = Signature::new(
        SignatureType::Committer,
        committer_identity.name.clone(),
        committer_identity.email.clone(),
    );

    Ok((author, committer, committer_identity))
}

fn first_message_line(message: &str) -> String {
    message.lines().next().unwrap_or("").trim().to_string()
}

/// Pure execution entry point. Receives `&OutputConfig` only for hook I/O
/// control (human mode: inherit, JSON/machine mode: piped). Does NOT render
/// output — returns [`CommitOutput`] on success for the caller to render.
/// The scissors cut-line marker (`# ------ >8 ------`). Any line that begins
/// with `# ` and contains the ` >8 ` token is treated as a cut line, matching
/// Git's `wt_status_truncate_message_at_cut_line` semantics.
const SCISSORS_TOKEN: &str = " >8 ";

/// Clean a commit message according to `mode`. `edited` selects the behaviour of
/// [`CleanupMode::Default`] (`strip` after an editor, `whitespace` otherwise).
fn cleanup_message(message: &str, mode: CleanupMode, edited: bool) -> String {
    let effective = match mode {
        CleanupMode::Default if edited => CleanupMode::Strip,
        CleanupMode::Default => CleanupMode::Whitespace,
        other => other,
    };

    if effective == CleanupMode::Verbatim {
        return message.to_string();
    }

    // `scissors` truncates at the first cut line, then applies `whitespace`.
    let body = if effective == CleanupMode::Scissors {
        let mut kept = Vec::new();
        for line in message.lines() {
            if line.starts_with("# ") && line.contains(SCISSORS_TOKEN) {
                break;
            }
            kept.push(line);
        }
        kept.join("\n")
    } else {
        message.to_string()
    };

    let strip_comments = effective == CleanupMode::Strip;
    let mut cleaned: Vec<&str> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_end();
        if strip_comments && trimmed.starts_with('#') {
            continue;
        }
        cleaned.push(trimmed);
    }
    // Collapse runs of blank lines and drop leading/trailing blanks.
    let mut result: Vec<&str> = Vec::new();
    let mut pending_blank = false;
    for line in cleaned {
        if line.is_empty() {
            if !result.is_empty() {
                pending_blank = true;
            }
            continue;
        }
        if pending_blank {
            result.push("");
            pending_blank = false;
        }
        result.push(line);
    }
    result.join("\n")
}

/// Resolve an **explicitly configured** editor command, in Git's precedence
/// order: `$GIT_EDITOR` → `core.editor` → `$VISUAL` → `$EDITOR`. Returns `None`
/// when nothing is configured (the caller falls back to `vi` only on a TTY).
async fn resolve_explicit_editor() -> Option<String> {
    let nonempty_env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());

    if let Some(value) = nonempty_env("GIT_EDITOR") {
        return Some(value);
    }
    if let Ok(Some(value)) =
        read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "core.editor").await
        && !value.trim().is_empty()
    {
        return Some(value);
    }
    nonempty_env("VISUAL").or_else(|| nonempty_env("EDITOR"))
}

/// Launch the editor on a temp file pre-filled with `initial_content`, returning
/// the edited contents. The editor command is split on whitespace into argv
/// (program + leading args) and the file path is appended as the last argument;
/// it is **never** wrapped in a shell, preventing command injection.
/// The signing decision derived from the `-S`/`--no-gpg-sign` flags and the
/// `commit.gpgSign` config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignDecision {
    /// Never sign (`--no-gpg-sign`).
    Skip,
    /// Always sign, bypassing the `vault.signing` gate (`-S` / `commit.gpgSign=true`).
    SignForce,
    /// Sign only if `vault.signing=true` (the default; current behavior).
    SignIfVaultEnabled,
}

/// Read `commit.gpgSign` config, accepting both casings (Git is case-insensitive
/// for config names, but `ConfigKv` matches keys exactly).
async fn read_commit_gpgsign_config() -> Option<bool> {
    for key in ["commit.gpgSign", "commit.gpgsign"] {
        if let Ok(Some(value)) =
            read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, key).await
        {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" => return Some(true),
                "false" => return Some(false),
                _ => {}
            }
        }
    }
    None
}

/// Resolve the signing decision (`--no-gpg-sign` > `-S` > `commit.gpgSign` > default).
async fn resolve_sign_decision(args: &CommitArgs) -> SignDecision {
    if args.no_gpg_sign {
        return SignDecision::Skip;
    }
    if args.gpg_sign {
        return SignDecision::SignForce;
    }
    match read_commit_gpgsign_config().await {
        Some(true) => SignDecision::SignForce,
        _ => SignDecision::SignIfVaultEnabled,
    }
}

/// Launch the editor on `file_path`, splitting the command into argv (program +
/// leading args) and appending the file path as the last argument. Never wrapped
/// in a shell, preventing command injection.
fn launch_editor_on(file_path: &std::path::Path, editor_cmd: &str) -> Result<(), CommitError> {
    let parts: Vec<&str> = editor_cmd.split_whitespace().collect();
    let Some((program, leading_args)) = parts.split_first() else {
        return Err(CommitError::EditorFailed(
            "empty editor command".to_string(),
        ));
    };
    let mut command = Command::new(program);
    command.args(leading_args).arg(file_path);
    let status = command
        .status()
        .map_err(|e| CommitError::EditorFailed(format!("failed to start editor: {e}")))?;
    if !status.success() {
        return Err(CommitError::EditorFailed(format!(
            "editor exited with status {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Run a Libra-native message hook (`prepare-commit-msg`/`commit-msg`) from
/// `.libra/hooks/<name>.sh` (Unix) / `.ps1` (Windows). The hook receives the
/// message file path as its first argument (plus any `extra_args`) and may
/// rewrite the file. A missing hook is a no-op; a non-zero exit aborts the commit.
fn run_message_hook(
    name: &str,
    msg_path: &std::path::Path,
    extra_args: &[&str],
    output: &OutputConfig,
) -> Result<(), CommitError> {
    let hooks_dir = path::hooks();
    #[cfg(not(target_os = "windows"))]
    let hook_path = hooks_dir.join(format!("{name}.sh"));
    #[cfg(target_os = "windows")]
    let hook_path = hooks_dir.join(format!("{name}.ps1"));
    if !hook_path.exists() {
        return Ok(());
    }
    let hook_display = hook_path.display().to_string();
    let (stdout_cfg, stderr_cfg) = if output.is_json() {
        (Stdio::piped(), Stdio::piped())
    } else {
        (Stdio::inherit(), Stdio::inherit())
    };
    let msg_arg = msg_path.to_string_lossy().into_owned();
    #[cfg(not(target_os = "windows"))]
    let mut command = Command::new("sh");
    #[cfg(not(target_os = "windows"))]
    command.arg(&hook_path).arg(&msg_arg);
    #[cfg(target_os = "windows")]
    let mut command = Command::new("powershell");
    #[cfg(target_os = "windows")]
    command.arg("-File").arg(&hook_path).arg(&msg_arg);
    for arg in extra_args {
        command.arg(arg);
    }
    let result = command
        .current_dir(util::working_dir())
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .output()
        .map_err(|e| {
            CommitError::CommitMsgHook(format!("failed to execute hook {hook_display}: {e}"))
        })?;
    if !result.status.success() {
        return Err(CommitError::CommitMsgHook(format!(
            "hook {hook_display} failed with exit code {}",
            result.status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Read the initial message content from `-t/--template` or `commit.template`.
async fn load_template_content(args: &CommitArgs) -> Result<Option<String>, CommitError> {
    let template_path = match &args.template {
        Some(path) => Some(path.clone()),
        None => read_cascaded_config_value(LocalIdentityTarget::CurrentRepo, "commit.template")
            .await
            .ok()
            .flatten(),
    };
    match template_path {
        Some(path) => {
            let content =
                std::fs::read_to_string(&path).map_err(|e| CommitError::MessageFileRead {
                    path,
                    detail: e.to_string(),
                })?;
            Ok(Some(content))
        }
        None => Ok(None),
    }
}

/// Resolve the final commit message from all sources (explicit `-m`/`-F`,
/// `-t`/`commit.template`, an editor, or the amended parent message), running the
/// message hooks and applying the configured `--cleanup` mode. The lifecycle is:
/// initial content → editor → `prepare-commit-msg` → cleanup → `commit-msg`.
/// Returns the message body **without** the signoff trailer (appended by the caller).
async fn resolve_commit_message(
    args: &CommitArgs,
    output: &OutputConfig,
    amend_parent_message: Option<&str>,
    skip_commit_msg: bool,
) -> Result<String, CommitError> {
    let has_explicit_message = args.message.is_some() || args.file.is_some();

    // Load any template once (used both for the initial content and the
    // prepare-commit-msg `source` label).
    let template_content = if has_explicit_message {
        None
    } else {
        load_template_content(args).await?
    };

    // 1. Initial content (precedence: -m > -F > template > amend-parent > empty).
    let mut content = if let Some(msg) = &args.message {
        msg.clone()
    } else if let Some(file_path) = &args.file {
        tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| CommitError::MessageFileRead {
                path: file_path.clone(),
                detail: e.to_string(),
            })?
    } else if let Some(template) = &template_content {
        template.clone()
    } else if let Some(parent) = amend_parent_message {
        parent.to_string()
    } else {
        String::new()
    };

    // The prepare-commit-msg `source` argument, aligned with Git.
    let source = if has_explicit_message {
        "message"
    } else if template_content.is_some() {
        "template"
    } else if amend_parent_message.is_some() {
        "commit"
    } else {
        "message"
    };

    // Hold the message in a single COMMIT_EDITMSG temp file shared by the editor
    // and the message hooks (RAII cleanup on drop).
    let temp_dir = tempfile::tempdir()
        .map_err(|e| CommitError::EditorFailed(format!("failed to create temp dir: {e}")))?;
    let file_path = temp_dir.path().join("COMMIT_EDITMSG");
    std::fs::write(&file_path, &content)
        .map_err(|e| CommitError::EditorFailed(format!("failed to write edit file: {e}")))?;

    // 2. Editor. `--edit` forces it; `--no-edit` suppresses it; otherwise it runs
    //    only when no explicit message was supplied.
    let launch_editor = if args.edit {
        true
    } else if args.no_edit {
        false
    } else {
        !has_explicit_message
    };
    let mut edited = false;
    if launch_editor {
        // An explicitly configured editor runs even without a TTY (e.g. a CI
        // script editor); the `vi` fallback requires a terminal or we would hang.
        let editor = match resolve_explicit_editor().await {
            Some(editor) => editor,
            None => {
                if !std::io::stdin().is_terminal() {
                    return Err(CommitError::NoEditorAvailable);
                }
                "vi".to_string()
            }
        };
        launch_editor_on(&file_path, &editor)?;
        edited = true;
    }

    // 3. prepare-commit-msg (before cleanup; sees the raw template/comments).
    if !skip_commit_msg {
        run_message_hook("prepare-commit-msg", &file_path, &[source], output)?;
    }
    content = std::fs::read_to_string(&file_path)
        .map_err(|e| CommitError::EditorFailed(format!("failed to read edit file: {e}")))?;

    // 4. Cleanup.
    let cleaned = cleanup_message(&content, args.cleanup, edited);
    std::fs::write(&file_path, &cleaned)
        .map_err(|e| CommitError::EditorFailed(format!("failed to write edit file: {e}")))?;

    // 5. commit-msg (after cleanup; sees the near-final message and may rewrite it).
    if !skip_commit_msg {
        run_message_hook("commit-msg", &file_path, &[], output)?;
        std::fs::read_to_string(&file_path)
            .map_err(|e| CommitError::EditorFailed(format!("failed to read edit file: {e}")))
    } else {
        Ok(cleaned)
    }
}

pub async fn run_commit(
    args: CommitArgs,
    output: &OutputConfig,
) -> Result<CommitOutput, CommitError> {
    let is_amend = args.amend;
    let is_signoff = args.signoff;
    let is_conventional = args.conventional;
    // `--disable-pre` only skips pre-commit; `--no-verify` skips pre-commit AND
    // the commit-msg/prepare-commit-msg hooks and the conventional check.
    let skip_pre_commit = args.disable_pre || args.no_verify;
    let skip_commit_msg = args.no_verify;
    let skip_conventional_check = args.no_verify;

    // Auto-stage tracked modifications/deletions (git commit -a)
    let auto_stage_applied = if args.all {
        auto_stage_tracked_changes()?
    } else {
        false
    };

    let index = Index::load(path::index()).map_err(|e| CommitError::IndexLoad(e.to_string()))?;
    let storage = ClientStorage::init(path::objects());
    let tracked_entries = index.tracked_entries(0);

    // Skip empty commit check for --amend operations
    if tracked_entries.is_empty() && !args.allow_empty && !is_amend && !auto_stage_applied {
        // No files have ever been staged — distinct from "staged but unchanged"
        return Err(CommitError::NothingToCommitNoTracked);
    }

    // Verify staged changes relative to HEAD (skip for --amend)
    let staged_changes = status::changes_to_be_committed_safe()
        .await
        .map_err(|e| CommitError::StagedChanges(e.to_string()))?;
    if staged_changes.is_empty() && !args.allow_empty && !is_amend {
        return Err(CommitError::NothingToCommit);
    }

    // INVARIANT: hooks and message validation must run before creating the
    // commit object or updating HEAD; once those writes happen, hook failure can
    // no longer block the commit without explicit rollback logic.
    if !skip_pre_commit {
        run_pre_commit_hook(output)?;
    }

    // Resolve parents and amend context before resolving the message: the
    // amended parent's message seeds the editor / is reused with --no-edit.
    let parents_commit_ids = get_parents_ids().await;
    let (effective_parents, amend_parent_message) = if is_amend {
        if parents_commit_ids.is_empty() {
            return Err(CommitError::NoCommitToAmend);
        }
        if parents_commit_ids.len() > 1 {
            return Err(CommitError::AmendUnsupported);
        }
        let parent_commit = load_object::<Commit>(&parents_commit_ids[0]).map_err(|e| {
            CommitError::ParentCommitLoad {
                commit_id: parents_commit_ids[0].to_string(),
                detail: e.to_string(),
            }
        })?;
        (
            parent_commit.parent_commit_ids.clone(),
            Some(parent_commit.message.clone()),
        )
    } else {
        (parents_commit_ids, None)
    };

    // Resolve the cleaned/hooked commit message from all sources.
    let message = resolve_commit_message(
        &args,
        output,
        amend_parent_message.as_deref(),
        skip_commit_msg,
    )
    .await?;

    // A message that is empty after cleanup aborts the commit (Git parity).
    // `--allow-empty` permits an empty tree, not an empty message.
    if message.trim().is_empty() {
        return Err(CommitError::EmptyMessage);
    }

    // Create tree
    let tree = create_tree(&index, &storage, "".into()).await?;

    // Create author and committer signatures
    let (author, committer, committer_identity) =
        create_commit_signatures(args.author.as_deref()).await?;

    // Build the signoff trailer
    let signoff_line = if is_signoff {
        Some(format!(
            "Signed-off-by: {} <{}>",
            committer_identity.name, committer_identity.email
        ))
    } else {
        None
    };

    let commit_message = match &signoff_line {
        Some(line) => format!("{message}\n\n{line}"),
        None => message.clone(),
    };

    // Conventional commit validation
    if is_conventional
        && !skip_conventional_check
        && !check_conventional_commits_message(&commit_message)
    {
        return Err(CommitError::ConventionalCommit(
            "commit message does not follow conventional commits".to_string(),
        ));
    }

    // Apply the signing decision (`-S` / `--no-gpg-sign` / `commit.gpgSign`).
    let gpg_sig = match resolve_sign_decision(&args).await {
        SignDecision::Skip => None,
        SignDecision::SignForce => {
            vault_sign_commit(
                &tree.id,
                &effective_parents,
                &author,
                &committer,
                &commit_message,
                true,
            )
            .await?
        }
        SignDecision::SignIfVaultEnabled => {
            vault_sign_commit(
                &tree.id,
                &effective_parents,
                &author,
                &committer,
                &commit_message,
                false,
            )
            .await?
        }
    };

    let commit = Commit::new(
        author,
        committer,
        tree.id,
        effective_parents,
        &format_commit_msg(&commit_message, gpg_sig.as_deref()),
    );

    // INVARIANT: persist the commit object before moving HEAD so a crash after
    // ref update never points the branch at a missing object.
    save_commit_object(&storage, &commit)?;
    update_head_and_reflog(&commit.id.to_string(), &commit_message).await?;

    let conventional_result = if is_conventional && !skip_conventional_check {
        Some(true)
    } else {
        None
    };
    Ok(build_commit_output(
        &commit,
        &commit_message,
        &staged_changes,
        is_amend,
        is_signoff,
        conventional_result,
        gpg_sig.is_some(),
    )
    .await)
}

/// Run the pre-commit hook, respecting OutputConfig for I/O isolation.
fn run_pre_commit_hook(output: &OutputConfig) -> Result<(), CommitError> {
    let hooks_dir = path::hooks();

    #[cfg(not(target_os = "windows"))]
    let hook_path = hooks_dir.join("pre-commit.sh");

    #[cfg(target_os = "windows")]
    let hook_path = hooks_dir.join("pre-commit.ps1");

    if !hook_path.exists() {
        return Ok(());
    }

    let hook_display = hook_path.display().to_string();

    // In JSON/machine mode, capture hook output to prevent stdout/stderr pollution.
    // In human mode, inherit so the user sees hook output directly.
    let (stdout_cfg, stderr_cfg) = if output.is_json() {
        (Stdio::piped(), Stdio::piped())
    } else {
        (Stdio::inherit(), Stdio::inherit())
    };

    #[cfg(not(target_os = "windows"))]
    let hook_output = Command::new("sh")
        .arg(&hook_path)
        .current_dir(util::working_dir())
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .output()
        .map_err(|e| {
            CommitError::PreCommitHook(format!("failed to execute hook {hook_display}: {e}"))
        })?;

    #[cfg(target_os = "windows")]
    let hook_output = Command::new("powershell")
        .arg("-File")
        .arg(&hook_path)
        .current_dir(util::working_dir())
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .output()
        .map_err(|e| {
            CommitError::PreCommitHook(format!("failed to execute hook {hook_display}: {e}"))
        })?;

    if !hook_output.status.success() {
        return Err(CommitError::PreCommitHook(format!(
            "hook {hook_display} failed with exit code {}",
            hook_output.status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Save a commit object to storage.
fn save_commit_object(storage: &ClientStorage, commit: &Commit) -> Result<(), CommitError> {
    let data = commit
        .to_data()
        .map_err(|e| CommitError::ObjectStorage(format!("failed to serialize commit: {e}")))?;
    storage
        .put(&commit.id, &data, commit.get_type())
        .map_err(|e| CommitError::ObjectStorage(format!("failed to save commit: {e}")))?;
    Ok(())
}

/// Build a [`CommitOutput`] from the created commit and flags.
///
/// `user_message` is the commit message as provided by the user (before GPG
/// signature embedding), used to derive the `subject` field.
async fn build_commit_output(
    commit: &Commit,
    user_message: &str,
    staged_changes: &status::Changes,
    amend: bool,
    signoff: bool,
    conventional: Option<bool>,
    signed: bool,
) -> CommitOutput {
    let (head_label, branch) = match Head::current().await {
        Head::Branch(name) => (name.clone(), Some(name)),
        Head::Detached(_) => ("detached".to_string(), None),
    };

    let commit_str = commit.id.to_string();
    let short_id: String = commit_str.chars().take(7).collect();
    let subject = first_message_line(user_message);

    CommitOutput {
        head: head_label,
        branch,
        commit: commit_str,
        short_id,
        subject,
        root_commit: commit.parent_commit_ids.is_empty(),
        amend,
        files_changed: FilesChanged {
            total: staged_changes.new.len()
                + staged_changes.modified.len()
                + staged_changes.deleted.len(),
            new: staged_changes.new.len(),
            modified: staged_changes.modified.len(),
            deleted: staged_changes.deleted.len(),
        },
        signoff,
        conventional,
        signed,
    }
}

/// Render commit output according to OutputConfig (human / JSON / machine).
fn render_commit_output(result: &CommitOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("commit", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    if result.root_commit {
        writeln!(
            writer,
            "[{} (root-commit) {}] {}",
            result.head, result.short_id, result.subject
        )
        .map_err(|e| CliError::io(format!("failed to write commit summary: {e}")))?;
    } else {
        writeln!(
            writer,
            "[{} {}] {}",
            result.head, result.short_id, result.subject
        )
        .map_err(|e| CliError::io(format!("failed to write commit summary: {e}")))?;
    }

    let file_count = result.files_changed.total;
    if file_count > 0 {
        let files_word = if file_count == 1 { "file" } else { "files" };
        writeln!(
            writer,
            " {} {} changed (new: {}, modified: {}, deleted: {})",
            file_count,
            files_word,
            result.files_changed.new,
            result.files_changed.modified,
            result.files_changed.deleted
        )
        .map_err(|e| CliError::io(format!("failed to write commit summary: {e}")))?;
    }
    Ok(())
}

pub async fn execute(args: CommitArgs) {
    if let Err(error) = execute_safe(args, &OutputConfig::default()).await {
        error.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting.
///
/// # Side Effects
/// - Reads the index and staged objects to build a new tree and commit object.
/// - Resolves author/committer identity and optionally signs the commit through
///   the vault when signing is enabled.
/// - Writes new objects, updates HEAD/current branch, records reflog state, and
///   renders the requested success output.
///
/// # Errors
/// Returns [`CliError`] when the repository is missing or corrupt, there is
/// nothing to commit, identity/signing setup fails, object writes fail, or HEAD
/// cannot be updated.
pub async fn execute_safe(args: CommitArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_commit(args, output).await.map_err(CliError::from)?;
    render_commit_output(&result, output)?;
    dispatch_current_repo_vcs_event_to_history(VCS_EVENT_POST_COMMIT).await;
    Ok(())
}

/// If vault signing is enabled, sign the commit content and return the
/// formatted `gpgsig` header string. Returns `None` if vault is not configured.
/// Sign a commit with the vault PGP key.
///
/// Returns the `gpgsig` block, or `None` when signing is not enabled. When
/// `force` is true the `vault.signing` config gate is bypassed (used by
/// `merge --gpg-sign`/`-S`, which opts in explicitly); otherwise signing only
/// happens when `vault.signing=true`.
pub(crate) async fn vault_sign_commit(
    tree_id: &ObjectHash,
    parent_ids: &[ObjectHash],
    author: &Signature,
    committer: &Signature,
    message: &str,
    force: bool,
) -> Result<Option<String>, CommitError> {
    use crate::internal::{config::ConfigKv, vault};

    // Check if vault signing is enabled (unless explicitly forced).
    if !force {
        let signing_enabled = ConfigKv::get("vault.signing")
            .await
            .ok()
            .flatten()
            .map(|e| e.value);
        if signing_enabled.as_deref() != Some("true") {
            return Ok(None);
        }
    }

    // Load unseal key
    let unseal_key = vault::load_unseal_key().await.ok_or_else(|| {
        CommitError::VaultSign("vault signing enabled but no unseal key found".to_string())
    })?;

    // Build the commit content to sign (same format Git uses)
    let mut content: Vec<u8> = Vec::new();
    content.extend(b"tree ");
    content.extend(tree_id.to_string().as_bytes());
    content.extend(b"\n");
    for parent in parent_ids {
        content.extend(b"parent ");
        content.extend(parent.to_string().as_bytes());
        content.extend(b"\n");
    }
    let author_data = author.to_data().map_err(|e| {
        CommitError::VaultSign(format!(
            "failed to serialize author signature for vault signing: {e}"
        ))
    })?;
    content.extend(author_data);
    content.extend(b"\n");
    let committer_data = committer.to_data().map_err(|e| {
        CommitError::VaultSign(format!(
            "failed to serialize committer signature for vault signing: {e}"
        ))
    })?;
    content.extend(committer_data);
    content.extend(b"\n\n");
    content.extend(message.as_bytes());

    let root_dir = util::storage_path();

    let sig_hex = vault::pgp_sign(&root_dir, &unseal_key, &content)
        .await
        .map_err(|e| CommitError::VaultSign(format!("vault PGP signing failed: {e}")))?;
    let gpgsig = vault::signature_to_gpgsig(&sig_hex)
        .map_err(|e| CommitError::VaultSign(format!("failed to format PGP signature: {e}")))?;

    Ok(Some(gpgsig))
}

/// recursively create tree from index's tracked entries
pub async fn create_tree(
    index: &Index,
    storage: &ClientStorage,
    current_root: PathBuf,
) -> Result<Tree, CommitError> {
    // blob created when add file to index
    let get_blob_entry = |path: &PathBuf| -> Result<TreeItem, CommitError> {
        let name = util::path_to_string(path);
        let mete = index.get(&name, 0).ok_or_else(|| {
            CommitError::TreeCreation(format!("failed to get index entry for {}", name))
        })?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                CommitError::TreeCreation(format!("invalid filename in path: {:?}", path))
            })?
            .to_string();

        Ok(TreeItem {
            name: filename,
            mode: TreeItemMode::tree_item_type_from_bytes(format!("{:o}", mete.mode).as_bytes())
                .map_err(|e| {
                    CommitError::TreeCreation(format!("invalid mode for {}: {}", name, e))
                })?,
            id: mete.hash,
        })
    };

    let mut tree_items: Vec<TreeItem> = Vec::new();
    let mut processed_path: HashSet<String> = HashSet::new();
    let path_entries: Vec<PathBuf> = index
        .tracked_entries(0)
        .iter()
        .map(|file| PathBuf::from(file.name.clone()))
        .filter(|path| path.starts_with(&current_root))
        .collect();
    for path in path_entries.iter() {
        let in_current_path = path
            .parent()
            .ok_or_else(|| CommitError::TreeCreation(format!("invalid path: {:?}", path)))?
            == current_root;
        if in_current_path {
            let item = get_blob_entry(path)?;
            tree_items.push(item);
        } else {
            if path.components().count() == 1 {
                continue;
            }
            // next level tree
            let process_path = path
                .components()
                .nth(current_root.components().count())
                .ok_or_else(|| {
                    CommitError::TreeCreation("failed to get next path component".to_string())
                })?
                .as_os_str()
                .to_str()
                .ok_or_else(|| CommitError::TreeCreation("invalid path component".to_string()))?;

            if processed_path.contains(process_path) {
                continue;
            }
            processed_path.insert(process_path.to_string());

            let sub_tree = Box::pin(create_tree(
                index,
                storage,
                current_root.clone().join(process_path),
            ))
            .await?;
            tree_items.push(TreeItem {
                name: process_path.to_string(),
                mode: TreeItemMode::Tree,
                id: sub_tree.id,
            });
        }
    }
    crate::utils::tree::sort_tree_items_for_git(&mut tree_items);
    let tree = {
        // `from_tree_items` can't create empty tree, so use `from_bytes` instead
        if tree_items.is_empty() {
            let empty_id = ObjectHash::from_type_and_data(ObjectType::Tree, &[]);
            Tree::from_bytes(&[], empty_id).map_err(|e| {
                CommitError::TreeCreation(format!("failed to create empty tree: {}", e))
            })?
        } else {
            Tree::from_tree_items(tree_items).map_err(|e| {
                CommitError::TreeCreation(format!("failed to create tree from items: {}", e))
            })?
        }
    };
    // save
    save_object_to_storage(storage, &tree, &tree.id)
        .map_err(|e| CommitError::TreeCreation(format!("failed to save tree object: {}", e)))?;
    Ok(tree)
}

fn auto_stage_tracked_changes() -> Result<bool, CommitError> {
    let pending = status::changes_to_be_staged().map_err(|e| {
        CommitError::AutoStage(format!("failed to determine working tree status: {e}"))
    })?;
    if pending.modified.is_empty() && pending.deleted.is_empty() {
        return Ok(false);
    }

    let index_path = path::index();
    let mut index = Index::load(&index_path)
        .map_err(|e| CommitError::IndexLoad(format!("failed to load index: {}", e)))?;
    let workdir = util::working_dir();
    let mut touched = false;

    for file in pending.modified {
        let abs = util::workdir_to_absolute(&file);
        if !abs.exists() {
            continue;
        }
        // Refresh blob IDs for modified tracked files before updating the index
        let blob = blob_from_file(&abs);
        blob.save();
        index.update(
            IndexEntry::new_from_file(&file, blob.id, &workdir).map_err(|e| {
                CommitError::AutoStage(format!("failed to create index entry: {}", e))
            })?,
        );
        touched = true;
    }

    for file in pending.deleted {
        if let Some(path) = file.to_str() {
            // Drop entries that disappeared from the working tree
            index.remove(path, 0);
            touched = true;
        }
    }

    if touched {
        index
            .save(&index_path)
            .map_err(|e| CommitError::IndexSave(format!("failed to save index: {}", e)))?;
    }
    Ok(touched)
}

fn blob_from_file(path: impl AsRef<std::path::Path>) -> Blob {
    if lfs::is_lfs_tracked(&path) {
        Blob::from_lfs_file(path)
    } else {
        Blob::from_file(path)
    }
}

/// Get the current HEAD commit ID as parent.
///
/// If on a branch, returns the branch's commit ID; if detached HEAD, returns the HEAD commit ID.
async fn get_parents_ids() -> Vec<ObjectHash> {
    let current_commit_id = Head::current_commit().await;
    match current_commit_id {
        Some(id) => vec![id],
        None => vec![], // first commit
    }
}

/// Update HEAD to point to a new commit.
///
/// If on a branch, updates the branch's commit ID; if detached HEAD, updates the HEAD reference.
async fn update_head<C: ConnectionTrait>(db: &C, commit_id: &str) -> Result<(), CommitError> {
    match Head::current_with_conn(db).await {
        Head::Branch(name) => {
            Branch::update_branch_with_conn(db, &name, commit_id, None)
                .await
                .map_err(|e| {
                    CommitError::HeadUpdate(format!("failed to update branch '{name}': {e}"))
                })?;
        }
        Head::Detached(_) => {
            let head = Head::Detached(
                ObjectHash::from_str(commit_id)
                    .map_err(|e| CommitError::HeadUpdate(format!("invalid commit id: {e}")))?,
            );
            Head::update_with_conn(db, head, None).await;
        }
    }
    Ok(())
}

async fn update_head_and_reflog(commit_id: &str, commit_message: &str) -> Result<(), CommitError> {
    let reflog_context = new_reflog_context(commit_id, commit_message).await;
    let commit_id = commit_id.to_string();
    with_reflog(
        reflog_context,
        |txn| {
            Box::pin(async move {
                update_head(txn, &commit_id)
                    .await
                    .map_err(|e| sea_orm::DbErr::Custom(e.to_string()))
            })
        },
        true,
    )
    .await
    .map_err(|e| CommitError::HeadUpdate(format!("failed to update reflog: {}", e)))
}

async fn new_reflog_context(commit_id: &str, message: &str) -> ReflogContext {
    // INVARIANT: zero-filled bytes of the correct hash size always produce a valid ObjectHash
    let zero_hash =
        ObjectHash::from_bytes(&vec![0u8; get_hash_kind().size()]).expect("zero hash is valid");
    let old_oid = Head::current_commit()
        .await
        .unwrap_or(zero_hash)
        .to_string();
    let new_oid = commit_id.to_string();
    let action = ReflogAction::Commit {
        message: message.to_string(),
    };
    ReflogContext {
        old_oid,
        new_oid,
        action,
    }
}

#[cfg(test)]
mod test {
    use std::env;

    use git_internal::internal::object::{ObjectTrait, signature::Signature};
    use serial_test::serial;
    use tempfile::tempdir;
    use tokio::{fs::File, io::AsyncWriteExt};

    use super::*;
    use crate::utils::test::*;

    #[test]
    fn test_commit_error_nothing_to_commit_maps_to_repo_state() {
        let err: CliError = CommitError::NothingToCommit.into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-003");
        assert!(err.message().contains("nothing to commit"));
    }

    #[test]
    fn cleanup_strip_drops_comment_lines() {
        let msg = "subject\n\n# a comment\nbody  \n\n";
        let out = cleanup_message(msg, CleanupMode::Strip, true);
        assert_eq!(out, "subject\n\nbody");
        assert!(!out.contains('#'));
    }

    #[test]
    fn cleanup_whitespace_keeps_comments_trims_trailing() {
        let msg = "subject\n# keep me\ntrailing   ";
        let out = cleanup_message(msg, CleanupMode::Whitespace, false);
        assert_eq!(out, "subject\n# keep me\ntrailing");
        assert!(out.contains("# keep me"));
    }

    #[test]
    fn cleanup_verbatim_keeps_everything() {
        let msg = "subject\n# comment\n\n\nbody   \n";
        let out = cleanup_message(msg, CleanupMode::Verbatim, true);
        assert_eq!(out, msg);
    }

    #[test]
    fn cleanup_scissors_truncates_at_cut_line() {
        let msg = "subject\nbody\n# ------------------------ >8 ------------------------\ndiff --git a b\n+x";
        let out = cleanup_message(msg, CleanupMode::Scissors, true);
        assert_eq!(out, "subject\nbody");
        assert!(!out.contains("diff --git"));
    }

    #[test]
    fn cleanup_default_strips_when_edited_keeps_comments_otherwise() {
        let msg = "subject\n# comment";
        // Edited → strip (drops comments).
        assert_eq!(cleanup_message(msg, CleanupMode::Default, true), "subject");
        // Not edited → whitespace (keeps comments).
        assert_eq!(
            cleanup_message(msg, CleanupMode::Default, false),
            "subject\n# comment"
        );
    }

    /// Pin the `Display` format for the static-message and direct-message
    /// variants of [`CommitError`]. These strings are used as the
    /// `CliError` message via `From<CommitError> for CliError` and
    /// surface in both human and `--json` envelopes (visible to scripts
    /// reading exit codes and JSON error blobs).
    ///
    /// Source-chained / wrapper variants (IndexLoad, IndexSave,
    /// TreeCreation, ObjectStorage, ParentCommitLoad, HeadUpdate,
    /// PreCommitHook, VaultSign, AutoStage, StagedChanges,
    /// MessageFileRead) wrap upstream error strings via `{0}` /
    /// `{detail}` and are intentionally skipped — their content is
    /// owned by the wrapped error type.
    #[test]
    fn commit_error_display_pins_static_message_variants() {
        assert_eq!(
            CommitError::NothingToCommit.to_string(),
            "nothing to commit, working tree clean",
        );
        assert_eq!(
            CommitError::NothingToCommitNoTracked.to_string(),
            "nothing to commit (create/copy files and use 'libra add' to track)",
        );
        assert_eq!(
            CommitError::IdentityMissing("set user.name and user.email".to_string()).to_string(),
            "set user.name and user.email",
        );
        assert_eq!(
            CommitError::NoCommitToAmend.to_string(),
            "there is no commit to amend",
        );
        assert_eq!(
            CommitError::AmendUnsupported.to_string(),
            "amend is not supported for merge commits with multiple parents",
        );
        assert_eq!(
            CommitError::InvalidAuthor("missing '<email>'".to_string()).to_string(),
            "invalid author format: missing '<email>'",
        );
        assert_eq!(
            CommitError::EmptyMessage.to_string(),
            "aborting commit due to empty commit message",
        );
        assert_eq!(
            CommitError::ConventionalCommit("subject too long".to_string()).to_string(),
            "conventional commit validation failed: subject too long",
        );
    }

    #[test]
    fn test_commit_error_identity_missing_maps_to_auth() {
        let err: CliError =
            CommitError::IdentityMissing("author identity unknown".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-AUTH-001");
    }

    #[test]
    fn test_commit_error_no_commit_to_amend_maps_to_repo_state() {
        let err: CliError = CommitError::NoCommitToAmend.into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-003");
    }

    #[test]
    fn test_commit_error_amend_unsupported_maps_to_repo_state() {
        let err: CliError = CommitError::AmendUnsupported.into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-003");
    }

    #[test]
    fn test_commit_error_invalid_author_maps_to_cli_args() {
        let err: CliError = CommitError::InvalidAuthor("bad format".to_string()).into();
        assert_eq!(err.exit_code(), 129);
        assert_eq!(err.stable_code().as_str(), "LBR-CLI-002");
    }

    #[test]
    fn test_commit_error_tree_creation_maps_to_internal() {
        let err: CliError = CommitError::TreeCreation("unexpected".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-INTERNAL-001");
    }

    #[test]
    fn test_commit_error_conventional_maps_to_cli_args() {
        let err: CliError = CommitError::ConventionalCommit("bad format".to_string()).into();
        assert_eq!(err.exit_code(), 129);
        assert_eq!(err.stable_code().as_str(), "LBR-CLI-002");
    }

    #[test]
    fn test_commit_error_pre_commit_hook_maps_to_repo_state() {
        let err: CliError = CommitError::PreCommitHook("hook failed".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-003");
    }

    #[test]
    fn test_commit_error_vault_sign_maps_to_auth() {
        let err: CliError = CommitError::VaultSign("no key".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-AUTH-001");
    }

    #[test]
    fn test_commit_error_index_load_maps_to_repo_corrupt() {
        let err: CliError = CommitError::IndexLoad("corrupted".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-002");
    }

    #[test]
    fn test_commit_error_object_storage_maps_to_io_write() {
        let err: CliError = CommitError::ObjectStorage("disk full".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-IO-002");
    }

    #[test]
    fn test_commit_error_parent_commit_load_maps_to_repo_corrupt() {
        let err: CliError = CommitError::ParentCommitLoad {
            commit_id: "abc1234".to_string(),
            detail: "missing object".to_string(),
        }
        .into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-002");
    }

    #[test]
    fn test_commit_error_empty_message_maps_to_repo_state() {
        let err: CliError = CommitError::EmptyMessage.into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-003");
    }

    #[test]
    fn test_commit_error_nothing_to_commit_no_tracked_maps_to_repo_state() {
        let err: CliError = CommitError::NothingToCommitNoTracked.into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-003");
    }

    #[test]
    fn test_commit_error_index_save_maps_to_io_write() {
        let err: CliError = CommitError::IndexSave("disk full".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-IO-002");
    }

    #[test]
    fn test_commit_error_message_file_read_maps_to_io_read() {
        let err: CliError = CommitError::MessageFileRead {
            path: "msg.txt".to_string(),
            detail: "not found".to_string(),
        }
        .into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-IO-001");
    }

    #[test]
    fn test_commit_error_auto_stage_maps_to_io_read() {
        let err: CliError = CommitError::AutoStage("failed".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-IO-001");
    }

    #[test]
    fn test_commit_error_staged_changes_maps_to_repo_corrupt() {
        let err: CliError = CommitError::StagedChanges("failed".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-REPO-002");
    }

    #[test]
    fn test_commit_error_head_update_maps_to_io_write() {
        let err: CliError = CommitError::HeadUpdate("failed".to_string()).into();
        assert_eq!(err.exit_code(), 128);
        assert_eq!(err.stable_code().as_str(), "LBR-IO-002");
    }

    #[test]
    ///Testing basic parameter parsing functionality.
    fn test_parse_args() {
        let args = CommitArgs::try_parse_from(["commit", "-m", "init"]);
        assert!(args.is_ok());

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--allow-empty"]);
        assert!(args.is_ok());

        let args = CommitArgs::try_parse_from(["commit", "--conventional", "-m", "init"]);
        assert!(args.is_ok());

        let args = CommitArgs::try_parse_from(["commit", "--conventional"]);
        assert!(args.is_err(), "conventional should require message");

        let args = CommitArgs::try_parse_from(["commit"]);
        assert!(args.is_err(), "message is required");

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend"]);
        assert!(args.is_ok());
        //failed
        let args = CommitArgs::try_parse_from(["commit", "--amend", "--no-edit"]);
        assert!(args.is_ok());
        let args = CommitArgs::try_parse_from(["commit", "--no-edit"]);
        assert!(args.is_err(), "--no-edit requires --amend");
        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--allow-empty", "--amend"]);
        assert!(args.is_ok());
        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "-s"]);
        assert!(args.is_ok());
        assert!(args.unwrap().signoff);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--signoff"]);
        assert!(args.is_ok());
        assert!(args.unwrap().signoff);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "-a"]);
        assert!(args.is_ok());
        assert!(args.unwrap().all);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--all"]);
        assert!(args.is_ok());
        assert!(args.unwrap().all);

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend", "--no-edit"]);
        assert!(
            args.is_err(),
            "--no-edit conflicts with --message and --file"
        );
        let args = CommitArgs::try_parse_from(["commit", "-F", "init", "--amend", "--no-edit"]);
        assert!(
            args.is_err(),
            "--no-edit conflicts with --message and --file"
        );
        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend", "--signoff"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        assert!(args.amend);
        assert!(args.signoff);

        let args = CommitArgs::try_parse_from(["commit", "-F", "unreachable_file"]);
        assert!(args.is_ok());
        assert!(args.unwrap().file.is_some());

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--no-verify"]);
        assert!(args.is_ok(), "--no-verify should be a valid parameter");

        let args =
            CommitArgs::try_parse_from(["commit", "-m", "init", "--conventional", "--no-verify"]);
        assert!(args.is_ok(), "--no-verify should work with --conventional");

        let args = CommitArgs::try_parse_from(["commit", "-m", "init", "--amend", "--no-verify"]);
        assert!(args.is_ok(), "--no-verify should work with --amend");

        let args = CommitArgs::try_parse_from([
            "commit",
            "-m",
            "init",
            "--author",
            "Test User <test@example.com>",
        ]);
        assert!(args.is_ok(), "--author should be a valid parameter");
        let args = args.unwrap();
        assert_eq!(
            args.author,
            Some("Test User <test@example.com>".to_string())
        );

        let args = CommitArgs::try_parse_from([
            "commit",
            "-m",
            "init",
            "--author",
            "Test User <test@example.com>",
            "--amend",
        ]);
        assert!(args.is_ok(), "--author should work with --amend");
    }

    #[test]
    fn test_parse_author() {
        // Valid author formats
        let (name, email) = parse_author("John Doe <john@example.com>").unwrap();
        assert_eq!(name, "John Doe");
        assert_eq!(email, "john@example.com");

        let (name, email) = parse_author("  Jane Smith  <jane@test.org>  ").unwrap();
        assert_eq!(name, "Jane Smith");
        assert_eq!(email, "jane@test.org");

        let (name, email) = parse_author("Multi Word Name <multi@word.com>").unwrap();
        assert_eq!(name, "Multi Word Name");
        assert_eq!(email, "multi@word.com");

        // Invalid formats should return CommitError::InvalidAuthor
        assert!(matches!(
            parse_author("invalid"),
            Err(CommitError::InvalidAuthor(_))
        ));
        assert!(matches!(
            parse_author("No Email"),
            Err(CommitError::InvalidAuthor(_))
        ));
        assert!(matches!(
            parse_author("<noemail@test.com>"),
            Err(CommitError::InvalidAuthor(_))
        ));
        assert!(matches!(
            parse_author("Name <"),
            Err(CommitError::InvalidAuthor(_))
        ));
    }

    #[test]
    fn test_commit_message() {
        let args = CommitArgs {
            message: None,
            file: None,
            allow_empty: false,
            conventional: false,
            amend: true,
            no_edit: true,
            signoff: false,
            disable_pre: false,
            all: false,
            no_verify: false,
            author: None,
            ..Default::default()
        };
        fn message_and_file_are_none(args: &CommitArgs) -> Option<String> {
            match (&args.message, &args.file) {
                (Some(msg), _) => Some(msg.clone()),
                (None, Some(file)) => Some(file.clone()),
                (None, None) => {
                    if args.no_edit {
                        Some("".to_string())
                    } else {
                        None
                    }
                }
            }
        }
        let message = message_and_file_are_none(&args);
        assert_eq!(message, Some("".to_string()));
    }

    #[tokio::test]
    #[serial]
    async fn test_commit_message_from_file() {
        let temp_dir = tempdir().unwrap();
        let test_path = temp_dir.path().join("test_data.txt");

        let test_cases = vec![
            "Hello, World! 你好，世界！",
            "Special chars: \n\t\r\\\"'",
            "Emoji: 😀🎉🚀, Unicode:  Café café",
            "",
            "Mix: 中文\n\tEmoji😀\rSpecial\\\"'",
        ];

        for test_data in test_cases {
            let bytes = test_data.as_bytes();
            let mut file = File::create(&test_path).await.expect("create file failed");
            file.write_all(bytes)
                .await
                .expect("write test data to file failed");
            file.sync_all()
                .await
                .expect("write test data to file failed");

            let content = tokio::fs::read_to_string(&test_path).await.unwrap();

            let author = Signature {
                signature_type: git_internal::internal::object::signature::SignatureType::Author,
                name: "test".to_string(),
                email: "test".to_string(),
                timestamp: 1,
                timezone: "test".to_string(),
            };

            let commiter = Signature {
                signature_type: git_internal::internal::object::signature::SignatureType::Committer,
                name: "test".to_string(),
                email: "test".to_string(),
                timestamp: 1,
                timezone: "test".to_string(),
            };

            let zero = ObjectHash::from_bytes(&vec![0u8; get_hash_kind().size()]).unwrap();
            let commit = Commit::new(author, commiter, zero, Vec::new(), &content);

            let commit_data = commit.to_data().unwrap();

            let message = Commit::from_bytes(&commit_data, commit.id).unwrap().message;

            assert_eq!(message, test_data);
        }
    }

    #[tokio::test]
    #[serial]
    // Tests the recursive tree creation from index entries (uses original test data via absolute path)
    async fn test_create_tree() {
        // 1. Initialize a temporary Libra repository
        let temp_path = tempdir().unwrap();
        setup_with_new_libra_in(temp_path.path()).await;
        let _guard = ChangeDirGuard::new(temp_path.path());

        // 2. Build absolute path to the test index file using the project root (CARGO_MANIFEST_DIR)
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let index_file_path = project_root.join("tests/data/index/index-760");

        // 3. Verify the test fixture exists
        assert!(
            index_file_path.exists(),
            "test fixture not found: {}; please place the index-760 file at that path",
            index_file_path.display()
        );

        // 4. Load the index file
        let index = Index::from_file(index_file_path).unwrap_or_else(|e| {
            panic!(
                "failed to load index file: {}; verify the file format is correct",
                e
            );
        });
        println!(
            "loaded index contains {} tracked entries",
            index.tracked_entries(0).len()
        );

        // 5. Initialize storage pointing at the temp repo's objects directory
        let temp_objects_dir = temp_path.path().join(".libra/objects");
        let storage = ClientStorage::init(temp_objects_dir);

        // 6. Call create_tree with an empty root (index paths are repo-root-relative)
        let tree = create_tree(&index, &storage, PathBuf::new()).await.unwrap();

        // 7. Verify tree structure
        assert!(
            storage.get(&tree.id).is_ok(),
            "root tree not saved to storage"
        );
        for item in tree.tree_items.iter() {
            if item.mode == TreeItemMode::Tree {
                assert!(
                    storage.get(&item.id).is_ok(),
                    "sub-tree not saved: {}",
                    item.name
                );
                if item.name == "DeveloperExperience" {
                    let sub_tree_data = storage.get(&item.id).unwrap();
                    let sub_tree = Tree::from_bytes(&sub_tree_data, item.id).unwrap();
                    assert_eq!(
                        sub_tree.tree_items.len(),
                        4,
                        "DeveloperExperience sub-tree entry count mismatch"
                    );
                }
            }
        }
    }

    #[test]
    fn test_no_verify_skips_conventional_check() {
        let invalid_conventional_msg = "invalid commit: no type or scope";
        assert!(
            !check_conventional_commits_message(invalid_conventional_msg),
            "Test setup error: message should be invalid for conventional commits"
        );

        let args_with_verify = CommitArgs {
            message: Some(invalid_conventional_msg.to_string()),
            file: None,
            allow_empty: true,
            conventional: true,
            no_verify: false,
            amend: false,
            no_edit: false,
            signoff: false,
            disable_pre: false,
            all: false,
            author: None,
            ..Default::default()
        };

        let commit_message_with_verify = if args_with_verify.signoff {
            format!(
                "{}\n\nSigned-off-by: test <test@example.com>",
                invalid_conventional_msg
            )
        } else {
            invalid_conventional_msg.to_string()
        };

        let verify_result = std::panic::catch_unwind(|| {
            if args_with_verify.conventional
                && !args_with_verify.no_verify
                && !check_conventional_commits_message(&commit_message_with_verify)
            {
                panic!("fatal: commit message does not follow conventional commits");
            }
        });
        assert!(
            verify_result.is_err(),
            "Conventional check should fail without --no-verify"
        );

        let args_no_verify = CommitArgs {
            no_verify: true,
            ..args_with_verify
        };

        let commit_message_no_verify = if args_no_verify.signoff {
            format!(
                "{}\n\nSigned-off-by: test <test@example.com>",
                invalid_conventional_msg
            )
        } else {
            invalid_conventional_msg.to_string()
        };

        let no_verify_result = std::panic::catch_unwind(|| {
            if args_no_verify.conventional
                && !args_no_verify.no_verify
                && !check_conventional_commits_message(&commit_message_no_verify)
            {
                panic!("fatal: commit message does not follow conventional commits");
            }
        });
        assert!(
            no_verify_result.is_ok(),
            "--no-verify should skip conventional check"
        );
    }

    /// Cross-Cutting G: `TreeCreation` is the lone CommitError variant
    /// that maps to `InternalInvariant`. It must include the GitHub
    /// Issues URL hint so users can report the bug.
    #[test]
    fn test_commit_error_tree_creation_has_issue_url_hint() {
        let err: CliError =
            CommitError::TreeCreation("synthetic tree-build failure".to_string()).into();
        assert_eq!(err.stable_code(), StableErrorCode::InternalInvariant);
        assert!(
            err.hints().iter().any(|h| h.as_str().contains("issues")),
            "TreeCreation must include the GitHub Issues URL hint, got hints: {:?}",
            err.hints()
        );
    }
}
