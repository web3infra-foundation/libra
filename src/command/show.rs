//! Show command that resolves object IDs and prints commit, tree, blob, or ref details with formatting suitable for diffable objects.

use std::{path::PathBuf, str::FromStr};

use clap::Parser;
use colored::Colorize;
use git_internal::{
    hash::ObjectHash,
    internal::object::{
        blob::Blob,
        commit::Commit,
        tree::{Tree, TreeItemMode},
        types::ObjectType,
    },
};
use serde::Serialize;

use crate::{
    command::{
        load_object,
        log::{ChangeType, generate_diff, get_changed_files_for_commit},
    },
    common_utils::parse_commit_msg,
    internal::{branch::Branch, head::Head, tag},
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

/// Shows commits, tags, trees, or blobs.
#[derive(Parser, Debug)]
pub struct ShowArgs {
    /// Object name (commit, tag, etc.) or `<object>:<path>`. Defaults to `HEAD`.
    #[clap(value_name = "OBJECT")]
    pub object: Option<String>,

    /// Skip patch output and only show object metadata.
    #[clap(long, short = 's')]
    pub no_patch: bool,

    /// Shorthand for `--pretty=oneline`.
    #[clap(long)]
    pub oneline: bool,

    /// Show only changed file names.
    #[clap(long)]
    pub name_only: bool,

    /// Show diff statistics.
    #[clap(long)]
    pub stat: bool,

    /// Limit output to matching paths.
    #[clap(value_name = "PATHS", num_args = 0..)]
    pub pathspec: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ShowOutput {
    #[serde(rename = "commit")]
    Commit(ShowCommitData),
    #[serde(rename = "tag")]
    Tag(ShowTagData),
    #[serde(rename = "tree")]
    Tree(ShowTreeData),
    #[serde(rename = "blob")]
    Blob(ShowBlobData),
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowCommitData {
    pub hash: String,
    pub short_hash: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_date: String,
    pub subject: String,
    pub body: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub files: Vec<ShowFileChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowFileChange {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowTagData {
    pub tag_name: String,
    pub tagger_name: Option<String>,
    pub tagger_email: Option<String>,
    pub tagger_date: Option<String>,
    pub message: String,
    pub target_hash: String,
    pub target_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowTreeData {
    pub entries: Vec<ShowTreeEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowTreeEntry {
    pub mode: String,
    pub object_type: String,
    pub hash: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowBlobData {
    pub hash: String,
    pub size: usize,
    pub is_binary: bool,
    pub content: Option<String>,
}

/// Executes the show command.
pub async fn execute(args: ShowArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Resolves a revision (commit, tag, tree, blob, or
/// `<rev>:<path>`) and prints its contents with diff formatting.
pub async fn execute_safe(args: ShowArgs, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        let result = run_show(&args).await?;
        return emit_json_data("show", &result, output);
    }

    let object_ref = args.object.as_deref().unwrap_or("HEAD");

    // Handle `<revision>:<path>` lookups before generic revision resolution.
    if let Some((rev, path)) = object_ref.split_once(':') {
        return show_commit_file(rev, path, &args).await;
    }

    // Resolve refs first so tags keep their custom rendering.
    if let Ok(commit_hash) = util::get_commit_base(object_ref).await {
        // Use find_tag_and_commit to check if it's a tag and get tag info
        match tag::find_tag_and_commit(object_ref).await {
            Ok(Some((object, _))) if object.get_type() == ObjectType::Tag => {
                // For annotated tags, show tag details
                let tag_hash = if let tag::TagObject::Tag(tag_obj) = &object {
                    tag_obj.id
                } else {
                    commit_hash
                };
                return show_tag_by_hash(&tag_hash, &args).await;
            }
            _ => {
                // Not a tag, lightweight tag, or tag doesn't exist: show as commit.
                return show_commit(&commit_hash, &args).await;
            }
        }
    }

    // Fall back to direct object IDs.
    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        return show_object_by_hash(&hash, &args).await;
    }

    Err(show_bad_revision_error(object_ref))
}

/// Shows an object by hash after resolving its object type.
fn show_object_by_hash<'a>(
    hash: &'a ObjectHash,
    args: &'a ShowArgs,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = CliResult<()>> + 'a>> {
    Box::pin(async move {
        let storage = ClientStorage::init(path::objects());

        let obj_type = storage.get_object_type(hash).map_err(|e| {
            CliError::fatal(format!("could not read object {}: {}", hash, e))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;

        match obj_type {
            ObjectType::Commit => show_commit(hash, args).await,
            ObjectType::Tag => show_tag_by_hash(hash, args).await,
            ObjectType::Tree => show_tree(hash).await,
            ObjectType::Blob => show_blob(hash).await,
            _ => Err(
                CliError::fatal(format!("unsupported object type for {}", hash))
                    .with_stable_code(StableErrorCode::CliInvalidTarget),
            ),
        }
    })
}

/// Shows a commit together with optional diff output.
async fn show_commit(commit_hash: &ObjectHash, args: &ShowArgs) -> CliResult<()> {
    // Load the commit before rendering any metadata or diff output.
    let commit = load_object::<Commit>(commit_hash).map_err(|e| {
        CliError::fatal(format!("could not load commit {}: {}", commit_hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    // Render the commit header first.
    display_commit_info(&commit, args);

    // Render patch-style details when requested.
    if !args.no_patch {
        let paths: Vec<PathBuf> = args.pathspec.iter().map(util::to_workdir_path).collect();

        if args.stat {
            // Show the summary view.
            show_diffstat(&commit, paths.clone()).await?;
        } else if args.name_only {
            // Show only changed file names.
            let changed_files = get_changed_files_for_commit(&commit, &paths).await?;
            if !changed_files.is_empty() {
                println!();
                for file in changed_files {
                    println!("{}", file.path.display());
                }
            }
        } else {
            // Show the full patch.
            let diff_output = generate_diff(&commit, paths).await?;
            if !diff_output.is_empty() {
                println!();
                print!("{}", diff_output);
            }
        }
    }
    Ok(())
}

/// Shows an annotated or lightweight tag.
async fn show_tag_by_hash(hash: &ObjectHash, args: &ShowArgs) -> CliResult<()> {
    match tag::load_object_trait(hash).await {
        Ok(tag::TagObject::Tag(tag_obj)) => {
            // Render the annotated tag header.
            println!("{} {}", "tag".yellow(), tag_obj.tag_name);
            println!(
                "Tagger: {} <{}>",
                tag_obj.tagger.name.trim(),
                tag_obj.tagger.email.trim()
            );

            let date = chrono::DateTime::from_timestamp(tag_obj.tagger.timestamp as i64, 0)
                .unwrap_or(chrono::DateTime::UNIX_EPOCH);
            println!("Date:   {}", date.to_rfc2822());
            println!();
            println!("{}", tag_obj.message.trim());
            println!();

            // Continue with the tagged object.
            show_object_by_hash(&tag_obj.object_hash, args).await?;
        }
        Ok(tag::TagObject::Commit(commit)) => {
            // Lightweight tags point directly to commits.
            show_commit(&commit.id, args).await?;
        }
        Ok(_) => {
            return Err(CliError::fatal("tag points to unsupported object type")
                .with_stable_code(StableErrorCode::CliInvalidTarget));
        }
        Err(e) => {
            return Err(
                CliError::fatal(e.to_string()).with_stable_code(StableErrorCode::RepoCorrupt)
            );
        }
    }
    Ok(())
}

/// Shows a tree object.
async fn show_tree(hash: &ObjectHash) -> CliResult<()> {
    let tree = load_object::<Tree>(hash).map_err(|e| {
        CliError::fatal(format!("could not load tree {}: {}", hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    println!("{} {}\n", "tree".yellow(), hash);

    for item in &tree.tree_items {
        println!(
            "{:06o} {} {}\t{}",
            tree_item_mode_to_u32(item.mode),
            tree_item_mode_to_object_type(item.mode),
            item.id,
            item.name
        );
    }
    Ok(())
}

/// Shows a blob as text when possible.
async fn show_blob(hash: &ObjectHash) -> CliResult<()> {
    let blob = load_object::<Blob>(hash).map_err(|e| {
        CliError::fatal(format!("could not load blob {}: {}", hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    // Print text blobs directly and summarize binary blobs.
    match String::from_utf8(blob.data.clone()) {
        Ok(text) => print!("{}", text),
        Err(_) => {
            println!("Binary file (size: {} bytes)", blob.data.len());
        }
    }
    Ok(())
}

/// Shows a file from a specific revision.
async fn show_commit_file(rev: &str, file_path: &str, _args: &ShowArgs) -> CliResult<()> {
    // Resolve the revision before looking up the path.
    let commit_hash = util::get_commit_base(rev)
        .await
        .map_err(|_| show_bad_revision_error(rev))?;

    let commit = load_object::<Commit>(&commit_hash).map_err(|e| {
        CliError::fatal(format!("could not load commit {}: {}", commit_hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    // Load the tree for the resolved commit.
    let tree = load_object::<Tree>(&commit.tree_id).map_err(|e| {
        CliError::fatal(format!("could not load tree {}: {}", commit.tree_id, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    // Find the target path inside the tree.
    let items = tree.get_plain_items();
    let target_path = PathBuf::from(file_path);

    if let Some((_, blob_hash)) = items.iter().find(|(path, _)| path == &target_path) {
        show_blob(blob_hash).await?;
    } else {
        return Err(
            CliError::fatal(format!("path '{}' does not exist in '{}'", file_path, rev))
                .with_stable_code(StableErrorCode::CliInvalidTarget),
        );
    }
    Ok(())
}

/// Renders the commit header using the selected format.
fn display_commit_info(commit: &Commit, args: &ShowArgs) {
    if args.oneline {
        // Oneline format prints the short hash and the first subject line.
        let short_hash = &commit.id.to_string()[..7];
        let (msg, _) = parse_commit_msg(&commit.message);
        let first_line = msg.lines().next().unwrap_or("");
        println!("{} {}", short_hash.yellow(), first_line);
    } else {
        // Full format matches the default `show` header layout.
        println!("{} {}", "commit".yellow(), commit.id.to_string().yellow());
        println!(
            "Author: {} <{}>",
            commit.author.name.trim(),
            commit.author.email.trim()
        );

        // Format the commit timestamp for display.
        let date = chrono::DateTime::from_timestamp(commit.committer.timestamp as i64, 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);
        println!("Date:   {}", date.to_rfc2822());

        // Print the commit message body.
        let (msg, _) = parse_commit_msg(&commit.message);
        for line in msg.lines() {
            println!("    {}", line);
        }
    }
}

/// Renders a simple diffstat summary.
async fn show_diffstat(commit: &Commit, paths: Vec<PathBuf>) -> CliResult<()> {
    let changed_files = get_changed_files_for_commit(commit, &paths).await?;

    if changed_files.is_empty() {
        return Ok(());
    }

    println!();

    // Count summary totals while printing each changed path.
    let mut additions = 0;
    let mut deletions = 0;

    for change in &changed_files {
        match change.status {
            ChangeType::Added => additions += 1,
            ChangeType::Deleted => deletions += 1,
            ChangeType::Modified => {
                additions += 1;
                deletions += 1;
            }
        }
        let status = match change.status {
            ChangeType::Added => "A",
            ChangeType::Modified => "M",
            ChangeType::Deleted => "D",
        };
        println!("{}  {}", status, change.path.display());
    }

    println!(
        "\n{} file{} changed, {} insertion{}(+), {} deletion{}(-)",
        changed_files.len(),
        if changed_files.len() != 1 { "s" } else { "" },
        additions,
        if additions != 1 { "s" } else { "" },
        deletions,
        if deletions != 1 { "s" } else { "" }
    );
    Ok(())
}

fn show_bad_revision_error(object_ref: &str) -> CliError {
    CliError::fatal(format!(
        "ambiguous argument '{}': unknown revision or path not in the working tree.",
        object_ref
    ))
    .with_stable_code(StableErrorCode::CliInvalidTarget)
    .with_hint("use '--' to separate paths from revisions, for example 'libra show -- <file>'.")
}

async fn run_show(args: &ShowArgs) -> CliResult<ShowOutput> {
    let object_ref = args.object.as_deref().unwrap_or("HEAD");

    if let Some((rev, path)) = object_ref.split_once(':') {
        return collect_commit_file_output(rev, path).await;
    }

    if let Ok(commit_hash) = util::get_commit_base(object_ref).await {
        match tag::find_tag_and_commit(object_ref).await {
            Ok(Some((object, _))) if object.get_type() == ObjectType::Tag => {
                let tag_hash = if let tag::TagObject::Tag(tag_obj) = &object {
                    tag_obj.id
                } else {
                    commit_hash
                };
                return collect_tag_output(&tag_hash).await;
            }
            _ => {
                return collect_commit_output(&commit_hash).await;
            }
        }
    }

    if let Ok(hash) = ObjectHash::from_str(object_ref) {
        return collect_object_output(&hash).await;
    }

    Err(show_bad_revision_error(object_ref))
}

async fn collect_object_output(hash: &ObjectHash) -> CliResult<ShowOutput> {
    let storage = ClientStorage::init(path::objects());
    let obj_type = storage.get_object_type(hash).map_err(|e| {
        CliError::fatal(format!("could not read object {}: {}", hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    match obj_type {
        ObjectType::Commit => collect_commit_output(hash).await,
        ObjectType::Tag => collect_tag_output(hash).await,
        ObjectType::Tree => collect_tree_output(hash).await,
        ObjectType::Blob => collect_blob_output(hash).await,
        _ => Err(
            CliError::fatal(format!("unsupported object type for {}", hash))
                .with_stable_code(StableErrorCode::CliInvalidTarget),
        ),
    }
}

async fn collect_commit_output(commit_hash: &ObjectHash) -> CliResult<ShowOutput> {
    let commit = load_object::<Commit>(commit_hash).map_err(|e| {
        CliError::fatal(format!("could not load commit {}: {}", commit_hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    let (subject, body) = split_subject_and_body(&commit.message);
    let files = get_changed_files_for_commit(&commit, &[]).await?;

    Ok(ShowOutput::Commit(ShowCommitData {
        hash: commit.id.to_string(),
        short_hash: commit.id.to_string()[..7].to_string(),
        author_name: commit.author.name.trim().to_string(),
        author_email: commit.author.email.trim().to_string(),
        author_date: format_timestamp(commit.author.timestamp as i64),
        committer_name: commit.committer.name.trim().to_string(),
        committer_email: commit.committer.email.trim().to_string(),
        committer_date: format_timestamp(commit.committer.timestamp as i64),
        subject,
        body,
        parents: commit
            .parent_commit_ids
            .iter()
            .map(ToString::to_string)
            .collect(),
        refs: collect_reference_names(commit.id).await,
        files: files
            .into_iter()
            .map(|file| ShowFileChange {
                path: file.path.display().to_string(),
                status: change_type_name(file.status).to_string(),
            })
            .collect(),
    }))
}

async fn collect_tag_output(hash: &ObjectHash) -> CliResult<ShowOutput> {
    match tag::load_object_trait(hash).await {
        Ok(tag::TagObject::Tag(tag_obj)) => Ok(ShowOutput::Tag(ShowTagData {
            tag_name: tag_obj.tag_name,
            tagger_name: Some(tag_obj.tagger.name.trim().to_string()),
            tagger_email: Some(tag_obj.tagger.email.trim().to_string()),
            tagger_date: chrono::DateTime::from_timestamp(tag_obj.tagger.timestamp as i64, 0)
                .map(|date| date.to_rfc3339()),
            message: tag_obj.message.trim().to_string(),
            target_hash: tag_obj.object_hash.to_string(),
            target_type: format!("{:?}", tag_obj.object_type).to_lowercase(),
        })),
        Ok(tag::TagObject::Commit(commit)) => collect_commit_output(&commit.id).await,
        Ok(_) => Err(CliError::fatal("tag points to unsupported object type")
            .with_stable_code(StableErrorCode::CliInvalidTarget)),
        Err(e) => {
            Err(CliError::fatal(e.to_string()).with_stable_code(StableErrorCode::RepoCorrupt))
        }
    }
}

async fn collect_tree_output(hash: &ObjectHash) -> CliResult<ShowOutput> {
    let tree = load_object::<Tree>(hash).map_err(|e| {
        CliError::fatal(format!("could not load tree {}: {}", hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    Ok(ShowOutput::Tree(ShowTreeData {
        entries: tree
            .tree_items
            .iter()
            .map(|item| ShowTreeEntry {
                mode: format!("{:06o}", tree_item_mode_to_u32(item.mode)),
                object_type: tree_item_mode_to_object_type(item.mode).to_string(),
                hash: item.id.to_string(),
                name: item.name.clone(),
            })
            .collect(),
    }))
}

async fn collect_blob_output(hash: &ObjectHash) -> CliResult<ShowOutput> {
    let blob = load_object::<Blob>(hash).map_err(|e| {
        CliError::fatal(format!("could not load blob {}: {}", hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    let content = String::from_utf8(blob.data.clone()).ok();

    Ok(ShowOutput::Blob(ShowBlobData {
        hash: hash.to_string(),
        size: blob.data.len(),
        is_binary: content.is_none(),
        content,
    }))
}

async fn collect_commit_file_output(rev: &str, file_path: &str) -> CliResult<ShowOutput> {
    let commit_hash = util::get_commit_base(rev)
        .await
        .map_err(|_| show_bad_revision_error(rev))?;
    let commit = load_object::<Commit>(&commit_hash).map_err(|e| {
        CliError::fatal(format!("could not load commit {}: {}", commit_hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    let tree = load_object::<Tree>(&commit.tree_id).map_err(|e| {
        CliError::fatal(format!("could not load tree {}: {}", commit.tree_id, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    let items = tree.get_plain_items();
    let target_path = PathBuf::from(file_path);

    if let Some((_, blob_hash)) = items.iter().find(|(path, _)| path == &target_path) {
        collect_blob_output(blob_hash).await
    } else {
        Err(
            CliError::fatal(format!("path '{}' does not exist in '{}'", file_path, rev))
                .with_stable_code(StableErrorCode::CliInvalidTarget),
        )
    }
}

fn split_subject_and_body(message: &str) -> (String, String) {
    let trimmed = parse_commit_msg(message).0.trim_end_matches('\n');
    match trimmed.split_once('\n') {
        Some((subject, body)) => (
            subject.to_string(),
            body.trim_start_matches('\n').to_string(),
        ),
        None => (trimmed.to_string(), String::new()),
    }
}

fn format_timestamp(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|date| date.to_rfc3339())
        .unwrap_or_else(|| timestamp.to_string())
}

fn change_type_name(change: ChangeType) -> &'static str {
    match change {
        ChangeType::Added => "added",
        ChangeType::Modified => "modified",
        ChangeType::Deleted => "deleted",
    }
}

fn tree_item_mode_to_u32(mode: TreeItemMode) -> u32 {
    match mode {
        TreeItemMode::Blob => 0o100644,
        TreeItemMode::BlobExecutable => 0o100755,
        TreeItemMode::Link => 0o120000,
        TreeItemMode::Tree => 0o040000,
        TreeItemMode::Commit => 0o160000,
    }
}

fn tree_item_mode_to_object_type(mode: TreeItemMode) -> &'static str {
    match mode {
        TreeItemMode::Blob | TreeItemMode::BlobExecutable => "blob",
        TreeItemMode::Link => "link",
        TreeItemMode::Tree => "tree",
        TreeItemMode::Commit => "commit",
    }
}

async fn collect_reference_names(commit_id: ObjectHash) -> Vec<String> {
    let mut refs = Vec::new();
    let head_branch = match (Head::current().await, Head::current_commit().await) {
        (Head::Branch(name), Some(head_commit)) if head_commit == commit_id => Some(name),
        _ => None,
    };

    for branch in Branch::list_branches(None).await {
        if branch.commit != commit_id {
            continue;
        }

        if head_branch.as_deref() == Some(branch.name.as_str()) {
            refs.push(format!("HEAD -> {}", branch.name));
        } else {
            refs.push(branch.name);
        }
    }

    match tag::list().await {
        Ok(tags) => {
            for tag in tags {
                let tagged_commit = match tag.object {
                    tag::TagObject::Commit(commit) => Some(commit.id),
                    tag::TagObject::Tag(tag_object) => Some(tag_object.object_hash),
                    _ => None,
                };
                if tagged_commit == Some(commit_id) {
                    refs.push(format!("tag: {}", tag.name));
                }
            }
        }
        Err(err) => tracing::warn!("failed to collect tag refs for show JSON output: {err}"),
    }

    refs.sort();
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_parsing() {
        // Default object is `HEAD`.
        let args = ShowArgs::try_parse_from(["show"]).unwrap();
        assert_eq!(args.object, None);
        assert!(!args.no_patch);
        assert!(!args.oneline);

        // Explicit object argument.
        let args = ShowArgs::try_parse_from(["show", "abc123"]).unwrap();
        assert_eq!(args.object, Some("abc123".to_string()));

        // `--no-patch` flag.
        let args = ShowArgs::try_parse_from(["show", "--no-patch"]).unwrap();
        assert!(args.no_patch);

        // `--oneline` flag.
        let args = ShowArgs::try_parse_from(["show", "--oneline"]).unwrap();
        assert!(args.oneline);

        // `--name-only` flag.
        let args = ShowArgs::try_parse_from(["show", "--name-only"]).unwrap();
        assert!(args.name_only);

        // `--stat` flag.
        let args = ShowArgs::try_parse_from(["show", "--stat"]).unwrap();
        assert!(args.stat);

        // `<revision>:<path>` syntax.
        let args = ShowArgs::try_parse_from(["show", "HEAD:test.txt"]).unwrap();
        assert_eq!(args.object, Some("HEAD:test.txt".to_string()));
    }

    #[test]
    fn test_tree_item_mode_helpers_use_git_modes_and_types() {
        assert_eq!(tree_item_mode_to_u32(TreeItemMode::Blob), 0o100644);
        assert_eq!(
            tree_item_mode_to_u32(TreeItemMode::BlobExecutable),
            0o100755
        );
        assert_eq!(tree_item_mode_to_u32(TreeItemMode::Link), 0o120000);
        assert_eq!(tree_item_mode_to_u32(TreeItemMode::Tree), 0o040000);
        assert_eq!(tree_item_mode_to_u32(TreeItemMode::Commit), 0o160000);

        assert_eq!(tree_item_mode_to_object_type(TreeItemMode::Blob), "blob");
        assert_eq!(
            tree_item_mode_to_object_type(TreeItemMode::BlobExecutable),
            "blob"
        );
        assert_eq!(tree_item_mode_to_object_type(TreeItemMode::Link), "link");
        assert_eq!(tree_item_mode_to_object_type(TreeItemMode::Tree), "tree");
        assert_eq!(
            tree_item_mode_to_object_type(TreeItemMode::Commit),
            "commit"
        );
    }
}
