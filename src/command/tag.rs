//! Manages tags by resolving target commits, creating lightweight or annotated tag objects, storing refs, and listing existing tags.

mod filters;
mod message;

use std::{io, path::PathBuf};

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
    libra tag -a -m \"Release v1.1\" v1.1 Create an annotated tag
    libra tag -F RELEASE_NOTES.md v1.2    Create an annotated tag from a file
    libra tag -a -e v1.3                  Edit an annotated tag message
    libra tag -l -n 2                     List tags with up to 2 annotation lines
    libra tag --points-at HEAD            List tags pointing at HEAD's commit
    libra tag --contains HEAD             List tags whose history contains HEAD
    libra tag --merged main               List tags merged into main
    libra tag --sort=-refname             List tags in reverse refname order
    libra tag -d v1.0 v1.1                Delete one or more tags
    libra tag --json v1.0                 Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(about = "Create, list, delete, or verify a tag object")]
#[command(after_help = TAG_EXAMPLES)]
pub struct TagArgs {
    /// The name of the tag to create, show, or delete. Only --delete accepts multiple names.
    #[clap(required = false, value_name = "name")]
    pub name: Vec<String>,

    /// List all tags
    #[clap(short, long, group = "action")]
    pub list: bool,

    /// Delete a tag
    #[clap(short, long, group = "action")]
    pub delete: bool,

    /// Create an annotated tag. Without an explicit message this opens an editor.
    #[clap(short = 'a', long = "annotate")]
    pub annotate: bool,

    /// Read the annotated tag message from a file.
    #[clap(short = 'F', long = "file", value_name = "file")]
    pub file: Option<PathBuf>,

    /// Open the annotated tag message in an editor before creating the tag.
    #[clap(short = 'e', long = "edit")]
    pub edit: bool,

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

    /// Only list tags whose target commit contains the specified commit (HEAD if omitted).
    #[clap(long, value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub contains: Vec<String>,

    /// Only list tags whose target commit does not contain the specified commit (HEAD if omitted).
    #[clap(long = "no-contains", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", action = clap::ArgAction::Append)]
    pub no_contains: Vec<String>,

    /// Only list tags whose target commit is merged into the specified commit (HEAD if omitted).
    #[clap(long, value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD", conflicts_with = "no_merged")]
    pub merged: Option<String>,

    /// Only list tags whose target commit is not merged into the specified commit (HEAD if omitted).
    #[clap(long = "no-merged", value_name = "commit", num_args = 0..=1, default_missing_value = "HEAD")]
    pub no_merged: Option<String>,

    /// Sort list output by refname or creatordate. Prefix with '-' to reverse.
    #[clap(long = "sort", value_name = "KEY")]
    pub sort: Option<String>,
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
    Delete {
        name: String,
        hash: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        deleted: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failed: Vec<TagDeleteFailure>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct TagDeleteFailure {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagListEntry {
    pub name: String,
    pub hash: String,
    pub tag_type: String,
    pub message: Option<String>,
    #[serde(skip_serializing)]
    pub display_message: Option<String>,
    #[serde(skip_serializing)]
    pub sort_time: usize,
}

impl TagOutput {
    fn failed_batch_delete(&self) -> bool {
        matches!(self, Self::Delete { failed, .. } if !failed.is_empty())
    }
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
    let failed_batch_delete = result.failed_batch_delete();
    render_tag_output(&result, output)?;
    if failed_batch_delete {
        return Err(CliError::silent_exit(128));
    }
    Ok(())
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

    #[error("malformed object name '{0}'")]
    InvalidPointsAtObject(String),

    #[error("malformed object name '{0}'")]
    InvalidFilterObject(String),

    #[error("'{0}' is not a valid tag name")]
    InvalidTagName(String),

    #[error("only --delete accepts multiple tag names")]
    MultipleNamesOnlyDelete,

    #[error("unsupported tag sort key '{0}'")]
    InvalidSortKey(String),

    #[error("{0}")]
    MissingName(String),

    #[error("{0}")]
    AnnotateUsage(String),

    #[error("failed to read tag message file '{path}': {source}")]
    ReadMessageFile {
        path: String,
        #[source]
        source: io::Error,
    },

    #[error("tag message file '{0}' is outside the working directory")]
    MessageFileOutsideWorkdir(String),

    #[error("tag message file '{path}' exceeds the {limit} byte limit")]
    MessageFileTooLarge { path: String, limit: u64 },

    #[error("no editor configured for tag annotation")]
    EditorNotConfigured,

    #[error("could not parse editor command: {0}")]
    EditorCommandInvalid(String),

    #[error("tag message editor failed: {0}")]
    EditorFailed(String),

    #[error("aborting due to empty annotation message")]
    EmptyAnnotationMessage,

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

    #[error("failed to load commit {commit}: {detail}")]
    CommitLoadFailed { commit: String, detail: String },
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
            TagError::InvalidPointsAtObject(object) => {
                CliError::fatal(format!("not a valid object name: '{object}'"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra log --oneline' to see available commits.")
            }
            TagError::InvalidFilterObject(object) => {
                CliError::fatal(format!("not a valid object name: '{object}'"))
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra log --oneline' to see available commits.")
            }
            TagError::InvalidTagName(name) => {
                CliError::command_usage(format!("'{name}' is not a valid tag name"))
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint("tag names must be 1-255 bytes and contain no control characters.")
            }
            TagError::MultipleNamesOnlyDelete => CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("use 'libra tag -d <name> [<name>...]' to delete multiple tags."),
            TagError::InvalidSortKey(key) => {
                CliError::command_usage(format!("unsupported tag sort key '{key}'"))
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint("tag supports refname, -refname, creatordate, and -creatordate.")
            }
            TagError::MissingName(message) => CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("use 'libra tag <name>' to create or update a tag")
                .with_hint("use 'libra tag -l' to list existing tags"),
            TagError::AnnotateUsage(message) => CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("create an annotated tag with 'libra tag -a -m <message> <name>'"),
            TagError::ReadMessageFile { .. } => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
            }
            TagError::MessageFileOutsideWorkdir(_) | TagError::MessageFileTooLarge { .. } => {
                CliError::command_usage(message)
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
            }
            TagError::EditorNotConfigured => CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("set $GIT_EDITOR, core.editor, $VISUAL, or $EDITOR.")
                .with_hint("or provide the message with -m or -F."),
            TagError::EditorCommandInvalid(_) => CliError::command_usage(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("check the configured editor command for quoting errors."),
            TagError::EditorFailed(_) => {
                CliError::fatal(message).with_stable_code(StableErrorCode::IoWriteFailed)
            }
            TagError::EmptyAnnotationMessage => CliError::fatal(message)
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("provide the tag message with -m, -F, or a non-empty editor buffer."),
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
            TagError::CommitLoadFailed { .. } => {
                CliError::fatal(message).with_stable_code(StableErrorCode::RepoCorrupt)
            }
        }
    }
}

fn validate_named_tag_action(args: &TagArgs) -> Result<(), TagError> {
    for name in &args.name {
        validate_tag_name(name)?;
    }

    let create_message_flag =
        args.annotate || args.message.is_some() || args.file.is_some() || args.edit;
    if create_message_flag && (args.list || args.delete) {
        return Err(TagError::AnnotateUsage(
            "tag message options are only valid when creating a tag".to_string(),
        ));
    }

    if args.delete {
        if args.n_lines.is_some()
            || args.points_at.is_some()
            || !args.contains.is_empty()
            || !args.no_contains.is_empty()
            || args.merged.is_some()
            || args.no_merged.is_some()
            || args.sort.is_some()
        {
            return Err(TagError::AnnotateUsage(
                "tag list filters are not valid with --delete".to_string(),
            ));
        }
        if args.name.is_empty() {
            return Err(TagError::MissingName(
                "tag name is required for --delete".to_string(),
            ));
        }
        return Ok(());
    }

    if args.name.len() > 1 {
        return Err(TagError::MultipleNamesOnlyDelete);
    }

    if args.annotate && args.name.is_empty() {
        return Err(TagError::MissingName(
            "tag name is required when using --annotate".to_string(),
        ));
    }

    if !args.name.is_empty() {
        return Ok(());
    }

    let message = if args.message.is_some() {
        Some("tag name is required when using --message")
    } else if args.file.is_some() {
        Some("tag name is required when using --file")
    } else if args.edit {
        Some("tag name is required when using --edit")
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

fn validate_tag_name(name: &str) -> Result<(), TagError> {
    if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
        return Err(TagError::InvalidTagName(name.to_string()));
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
    let args = TagArgs {
        name: vec![tag_name.to_string()],
        list: false,
        delete: false,
        annotate: message.is_some(),
        file: None,
        edit: false,
        message,
        force,
        n_lines: None,
        points_at: None,
        contains: Vec::new(),
        no_contains: Vec::new(),
        merged: None,
        no_merged: None,
        sort: None,
    };
    run_create_tag(tag_name, &args)
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

    if args.list || tag_list_implied(args) {
        // `--points-at` peels each tag to its commit and keeps only those that
        // resolve to the requested object, mirroring `git tag --points-at`.
        // Like `-n`, it forces list mode even when a name is also supplied.
        let points_at = match args.points_at.as_deref() {
            Some(object) => Some(resolve_points_at_object(object).await?),
            None => None,
        };
        return Ok(TagOutput::List {
            tags: collect_tags(args.n_lines.unwrap_or(0), points_at.as_ref(), args).await?,
        });
    }

    if args.delete {
        return run_delete_tags(&args.name).await;
    }

    let name = args.name.first().map(String::as_str).unwrap_or_default();
    run_create_tag(name, args).await
}

fn tag_list_implied(args: &TagArgs) -> bool {
    args.n_lines.is_some()
        || args.points_at.is_some()
        || args.name.is_empty()
        || !args.contains.is_empty()
        || !args.no_contains.is_empty()
        || args.merged.is_some()
        || args.no_merged.is_some()
        || args.sort.is_some()
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
        TagOutput::Delete {
            name,
            hash,
            deleted,
            failed,
        } => {
            if deleted.is_empty() {
                if let Some(hash) = hash {
                    println!("Deleted tag '{name}' (was {})", short_display_hash(hash));
                } else {
                    println!("Deleted tag '{name}'");
                }
            } else {
                for tag_name in deleted {
                    println!("Deleted tag '{tag_name}'");
                }
            }
            for failure in failed {
                eprintln!("error: {}", failure.reason);
            }
        }
    }

    Ok(())
}

pub async fn render_tags(show_lines: usize) -> Result<String, anyhow::Error> {
    let args = TagArgs {
        name: Vec::new(),
        list: true,
        delete: false,
        annotate: false,
        file: None,
        edit: false,
        message: None,
        force: false,
        n_lines: Some(show_lines),
        points_at: None,
        contains: Vec::new(),
        no_contains: Vec::new(),
        merged: None,
        no_merged: None,
        sort: None,
    };
    let tags = collect_tags(show_lines, None, &args)
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

async fn run_create_tag(tag_name: &str, args: &TagArgs) -> Result<TagOutput, TagError> {
    let message = message::resolve_annotation_message(args).await?;
    let created = tag::create(tag_name, message, args.force)
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
        deleted: Vec::new(),
        failed: Vec::new(),
    })
}

async fn run_delete_tags(tag_names: &[String]) -> Result<TagOutput, TagError> {
    if tag_names.len() == 1 {
        return run_delete_tag(&tag_names[0]).await;
    }

    let mut deleted = Vec::new();
    let mut failed = Vec::new();
    for tag_name in tag_names {
        match resolve_tag_ref_for_delete(tag_name).await {
            Ok(_) => match tag::delete(tag_name).await {
                Ok(()) => deleted.push(tag_name.clone()),
                Err(source) => failed.push(TagDeleteFailure {
                    name: tag_name.clone(),
                    reason: TagError::DeleteFailed {
                        name: tag_name.clone(),
                        source,
                    }
                    .to_string(),
                }),
            },
            Err(error) => failed.push(TagDeleteFailure {
                name: tag_name.clone(),
                reason: error.to_string(),
            }),
        }
    }

    Ok(TagOutput::Delete {
        name: tag_names.first().cloned().unwrap_or_default(),
        hash: None,
        deleted,
        failed,
    })
}

async fn collect_tags(
    show_lines: usize,
    points_at: Option<&ObjectHash>,
    args: &TagArgs,
) -> Result<Vec<TagListEntry>, TagError> {
    let tags = tag::list().await.map_err(TagError::ListFailed)?;
    let mut entries = Vec::with_capacity(tags.len());
    let contains_set = filters::resolve_commit_set(&args.contains).await?;
    let no_contains_set = filters::resolve_commit_set(&args.no_contains).await?;
    let merge_filter = if let Some(baseline) = args.merged.as_deref() {
        Some((filters::resolve_reachable_set(baseline).await?, true))
    } else if let Some(baseline) = args.no_merged.as_deref() {
        Some((filters::resolve_reachable_set(baseline).await?, false))
    } else {
        None
    };
    let sort_key = args
        .sort
        .as_deref()
        .map(filters::parse_sort_key)
        .transpose()?;

    for tag in tags {
        if let Some(target) = points_at
            && &tag_peeled_commit(&tag.object) != target
        {
            continue;
        }
        let commit_id = tag_target_commit_id(&tag.object);
        if let Some(commit_id) = commit_id {
            if !contains_set.is_empty() && !filters::commit_contains(commit_id, &contains_set)? {
                continue;
            }
            if !no_contains_set.is_empty() && filters::commit_contains(commit_id, &no_contains_set)?
            {
                continue;
            }
            if let Some((reachable, want_merged)) = &merge_filter
                && reachable.contains(&commit_id) != *want_merged
            {
                continue;
            }
        } else if !contains_set.is_empty() || !no_contains_set.is_empty() || merge_filter.is_some()
        {
            continue;
        }
        entries.push(tag_to_list_entry(tag, show_lines));
    }
    if let Some(sort_key) = sort_key {
        filters::sort_entries(&mut entries, sort_key);
    }
    Ok(entries)
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
fn tag_peeled_commit(object: &tag::TagObject) -> ObjectHash {
    match object {
        TagObject::Commit(commit) => commit.id,
        TagObject::Tag(tag_object) => tag_object.object_hash,
        TagObject::Tree(tree) => tree.id,
        TagObject::Blob(blob) => blob.id,
    }
}

fn tag_target_commit_id(object: &tag::TagObject) -> Option<ObjectHash> {
    match object {
        TagObject::Commit(commit) => Some(commit.id),
        TagObject::Tag(tag_object)
            if tag_object.object_type
                == git_internal::internal::object::types::ObjectType::Commit =>
        {
            Some(tag_object.object_hash)
        }
        _ => None,
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
    let (tag_type, message, display_message, sort_time) = match &object {
        TagObject::Tag(tag_object) => {
            let message = trim_tag_message(&tag_object.message, show_lines);
            (
                "annotated".to_string(),
                message.clone(),
                message,
                tag_object.tagger.timestamp,
            )
        }
        TagObject::Commit(commit) => (
            "lightweight".to_string(),
            None,
            trim_tag_message(&commit.message, show_lines),
            commit.committer.timestamp,
        ),
        _ => ("lightweight".to_string(), None, None, 0),
    };

    TagListEntry {
        name: tag_name,
        hash,
        tag_type,
        message,
        display_message,
        sort_time,
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
        assert_eq!(
            crate::common_utils::parse_commit_msg(&commit.message).0,
            "Initial commit"
        );
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
        assert_eq!(
            crate::common_utils::parse_commit_msg(&commit.message).0,
            "Initial commit"
        );

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
    fn parses_points_at_flag() {
        let args = TagArgs::try_parse_from(["tag", "--points-at", "HEAD"]).unwrap();
        assert_eq!(args.points_at.as_deref(), Some("HEAD"));
        assert!(!args.list);
        assert!(args.name.is_empty());
    }

    #[test]
    fn annotate_with_message_is_valid() {
        let args = TagArgs::try_parse_from(["tag", "-a", "-m", "msg", "v1.0"]).unwrap();
        assert!(args.annotate);
        assert!(validate_named_tag_action(&args).is_ok());
    }

    #[test]
    fn annotate_without_message_is_valid_for_editor_entry() {
        let args = TagArgs::try_parse_from(["tag", "-a", "v1.0"]).unwrap();
        assert!(args.annotate);
        assert_eq!(args.name, vec!["v1.0".to_string()]);
        assert!(validate_named_tag_action(&args).is_ok());
    }

    #[test]
    fn annotate_with_list_is_usage_error() {
        let args = TagArgs::try_parse_from(["tag", "-a", "-l"]).unwrap();
        let err = validate_named_tag_action(&args).unwrap_err();
        assert!(matches!(err, TagError::AnnotateUsage(_)), "got: {err:?}");
        assert!(
            CliError::from(err).stable_code() == StableErrorCode::CliInvalidArguments,
            "annotate misuse must map to LBR-CLI-002"
        );
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
