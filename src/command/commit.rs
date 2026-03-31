//! Commit command that collects staged changes, builds tree and commit objects, validates messages (including GPG), and updates HEAD/refs.

use std::{
    collections::HashSet,
    io::Write,
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
    command::{load_object, status},
    common_utils::{check_conventional_commits_message, format_commit_msg},
    internal::{
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
/// # Examples
///
/// ```text
/// libra commit -m "Add new feature"          Create a commit with message
/// libra commit -m "feat: add login" --conventional  Validate conventional commit format
/// libra commit --amend                       Amend the last commit
/// libra commit --amend --no-edit             Amend without changing the message
/// libra commit -a -m "Fix typo"              Auto-stage tracked changes and commit
/// libra commit -F message.txt                Read commit message from file
/// libra commit -s -m "Add feature"           Add Signed-off-by trailer
/// libra commit --allow-empty -m "Trigger CI" Create an empty commit
/// libra commit --json -m "Add feature"       Structured JSON output for agents
/// ```
#[derive(Parser, Debug, Default)]
pub struct CommitArgs {
    #[arg(short, long, required_unless_present_any(["file", "no_edit"]))]
    pub message: Option<String>,

    /// read message from file
    #[arg(short = 'F', long, required_unless_present_any(["message", "no_edit"]))]
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

    /// use the message from the original commit when amending
    #[arg(long, requires = "amend",conflicts_with_all(["message", "file"]))]
    pub no_edit: bool,
    /// add signed-off-by line at the end of the commit message
    #[arg(short = 's', long)]
    pub signoff: bool,

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
                .with_stable_code(StableErrorCode::InternalInvariant),
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
pub async fn run_commit(
    args: CommitArgs,
    output: &OutputConfig,
) -> Result<CommitOutput, CommitError> {
    let is_amend = args.amend;
    let is_signoff = args.signoff;
    let is_conventional = args.conventional;
    let skip_hooks = args.disable_pre || args.no_verify;
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

    // Run pre-commit hook
    if !skip_hooks {
        run_pre_commit_hook(output)?;
    }

    // Resolve commit message
    let message = match (args.message, args.file) {
        (Some(msg), _) => msg,
        (None, Some(file_path)) => tokio::fs::read_to_string(&file_path).await.map_err(|e| {
            CommitError::MessageFileRead {
                path: file_path,
                detail: e.to_string(),
            }
        })?,
        (None, None) => {
            if !args.no_edit {
                return Err(CommitError::EmptyMessage);
            }
            // --no-edit with --amend: message comes from parent commit below
            String::new()
        }
    };

    // Create tree
    let tree = create_tree(&index, &storage, "".into()).await?;

    // Resolve parent commits
    let parents_commit_ids = get_parents_ids().await;

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

    // Amend path
    if is_amend {
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
        let grandpa_commit_id = parent_commit.parent_commit_ids;

        let final_message = if args.no_edit {
            parent_commit.message.clone()
        } else {
            message.clone()
        };

        let commit_message = match &signoff_line {
            Some(line) => format!("{final_message}\n\n{line}"),
            None => final_message.clone(),
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

        let gpg_sig = vault_sign_commit(
            &tree.id,
            &grandpa_commit_id,
            &author,
            &committer,
            &commit_message,
        )
        .await?;

        let commit = Commit::new(
            author,
            committer,
            tree.id,
            grandpa_commit_id,
            &format_commit_msg(&commit_message, gpg_sig.as_deref()),
        );

        save_commit_object(&storage, &commit)?;
        update_head_and_reflog(&commit.id.to_string(), &commit_message).await?;

        let conventional_result = if is_conventional && !skip_conventional_check {
            Some(true)
        } else {
            None
        };
        return Ok(build_commit_output(
            &commit,
            &commit_message,
            &staged_changes,
            is_amend,
            is_signoff,
            conventional_result,
            gpg_sig.is_some(),
        )
        .await);
    }

    // Normal (non-amend) path
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

    let gpg_sig = vault_sign_commit(
        &tree.id,
        &parents_commit_ids,
        &author,
        &committer,
        &commit_message,
    )
    .await?;

    let commit = Commit::new(
        author,
        committer,
        tree.id,
        parents_commit_ids,
        &format_commit_msg(&commit_message, gpg_sig.as_deref()),
    );

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
/// errors and exiting. Collects staged changes, resolves committer identity,
/// builds tree and commit objects, and updates HEAD.
pub async fn execute_safe(args: CommitArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_commit(args, output).await.map_err(CliError::from)?;
    render_commit_output(&result, output)
}

/// If vault signing is enabled, sign the commit content and return the
/// formatted `gpgsig` header string. Returns `None` if vault is not configured.
async fn vault_sign_commit(
    tree_id: &ObjectHash,
    parent_ids: &[ObjectHash],
    author: &Signature,
    committer: &Signature,
    message: &str,
) -> Result<Option<String>, CommitError> {
    use crate::internal::{config::ConfigKv, vault};

    // Check if vault signing is enabled
    let signing_enabled = ConfigKv::get("vault.signing")
        .await
        .ok()
        .flatten()
        .map(|e| e.value);
    if signing_enabled.as_deref() != Some("true") {
        return Ok(None);
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
    crate::command::save_object_to_storage(storage, &tree, &tree.id)
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
}
