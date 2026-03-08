//! Commit command that collects staged changes, builds tree and commit objects, validates messages (including GPG), and updates HEAD/refs.

use std::{
    collections::HashSet,
    env,
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

use super::save_object;
use crate::{
    command::{
        config::{ConfigScope, ScopedConfig},
        load_object, status,
    },
    common_utils::{check_conventional_commits_message, format_commit_msg},
    internal::{
        branch::Branch,
        head::Head,
        reflog::{ReflogAction, ReflogContext, with_reflog},
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult},
        lfs,
        object_ext::BlobExt,
        path, util,
    },
};

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

/// Parse author string in format "Name <email>" and return (name, email)
/// If parsing fails, return an error message
fn parse_author(author: &str) -> Result<(String, String), String> {
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

    Err(format!(
        "fatal: invalid author format '{}'. Expected format: 'Name <email>'",
        author
    ))
}

/// A user's name + email pair used for commit authoring and committing.
#[derive(Clone, Debug)]
struct UserIdentity {
    name: String,
    email: String,
}

/// Internal error type that bridges legacy `String` errors from `execute_impl`
/// with the structured `CliError` type. Converted via `into_cli()` at the
/// `execute_safe` boundary.
#[derive(Debug)]
enum CommitExecError {
    Cli(CliError),
    Message(String),
}

impl From<CliError> for CommitExecError {
    fn from(value: CliError) -> Self {
        Self::Cli(value)
    }
}

impl From<String> for CommitExecError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl CommitExecError {
    fn into_cli(self) -> CliError {
        match self {
            Self::Cli(error) => error,
            Self::Message(message) => classify_commit_error(message),
        }
    }
}

async fn get_user_config_value(key: &str) -> Option<String> {
    for scope in ConfigScope::CASCADE_ORDER {
        if scope != ConfigScope::Local {
            let Some(config_path) = scope.get_config_path() else {
                continue;
            };
            if !config_path.exists() {
                continue;
            }
        }

        if let Ok(Some(value)) = ScopedConfig::get(scope, "user", None, key).await {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

fn env_first_non_empty(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

fn missing_identity_error(name_missing: bool, email_missing: bool) -> CliError {
    let config_hint = match (name_missing, email_missing) {
        (true, true) => {
            "run 'libra config --global user.name \"Your Name\"' and 'libra config --global user.email \"you@example.com\"'."
        }
        (true, false) => {
            "run 'libra config --global user.name \"Your Name\"' to set your default identity."
        }
        (false, true) => {
            "run 'libra config --global user.email \"you@example.com\"' to set your default identity."
        }
        (false, false) => {
            "run 'libra config --global --edit' to inspect your identity configuration."
        }
    };

    CliError::fatal("author identity unknown")
        .with_hint(config_hint)
        .with_hint("omit '--global' to set the identity only in this repository.")
}

fn classify_commit_error(message: String) -> CliError {
    if message == "nothing to commit, working tree clean" {
        return CliError::failure(message);
    }
    if let Some(message) = message.strip_prefix("fatal: ") {
        return CliError::fatal(message);
    }
    if let Some(message) = message.strip_prefix("error: ") {
        return CliError::failure(message);
    }
    CliError::fatal(message)
}

async fn resolve_committer_identity() -> Result<UserIdentity, CliError> {
    // Step 1: check libra config (highest precedence after explicit --author)
    let config_name = get_user_config_value("name").await;
    let config_email = get_user_config_value("email").await;

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
        if let (Some(name), Some(email)) = (config_name.clone(), config_email.clone()) {
            return Ok(UserIdentity { name, email });
        }
        // Report which field(s) are missing — using *config-only* perspective.
        // Reuse the already-fetched values instead of querying config again.
        let name_missing = config_name.is_none();
        let email_missing = config_email.is_none();
        return Err(missing_identity_error(name_missing, email_missing));
    }

    // Step 3: env-var fallback (GIT_COMMITTER_*, GIT_AUTHOR_*, EMAIL, LIBRA_COMMITTER_*)
    let name = config_name.or_else(|| {
        env_first_non_empty(&[
            "GIT_COMMITTER_NAME",
            "GIT_AUTHOR_NAME",
            "LIBRA_COMMITTER_NAME",
        ])
    });
    let email = config_email.or_else(|| {
        env_first_non_empty(&[
            "GIT_COMMITTER_EMAIL",
            "GIT_AUTHOR_EMAIL",
            "EMAIL",
            "LIBRA_COMMITTER_EMAIL",
        ])
    });

    if let (Some(name), Some(email)) = (name.clone(), email.clone()) {
        return Ok(UserIdentity { name, email });
    }

    Err(missing_identity_error(name.is_none(), email.is_none()))
}

/// Create author and committer signatures based on the provided arguments
async fn create_commit_signatures(
    author_override: Option<&str>,
) -> Result<(Signature, Signature, UserIdentity), CommitExecError> {
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

async fn print_commit_summary(commit: &Commit, message: &str, staged_changes: &status::Changes) {
    let head_label = match Head::current().await {
        Head::Branch(branch) => branch,
        Head::Detached(_) => "detached".to_string(),
    };

    let commit_str = commit.id.to_string();
    let short_id: String = commit_str.chars().take(7).collect();
    let subject = first_message_line(message);

    if commit.parent_commit_ids.is_empty() {
        println!("[{} (root-commit) {}] {}", head_label, short_id, subject);
    } else {
        println!("[{} {}] {}", head_label, short_id, subject);
    }

    let file_count =
        staged_changes.new.len() + staged_changes.modified.len() + staged_changes.deleted.len();
    if file_count > 0 {
        let files_word = if file_count == 1 { "file" } else { "files" };
        println!(
            " {} {} changed (new: {}, modified: {}, deleted: {})",
            file_count,
            files_word,
            staged_changes.new.len(),
            staged_changes.modified.len(),
            staged_changes.deleted.len()
        );
    }
}

async fn execute_impl(args: CommitArgs) -> Result<(), CommitExecError> {
    /* check args */
    let auto_stage_applied = if args.all {
        // Mimic `git commit -a` by staging tracked modifications/deletions first
        auto_stage_tracked_changes()?
    } else {
        false
    };
    let index = Index::load(path::index()).map_err(|e| format!("failed to load index: {}", e))?;
    let storage = ClientStorage::init(path::objects());
    let tracked_entries = index.tracked_entries(0);
    // Skip empty commit check for --amend operations (allowed to modify message/author without changes)
    if tracked_entries.is_empty() && !args.allow_empty && !args.amend && !auto_stage_applied {
        return Err("nothing to commit, working tree clean".to_string().into());
    }

    // Additional check: verify if there are any staged changes relative to HEAD
    // Skip this check for --amend operations
    let staged_changes = status::changes_to_be_committed().await;
    if staged_changes.is_empty() && !args.allow_empty && !args.amend {
        return Err("nothing to commit, working tree clean".to_string().into());
    }

    // run pre commit hook
    if !args.disable_pre && !args.no_verify {
        let hooks_dir = path::hooks();

        #[cfg(not(target_os = "windows"))]
        let hook_path = hooks_dir.join("pre-commit.sh");

        #[cfg(target_os = "windows")]
        let hook_path = hooks_dir.join("pre-commit.ps1");
        if hook_path.exists() {
            let hook_display = hook_path.display();
            #[cfg(not(target_os = "windows"))]
            let output = Command::new("sh")
                .arg(&hook_path)
                .current_dir(util::working_dir())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()
                .map_err(|e| format!("Failed to execute hook {hook_display}: {e}"))?;

            #[cfg(target_os = "windows")]
            let output = Command::new("powershell")
                .arg("-File")
                .arg(&hook_path)
                .current_dir(util::working_dir())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()
                .map_err(|e| format!("Failed to execute hook {hook_display}: {e}"))?;

            if !output.status.success() {
                return Err(format!(
                    "Hook {} failed with exit code {}",
                    hook_display,
                    output.status.code().unwrap_or(-1)
                )
                .into());
            }
        }
    }

    //Find commit message source
    let message = match (args.message, args.file) {
        //from -m
        (Some(msg), _) => msg,
        //from file
        (None, Some(file_path)) => match tokio::fs::read_to_string(file_path).await {
            Ok(msg) => msg,
            Err(e) => {
                return Err(
                    format!("fatal: failed to read commit message from file: {}", e).into(),
                );
            }
        },
        //no commit message, which is not supposed to happen
        (None, None) => {
            if !args.no_edit {
                return Err("fatal: no commit message provided".to_string().into());
            } else {
                //its ok to use "" because no_edit is True ,
                //and we will use the message from the original commit
                // message wont be used by amend
                "".to_string()
            }
        }
    };
    /* Create tree */
    let tree = create_tree(&index, &storage, "".into()).await?;

    /* Create & save commit objects */
    let parents_commit_ids = get_parents_ids().await;

    // Create author and committer signatures (respecting --author override)
    let (author, committer, committer_identity) =
        create_commit_signatures(args.author.as_deref()).await?;

    // Amend commits are only supported for a single parent commit.
    if args.amend {
        if parents_commit_ids.len() > 1 {
            return Err(
                "fatal: --amend is not supported for merge commits with multiple parents"
                    .to_string()
                    .into(),
            );
        }
        let parent_commit = load_object::<Commit>(&parents_commit_ids[0]).map_err(|_| {
            format!(
                "fatal: not a valid object name: '{}'",
                parents_commit_ids[0]
            )
        })?;
        let grandpa_commit_id = parent_commit.parent_commit_ids;
        // if no_edit is True, use parent commit message;else use commit message from args
        let final_message = if args.no_edit {
            parent_commit.message.clone()
        } else {
            message.clone()
        };
        //Prepare commit message
        let commit_message = if args.signoff {
            // get sign line
            let signoff_line = format!(
                "Signed-off-by: {} <{}>",
                committer_identity.name, committer_identity.email
            );
            format!("{}\n\n{signoff_line}", final_message)
        } else {
            final_message.clone()
        };

        // check format(if needed)
        if args.conventional
            && !args.no_verify
            && !check_conventional_commits_message(&commit_message)
        {
            return Err("fatal: commit message does not follow conventional commits"
                .to_string()
                .into());
        }
        let commit = Commit::new(
            author,
            committer,
            tree.id,
            grandpa_commit_id,
            &format_commit_msg(&final_message, None),
        );

        storage
            .put(
                &commit.id,
                &commit
                    .to_data()
                    .map_err(|e| format!("failed to serialize commit: {}", e))?,
                commit.get_type(),
            )
            .map_err(|e| format!("failed to save commit: {}", e))?;

        /* update HEAD */
        update_head_and_reflog(&commit.id.to_string(), &commit_message).await?;
        print_commit_summary(&commit, &commit_message, &staged_changes).await;
        return Ok(());
    }

    //Prepare commit message
    let commit_message = if args.signoff {
        // get sign line
        let signoff_line = format!(
            "Signed-off-by: {} <{}>",
            committer_identity.name, committer_identity.email
        );
        format!("{}\n\n{signoff_line}", message)
    } else {
        message.clone()
    };

    // check format(if needed)
    if args.conventional && !args.no_verify && !check_conventional_commits_message(&commit_message)
    {
        return Err("fatal: commit message does not follow conventional commits"
            .to_string()
            .into());
    }

    // There must be a `blank line`(\n) before `message`, or remote unpack failed
    let commit = Commit::new(
        author,
        committer,
        tree.id,
        parents_commit_ids,
        &format_commit_msg(&message, None),
    );

    storage
        .put(
            &commit.id,
            &commit
                .to_data()
                .map_err(|e| format!("failed to serialize commit: {}", e))?,
            commit.get_type(),
        )
        .map_err(|e| format!("failed to save commit: {}", e))?;

    /* update HEAD */
    update_head_and_reflog(&commit.id.to_string(), &commit_message).await?;
    print_commit_summary(&commit, &commit_message, &staged_changes).await;
    Ok(())
}

pub async fn execute(args: CommitArgs) {
    if let Err(error) = execute_safe(args).await {
        eprintln!("{}", error.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Collects staged changes, resolves committer identity,
/// builds tree and commit objects, and updates HEAD.
pub async fn execute_safe(args: CommitArgs) -> CliResult<()> {
    execute_impl(args).await.map_err(CommitExecError::into_cli)
}

/// recursively create tree from index's tracked entries
pub async fn create_tree(
    index: &Index,
    storage: &ClientStorage,
    current_root: PathBuf,
) -> Result<Tree, String> {
    // blob created when add file to index
    let get_blob_entry = |path: &PathBuf| -> Result<TreeItem, String> {
        let name = util::path_to_string(path);
        let mete = index
            .get(&name, 0)
            .ok_or_else(|| format!("failed to get index entry for {}", name))?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("invalid filename in path: {:?}", path))?
            .to_string();

        Ok(TreeItem {
            name: filename,
            mode: TreeItemMode::tree_item_type_from_bytes(format!("{:o}", mete.mode).as_bytes())
                .map_err(|e| format!("invalid mode for {}: {}", name, e))?,
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
            .ok_or_else(|| format!("invalid path: {:?}", path))?
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
                .ok_or_else(|| "failed to get next path component".to_string())?
                .as_os_str()
                .to_str()
                .ok_or_else(|| "invalid path component".to_string())?;

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
            Tree::from_bytes(&[], empty_id)
                .map_err(|e| format!("failed to create empty tree: {}", e))?
        } else {
            Tree::from_tree_items(tree_items)
                .map_err(|e| format!("failed to create tree from items: {}", e))?
        }
    };
    // save
    save_object(&tree, &tree.id).map_err(|e| format!("failed to save tree object: {}", e))?;
    Ok(tree)
}

fn auto_stage_tracked_changes() -> Result<bool, String> {
    let pending = status::changes_to_be_staged()
        .map_err(|e| format!("failed to determine working tree status: {e}"))?;
    if pending.modified.is_empty() && pending.deleted.is_empty() {
        return Ok(false);
    }

    let index_path = path::index();
    let mut index = Index::load(&index_path).map_err(|e| format!("failed to load index: {}", e))?;
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
            IndexEntry::new_from_file(&file, blob.id, &workdir)
                .map_err(|e| format!("failed to create index entry: {}", e))?,
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
            .map_err(|e| format!("failed to save index: {}", e))?;
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
async fn update_head<C: ConnectionTrait>(db: &C, commit_id: &str) -> Result<(), String> {
    match Head::current_with_conn(db).await {
        Head::Branch(name) => {
            Branch::update_branch_with_conn(db, &name, commit_id, None).await;
        }
        Head::Detached(_) => {
            let head = Head::Detached(
                ObjectHash::from_str(commit_id).map_err(|e| format!("invalid commit id: {}", e))?,
            );
            Head::update_with_conn(db, head, None).await;
        }
    }
    Ok(())
}

async fn update_head_and_reflog(commit_id: &str, commit_message: &str) -> Result<(), String> {
    let reflog_context = new_reflog_context(commit_id, commit_message).await;
    let commit_id = commit_id.to_string();
    with_reflog(
        reflog_context,
        |txn| {
            Box::pin(async move {
                update_head(txn, &commit_id)
                    .await
                    .map_err(sea_orm::DbErr::Custom)
            })
        },
        true,
    )
    .await
    .map_err(|e| format!("failed to update reflog: {}", e))
}

async fn new_reflog_context(commit_id: &str, message: &str) -> ReflogContext {
    let old_oid = Head::current_commit()
        .await
        .unwrap_or(ObjectHash::from_bytes(&vec![0u8; get_hash_kind().size()]).unwrap())
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
    fn test_classify_commit_error_nothing_to_commit() {
        let err = classify_commit_error("nothing to commit, working tree clean".to_string());
        assert_eq!(
            err.exit_code(),
            1,
            "nothing-to-commit is a non-fatal failure"
        );
        assert!(
            err.message().contains("nothing to commit"),
            "message should be preserved: {}",
            err.message()
        );
    }

    #[test]
    fn test_classify_commit_error_fatal_prefix() {
        let err = classify_commit_error("fatal: could not read tree".to_string());
        assert_eq!(err.exit_code(), 128, "fatal prefix should map to exit 128");
        assert!(
            err.message().contains("could not read tree"),
            "message should strip prefix: {}",
            err.message()
        );
    }

    #[test]
    fn test_classify_commit_error_error_prefix() {
        let err = classify_commit_error("error: pathspec 'x' did not match any file".to_string());
        assert_eq!(err.exit_code(), 1, "error prefix should map to exit 1");
        assert!(
            err.message().contains("pathspec"),
            "message should strip prefix: {}",
            err.message()
        );
    }

    #[test]
    fn test_classify_commit_error_unknown_prefix() {
        let err = classify_commit_error("some unexpected message".to_string());
        assert_eq!(
            err.exit_code(),
            128,
            "unknown messages should default to fatal (128)"
        );
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

        // Invalid formats should return error
        assert!(parse_author("invalid").is_err());
        assert!(parse_author("No Email").is_err());
        assert!(parse_author("<noemail@test.com>").is_err());
        assert!(parse_author("Name <").is_err());
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
        // 1. 初始化临时 Libra 仓库（保持原有逻辑，确保仓库结构正确）
        let temp_path = tempdir().unwrap();
        setup_with_new_libra_in(temp_path.path()).await;
        let _guard = ChangeDirGuard::new(temp_path.path());

        // 2. 基于项目根目录（CARGO_MANIFEST_DIR）构建测试 index 文件的绝对路径（关键修复）
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // 项目根目录（Cargo.toml 所在处）
        let index_file_path = project_root.join("tests/data/index/index-760"); // 绝对路径：根目录/tests/data/...

        // 3. 检查文件是否存在，给出明确提示（指导你补充文件）
        assert!(
            index_file_path.exists(),
            "测试文件不存在！请在项目根目录下创建路径：{}，并放入 index-760 文件",
            index_file_path.display()
        );

        // 4. 加载 index 文件（使用绝对路径，不再报错）
        let index = Index::from_file(index_file_path).unwrap_or_else(|e| {
            panic!("加载 index 文件失败：{}，请确认文件格式正确", e);
        });
        println!(
            "加载的 index 包含 {} 个跟踪文件",
            index.tracked_entries(0).len()
        );

        // 5. 初始化存储（确保指向临时仓库的 objects 目录，避免干扰主仓库）
        let temp_objects_dir = temp_path.path().join(".libra/objects"); // 临时仓库的 objects 目录
        let storage = ClientStorage::init(temp_objects_dir);

        // 6. 调用 create_tree（current_root 设为空，因为 index 中路径是相对于仓库根的）
        let tree = create_tree(&index, &storage, PathBuf::new()).await.unwrap();

        // 7. 原有验证逻辑（不变）
        assert!(storage.get(&tree.id).is_ok(), "根 tree 未保存到存储");
        for item in tree.tree_items.iter() {
            if item.mode == TreeItemMode::Tree {
                assert!(
                    storage.get(&item.id).is_ok(),
                    "子 tree 未保存：{}",
                    item.name
                );
                if item.name == "DeveloperExperience" {
                    let sub_tree_data = storage.get(&item.id).unwrap();
                    let sub_tree = Tree::from_bytes(&sub_tree_data, item.id).unwrap();
                    assert_eq!(
                        sub_tree.tree_items.len(),
                        4,
                        "DeveloperExperience 子 tree 条目数错误"
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
