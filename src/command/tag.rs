//! Manages tags by resolving target commits, creating lightweight or annotated tag objects, storing refs, and listing existing tags.

use std::io;

use clap::Parser;
use git_internal::{errors::GitError, hash::ObjectHash};
use sea_orm::DbErr;
use serde::Serialize;

use crate::{
    internal::{branch, tag, tag::TagObject},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        text::short_display_hash,
        util,
    },
};

/// GitHub Issues URL shown on the `SerializeAnnotatedTag` internal-invariant
/// error path so users can report the bug; mirrors `push.rs`'s
/// `ObjectCollection` / `PackEncoding` hint pattern.
const ISSUE_URL: &str = "https://github.com/web3infra-foundation/libra/issues";

const TAG_EXAMPLES: &str = "\
EXAMPLES:
    libra tag v1.0                        Create a lightweight tag at HEAD
    libra tag -m \"Release v1.1\" v1.1    Create an annotated tag
    libra tag -l -n 2                     List tags with up to 2 annotation lines
    libra tag -d v1.0                     Delete a tag
    libra tag --points-at HEAD            List tags pointing at HEAD's commit
    libra tag --json v1.0                 Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(about = "Create, list, delete, or verify a tag object")]
#[command(after_help = TAG_EXAMPLES)]
pub struct TagArgs {
    /// The name of the tag to create, show, or delete
    #[clap(required = false)]
    pub name: Option<String>,

    /// List all tags
    #[clap(short, long, group = "action")]
    pub list: bool,

    /// Delete a tag
    #[clap(short, long, group = "action")]
    pub delete: bool,

    /// Message for the annotated tag. If provided, creates an annotated tag.
    #[clap(short, long)]
    pub message: Option<String>,

    /// Replace an existing tag with the same name instead of failing
    #[clap(short, long, group = "action")]
    pub force: bool,

    /// Number of annotation lines to display when listing tags (0 for tag names only)
    #[clap(short, long)]
    pub n_lines: Option<usize>,

    /// Only list tags pointing at the given object (peeled to its commit). Implies list mode.
    #[clap(long = "points-at", value_name = "object")]
    pub points_at: Option<String>,

    /// Only list tags whose commit contains COMMIT (COMMIT is an ancestor). Implies list mode.
    #[clap(long, value_name = "commit")]
    pub contains: Option<String>,

    /// Only list tags whose commit does NOT contain COMMIT. Implies list mode.
    #[clap(long = "no-contains", value_name = "commit")]
    pub no_contains: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum TagOutput {
    #[serde(rename = "list")]
    List { tags: Vec<TagListEntry> },
    #[serde(rename = "create")]
    Create {
        name: String,
        hash: String,
        tag_type: String,
        message: Option<String>,
    },
    #[serde(rename = "delete")]
    Delete { name: String, hash: Option<String> },
}

#[derive(Debug, Clone, Serialize)]
pub struct TagListEntry {
    pub name: String,
    pub hash: String,
    pub tag_type: String,
    pub message: Option<String>,
    #[serde(skip_serializing)]
    pub display_message: Option<String>,
}

pub async fn execute(args: TagArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Lists, creates, or deletes tags depending on the
/// provided arguments.
pub async fn execute_safe(args: TagArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_tag(&args).await.map_err(CliError::from)?;
    render_tag_output(&result, output)
}

pub(crate) fn validate_cli_args(args: &TagArgs) -> CliResult<()> {
    validate_named_tag_action(args).map_err(CliError::from)
}

#[derive(Debug, thiserror::Error)]
enum TagError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("tag '{0}' already exists")]
    AlreadyExists(String),

    #[error("tag '{0}' not found")]
    NotFound(String),

    #[error("{0}")]
    MissingName(String),

    #[error("Cannot create tag: HEAD does not point to a commit")]
    HeadUnborn,

    #[error("failed to resolve HEAD commit: {0}")]
    ResolveHead(#[source] branch::BranchStoreError),

    #[error("failed to read existing tags before creating '{name}': {source}")]
    CheckExistingFailed {
        name: String,
        #[source]
        source: DbErr,
    },

    #[error("failed to serialize annotated tag object: {0}")]
    SerializeAnnotatedTag(#[source] GitError),

    #[error("failed to store annotated tag object: {0}")]
    StoreObjectFailed(#[source] io::Error),

    #[error("failed to persist tag reference '{name}': {source}")]
    PersistReferenceFailed {
        name: String,
        #[source]
        source: DbErr,
    },

    #[error("failed to delete tag '{name}': {source}")]
    DeleteFailed {
        name: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to load tag '{name}': {source}")]
    LoadFailed {
        name: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to list tags: {0}")]
    ListFailed(#[source] tag::ListTagError),

    #[error("malformed object name '{0}'")]
    InvalidPointsAtObject(String),

    #[error("failed to compute reachability for --contains/--no-contains: {0}")]
    Reachability(String),
}

fn classify_tag_load_error(error: &anyhow::Error) -> StableErrorCode {
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<DbErr>().is_some())
    {
        StableErrorCode::IoReadFailed
    } else {
        StableErrorCode::RepoCorrupt
    }
}

fn classify_list_tag_error(error: &tag::ListTagError) -> StableErrorCode {
    match error {
        tag::ListTagError::Query(_) => StableErrorCode::IoReadFailed,
        tag::ListTagError::MissingCommit { .. }
        | tag::ListTagError::InvalidObjectHash { .. }
        | tag::ListTagError::MissingName
        | tag::ListTagError::LoadObject { .. } => StableErrorCode::RepoCorrupt,
    }
}

impl From<TagError> for CliError {
    fn from(error: TagError) -> Self {
        let message = error.to_string();
        match error {
            TagError::NotInRepo => CliError::repo_not_found(),
            TagError::AlreadyExists(name) => {
                CliError::fatal(format!("tag '{name}' already exists"))
                    .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                    .with_hint(format!("delete it first with 'libra tag -d {name}'."))
                    .with_hint("or choose a different tag name.")
            }
            TagError::NotFound(name) => CliError::fatal(format!("tag '{name}' not found"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("use 'libra tag -l' to list available tags."),
            TagError::MissingName(message) => CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("use 'libra tag <name>' to create or update a tag")
                .with_hint("use 'libra tag -l' to list existing tags"),
            TagError::HeadUnborn => CliError::fatal(message)
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_hint("create a commit first before tagging HEAD."),
            TagError::ResolveHead(source) => {
                let stable_code = match source {
                    branch::BranchStoreError::Query(_) => StableErrorCode::IoReadFailed,
                    _ => StableErrorCode::RepoCorrupt,
                };
                CliError::fatal(message).with_stable_code(stable_code)
            }
            TagError::CheckExistingFailed { .. } => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
            }
            TagError::SerializeAnnotatedTag(_) => CliError::fatal(message)
                .with_stable_code(StableErrorCode::InternalInvariant)
                .with_hint(format!("this is a bug; please report it at {ISSUE_URL}")),
            TagError::StoreObjectFailed(_) => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            TagError::PersistReferenceFailed { .. } => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            TagError::DeleteFailed { .. } => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            TagError::LoadFailed { source, .. } => {
                CliError::fatal(message).with_stable_code(classify_tag_load_error(&source))
            }
            TagError::ListFailed(source) => {
                CliError::fatal(message).with_stable_code(classify_list_tag_error(&source))
            }
            TagError::InvalidPointsAtObject(object) => {
                CliError::fatal(format!("not a valid object name: '{object}'"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra log --oneline' to see available commits.")
            }
            TagError::Reachability(_) => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
            }
        }
    }
}

fn validate_named_tag_action(args: &TagArgs) -> Result<(), TagError> {
    if args.name.is_some() {
        return Ok(());
    }

    let message = if args.delete {
        Some("tag name is required for --delete")
    } else if args.message.is_some() {
        Some("tag name is required when using --message")
    } else if args.force {
        Some("tag name is required for --force")
    } else {
        None
    };

    if let Some(message) = message {
        return Err(TagError::MissingName(message.to_string()));
    }

    Ok(())
}

#[cfg(test)]
async fn create_tag(tag_name: &str, message: Option<String>, force: bool) {
    if let Err(err) = create_tag_safe(tag_name, message, force).await {
        err.print_stderr();
    }
}

#[cfg(test)]
async fn create_tag_safe(tag_name: &str, message: Option<String>, force: bool) -> CliResult<()> {
    run_create_tag(tag_name, message, force)
        .await
        .map(|_| ())
        .map_err(CliError::from)?;
    Ok(())
}

fn map_create_tag_error(tag_name: &str, error: tag::CreateTagError) -> TagError {
    match error {
        tag::CreateTagError::AlreadyExists(existing_tag_name) => {
            TagError::AlreadyExists(existing_tag_name)
        }
        tag::CreateTagError::HeadUnborn => TagError::HeadUnborn,
        tag::CreateTagError::ResolveHead(source) => TagError::ResolveHead(source),
        tag::CreateTagError::CheckExisting(source) => TagError::CheckExistingFailed {
            name: tag_name.to_string(),
            source,
        },
        tag::CreateTagError::SerializeTag(source) => TagError::SerializeAnnotatedTag(source),
        tag::CreateTagError::StoreObject(source) => TagError::StoreObjectFailed(source),
        tag::CreateTagError::PersistReference(source) => TagError::PersistReferenceFailed {
            name: tag_name.to_string(),
            source,
        },
    }
}

async fn run_tag(args: &TagArgs) -> Result<TagOutput, TagError> {
    validate_named_tag_action(args)?;
    util::require_repo().map_err(|_| TagError::NotInRepo)?;

    if args.list
        || args.n_lines.is_some()
        || args.points_at.is_some()
        || args.contains.is_some()
        || args.no_contains.is_some()
        || args.name.is_none()
    {
        // `--points-at` peels each tag to its commit and keeps only those that
        // resolve to the requested object, mirroring `git tag --points-at`.
        // Like `-n`, it forces list mode even when a name is also supplied.
        let points_at = match args.points_at.as_deref() {
            Some(object) => Some(resolve_points_at_object(object).await?),
            None => None,
        };
        // `--contains`/`--no-contains` resolve to a commit and keep (or drop)
        // tags whose peeled commit has that commit as an ancestor.
        let contains = match args.contains.as_deref() {
            Some(object) => Some(resolve_points_at_object(object).await?),
            None => None,
        };
        let no_contains = match args.no_contains.as_deref() {
            Some(object) => Some(resolve_points_at_object(object).await?),
            None => None,
        };
        // In list mode a positional name acts as a glob pattern (`tag -l 'v1.*'`).
        let pattern = args.name.as_deref().map(compile_tag_glob);
        return Ok(TagOutput::List {
            tags: collect_tags(
                args.n_lines.unwrap_or(0),
                points_at.as_ref(),
                pattern.as_ref(),
                contains.as_ref(),
                no_contains.as_ref(),
            )
            .await?,
        });
    }

    let name = args.name.as_deref().unwrap_or_default();
    if args.delete {
        return run_delete_tag(name).await;
    }

    run_create_tag(name, args.message.clone(), args.force).await
}

fn render_tag_output(result: &TagOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("tag", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    match result {
        TagOutput::List { tags } => {
            print!("{}", format_tag_entries(tags));
        }
        TagOutput::Create {
            name,
            hash,
            tag_type,
            ..
        } => {
            println!(
                "Created {tag_type} tag '{name}' at {}",
                short_display_hash(hash)
            );
        }
        TagOutput::Delete { name, hash } => {
            if let Some(hash) = hash {
                println!("Deleted tag '{name}' (was {})", short_display_hash(hash));
            } else {
                println!("Deleted tag '{name}'");
            }
        }
    }

    Ok(())
}

pub async fn render_tags(show_lines: usize) -> Result<String, anyhow::Error> {
    let tags = collect_tags(show_lines, None, None, None, None)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(format_tag_entries(&tags))
}

#[cfg(test)]
async fn delete_tag(tag_name: &str) {
    if let Err(err) = delete_tag_safe(tag_name, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

#[cfg(test)]
async fn delete_tag_safe(tag_name: &str, output: &OutputConfig) -> CliResult<()> {
    let result = run_delete_tag(tag_name).await.map_err(CliError::from)?;
    render_tag_output(&result, output)?;
    Ok(())
}

async fn run_create_tag(
    tag_name: &str,
    message: Option<String>,
    force: bool,
) -> Result<TagOutput, TagError> {
    let created = tag::create(tag_name, message, force)
        .await
        .map_err(|error| map_create_tag_error(tag_name, error))?;
    Ok(TagOutput::Create {
        name: created.name,
        hash: created.target.to_string(),
        tag_type: if created.annotated {
            "annotated".to_string()
        } else {
            "lightweight".to_string()
        },
        message: created.message,
    })
}

async fn run_delete_tag(tag_name: &str) -> Result<TagOutput, TagError> {
    let snapshot = resolve_tag_ref_for_delete(tag_name).await?;
    tag::delete(tag_name)
        .await
        .map_err(|source| TagError::DeleteFailed {
            name: tag_name.to_string(),
            source,
        })?;
    Ok(TagOutput::Delete {
        name: tag_name.to_string(),
        hash: snapshot.target,
    })
}

async fn collect_tags(
    show_lines: usize,
    points_at: Option<&ObjectHash>,
    pattern: Option<&regex::Regex>,
    contains: Option<&ObjectHash>,
    no_contains: Option<&ObjectHash>,
) -> Result<Vec<TagListEntry>, TagError> {
    let tags = tag::list().await.map_err(TagError::ListFailed)?;
    let mut entries = Vec::with_capacity(tags.len());
    for tag in tags {
        // `tag -l <pattern>` keeps only tags whose name matches the fnmatch glob.
        if let Some(re) = pattern
            && !re.is_match(&tag.name)
        {
            continue;
        }
        if let Some(target) = points_at
            && &tag_peeled_commit(&tag.object) != target
        {
            continue;
        }
        // `--contains`/`--no-contains`: walk the tag's peeled commit's history
        // once and keep (or drop) tags that reach the requested commit.
        if contains.is_some() || no_contains.is_some() {
            let peeled = tag_peeled_commit(&tag.object);
            let reachable = crate::command::log::get_reachable_commits(peeled.to_string(), None)
                .await
                .map_err(|error| TagError::Reachability(error.to_string()))?;
            let reaches = |target: &ObjectHash| reachable.iter().any(|commit| &commit.id == target);
            if let Some(target) = contains
                && !reaches(target)
            {
                continue;
            }
            if let Some(target) = no_contains
                && reaches(target)
            {
                continue;
            }
        }
        entries.push(tag_to_list_entry(tag, show_lines));
    }
    Ok(entries)
}

/// Compile a `tag -l <pattern>` shell-glob into an anchored regex. `*`/`?`
/// become `.*`/`.`, `[...]` character classes pass through, other regex
/// metacharacters are escaped. An unparseable glob falls back to a literal match.
fn compile_tag_glob(glob: &str) -> regex::Regex {
    let mut pattern = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => pattern.push_str(".*"),
            '?' => pattern.push('.'),
            '[' | ']' => pattern.push(ch),
            c if ".+(){}|^$\\".contains(c) => {
                pattern.push('\\');
                pattern.push(c);
            }
            c => pattern.push(c),
        }
    }
    pattern.push('$');
    regex::Regex::new(&pattern).unwrap_or_else(|_| {
        regex::Regex::new(&format!("^{}$", regex::escape(glob)))
            .expect("escaped-literal regex is always valid")
    })
}

/// Resolve the `--points-at` argument to an object hash, mapping an
/// unresolvable revision to [`TagError::InvalidPointsAtObject`] so the CLI
/// reports `not a valid object name` (LBR-CLI-003, exit 129) rather than a
/// raw resolver error.
async fn resolve_points_at_object(object: &str) -> Result<ObjectHash, TagError> {
    crate::command::get_target_commit(object)
        .await
        .map_err(|_| TagError::InvalidPointsAtObject(object.to_string()))
}

/// Peel a tag's target object down to the commit it ultimately references:
/// lightweight tags point straight at a commit, annotated tags carry the
/// commit in their `object_hash`, and tree/blob tags peel to themselves.
/// Used by `--points-at` to compare against the requested object.
fn tag_peeled_commit(object: &TagObject) -> ObjectHash {
    match object {
        TagObject::Commit(commit) => commit.id,
        TagObject::Tag(tag_object) => tag_object.object_hash,
        TagObject::Tree(tree) => tree.id,
        TagObject::Blob(blob) => blob.id,
    }
}

fn tag_to_list_entry(tag: tag::Tag, show_lines: usize) -> TagListEntry {
    let tag::Tag { name, object } = tag;
    tag_object_to_list_entry(name, object, show_lines)
}

fn tag_object_to_list_entry(
    tag_name: String,
    object: tag::TagObject,
    show_lines: usize,
) -> TagListEntry {
    let hash = match &object {
        TagObject::Commit(commit) => commit.id.to_string(),
        TagObject::Tag(tag_object) => tag_object.id.to_string(),
        TagObject::Tree(tree) => tree.id.to_string(),
        TagObject::Blob(blob) => blob.id.to_string(),
    };
    let (tag_type, message, display_message) = match &object {
        TagObject::Tag(tag_object) => {
            let message = trim_tag_message(&tag_object.message, show_lines);
            ("annotated".to_string(), message.clone(), message)
        }
        TagObject::Commit(commit) => (
            "lightweight".to_string(),
            None,
            trim_tag_message(&commit.message, show_lines),
        ),
        _ => ("lightweight".to_string(), None, None),
    };

    TagListEntry {
        name: tag_name,
        hash,
        tag_type,
        message,
        display_message,
    }
}

fn trim_tag_message(message: &str, show_lines: usize) -> Option<String> {
    if show_lines == 0 {
        return None;
    }

    let value = message
        .trim()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(show_lines)
        .collect::<Vec<_>>()
        .join("\n");

    if value.is_empty() { None } else { Some(value) }
}

fn format_tag_entries(tags: &[TagListEntry]) -> String {
    let mut output = String::new();
    for tag in tags {
        match tag.display_message.as_ref().or(tag.message.as_ref()) {
            Some(message) => {
                for (index, line) in message.lines().enumerate() {
                    if index == 0 {
                        output.push_str(&format!("{:<20} {}\n", tag.name, line));
                    } else {
                        output.push_str(&format!("{:<20} {}\n", "", line));
                    }
                }
            }
            None => output.push_str(&format!("{}\n", tag.name)),
        }
    }
    output
}

async fn resolve_tag_ref_for_delete(tag_name: &str) -> Result<tag::TagReference, TagError> {
    match tag::find_tag_ref(tag_name).await {
        Ok(Some(reference)) => Ok(reference),
        Ok(None) => Err(TagError::NotFound(tag_name.to_string())),
        Err(source) => Err(TagError::LoadFailed {
            name: tag_name.to_string(),
            source: source.into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git_internal::internal::object::types::ObjectType;
    use sea_orm::DbErr;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        cli::parse_async,
        command::init::{self, InitArgs},
        internal::tag,
        utils::test::ChangeDirGuard,
    };

    async fn setup_repo_with_commit() -> (tempfile::TempDir, ChangeDirGuard) {
        let temp_dir = tempdir().unwrap();
        let guard = ChangeDirGuard::new(temp_dir.path());
        init::init(InitArgs {
            bare: false,
            template: None,
            initial_branch: None,
            repo_directory: ".".to_string(),
            quiet: false,
            shared: None,
            object_format: None,
            ref_format: None,
            from_git_repository: None,
            vault: false,
        })
        .await
        .unwrap();
        parse_async(Some(&["libra", "config", "user.name", "Tag Test User"]))
            .await
            .unwrap();
        parse_async(Some(&[
            "libra",
            "config",
            "user.email",
            "tag-test@example.com",
        ]))
        .await
        .unwrap();
        fs::write("test.txt", "hello").unwrap();
        parse_async(Some(&["libra", "add", "test.txt"]))
            .await
            .unwrap();
        parse_async(Some(&[
            "libra",
            "commit",
            "--no-verify",
            "-m",
            "Initial commit",
        ]))
        .await
        .unwrap();
        (temp_dir, guard)
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_lightweight_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-light", None, false).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-light");
        assert_eq!(tags[0].object.get_type(), ObjectType::Commit);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_lightweight_tag_force() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-light", None, false).await;
        create_tag("v1.0-light", None, true).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-light");
        assert_eq!(tags[0].object.get_type(), ObjectType::Commit);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_annotated_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-annotated", Some("Release v1.0".to_string()), false).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-annotated");
        assert_eq!(tags[0].object.get_type(), ObjectType::Tag);
    }

    #[tokio::test]
    #[serial]
    async fn test_create_and_list_annotated_tag_force() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-annotated", Some("Release v1.0".to_string()), false).await;
        create_tag("v1.0-annotated", Some("Release v2.0".to_string()), true).await;
        let tags = tag::list().await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0-annotated");
        assert_eq!(tags[0].object.get_type(), ObjectType::Tag);

        // Check message
        let result = tag::find_tag_and_commit("v1.0-annotated").await;
        assert!(result.is_ok());
        let (object, _) = result.unwrap().unwrap();
        if let tag::TagObject::Tag(tag_object) = object {
            assert_eq!(tag_object.message, "Release v2.0");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_show_lightweight_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-light", None, false).await;
        let result = tag::find_tag_and_commit("v1.0-light").await;
        assert!(result.is_ok());
        let (object, commit) = result.unwrap().unwrap();
        assert_eq!(object.get_type(), ObjectType::Commit);
        assert_eq!(commit.message.trim(), "Initial commit");
    }

    #[tokio::test]
    #[serial]
    async fn test_show_annotated_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0-annotated", Some("Test message".to_string()), false).await;
        let result = tag::find_tag_and_commit("v1.0-annotated").await;
        assert!(result.is_ok());
        let (object, commit) = result.unwrap().unwrap();
        assert_eq!(object.get_type(), ObjectType::Tag);
        assert_eq!(commit.message.trim(), "Initial commit");

        // Verify tag object content directly from the TagObject enum
        if let tag::TagObject::Tag(tag_object) = object {
            assert_eq!(tag_object.message, "Test message");
        } else {
            panic!("Expected Tag object type");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_delete_tag() {
        let (_temp_dir, _guard) = setup_repo_with_commit().await;
        create_tag("v1.0", None, false).await;
        delete_tag("v1.0").await;
        let tags = tag::list().await.unwrap();
        assert!(tags.is_empty());
    }

    #[test]
    fn test_tag_check_existing_db_error_maps_as_io_read() {
        let cli_error = CliError::from(TagError::CheckExistingFailed {
            name: "v1.0".to_string(),
            source: DbErr::Custom("database is locked".to_string()),
        });

        assert_eq!(cli_error.stable_code(), StableErrorCode::IoReadFailed);
    }

    #[test]
    fn test_tag_list_db_error_maps_as_io_read() {
        let cli_error = CliError::from(TagError::ListFailed(tag::ListTagError::Query(
            DbErr::Custom("database is locked".to_string()),
        )));

        assert_eq!(cli_error.stable_code(), StableErrorCode::IoReadFailed);
    }

    #[test]
    fn test_tag_list_object_error_maps_as_repo_corrupt() {
        let cli_error =
            CliError::from(TagError::ListFailed(tag::ListTagError::InvalidObjectHash {
                name: "v1.0".to_string(),
                detail: "not-a-valid-hash".to_string(),
            }));

        assert_eq!(cli_error.stable_code(), StableErrorCode::RepoCorrupt);
    }

    #[test]
    fn parses_points_at_flag() {
        let args = TagArgs::try_parse_from(["tag", "--points-at", "HEAD"]).unwrap();
        assert_eq!(args.points_at.as_deref(), Some("HEAD"));
        assert!(!args.list);
        assert!(args.name.is_none());
    }

    /// An unresolvable `--points-at` revision maps to `CliInvalidTarget`
    /// (LBR-CLI-003, exit 129) with the same human-facing message as
    /// `branch --points-at`, so scripts see a stable "not a valid object
    /// name" failure rather than a raw resolver error.
    #[test]
    fn tag_error_invalid_points_at_object_maps_as_cli_invalid_target() {
        let err = CliError::from(TagError::InvalidPointsAtObject("nope".to_string()));
        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidTarget);
        assert!(
            err.message().contains("not a valid object name"),
            "unexpected message: {}",
            err.message(),
        );
    }

    /// Pin the `Display` format for the static-message and direct-
    /// message variants of [`TagError`]. These strings are used as
    /// the `CliError` message via `From<TagError> for CliError` and
    /// surface in both human and `--json` envelopes for the `tag`
    /// subcommand.
    ///
    /// Source-chained variants (ResolveHead, CheckExistingFailed,
    /// SerializeAnnotatedTag, StoreObjectFailed, …) wrap upstream
    /// error types (BranchStoreError / DbErr / GitError / io::Error)
    /// and are intentionally skipped — their {source} slot is owned
    /// by the wrapped error.
    #[test]
    fn tag_error_display_pins_static_message_variants() {
        assert_eq!(TagError::NotInRepo.to_string(), "not a libra repository");
        assert_eq!(
            TagError::AlreadyExists("v1.0.0".to_string()).to_string(),
            "tag 'v1.0.0' already exists",
        );
        assert_eq!(
            TagError::NotFound("v9.9.9".to_string()).to_string(),
            "tag 'v9.9.9' not found",
        );
        // MissingName(s) echoes the inner string verbatim.
        assert_eq!(
            TagError::MissingName("provide a tag name".to_string()).to_string(),
            "provide a tag name",
        );
        assert_eq!(
            TagError::HeadUnborn.to_string(),
            "Cannot create tag: HEAD does not point to a commit",
        );
    }

    /// tag.md Cross-Cutting G: `SerializeAnnotatedTag` is the one TagError
    /// variant that maps to `InternalInvariant` and must surface the
    /// GitHub Issues URL hint (mirroring push.rs `ObjectCollection` /
    /// `PackEncoding` pattern).
    #[test]
    fn tag_error_to_cli_error_serialize_annotated_tag_has_issue_url() {
        let err: CliError = TagError::SerializeAnnotatedTag(GitError::InvalidArgument(
            "synthetic serialize failure".to_string(),
        ))
        .into();
        assert_eq!(err.stable_code(), StableErrorCode::InternalInvariant);
        assert!(
            err.hints().iter().any(|h| h.as_str().contains("issues")),
            "SerializeAnnotatedTag must include the GitHub Issues URL hint, got hints: {:?}",
            err.hints()
        );
    }
}
