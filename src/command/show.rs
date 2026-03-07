//! Show command that resolves object IDs and prints commit, tree, blob, or ref details with formatting suitable for diffable objects.

use std::{path::PathBuf, str::FromStr};

use clap::Parser;
use colored::Colorize;
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
};

use crate::{
    command::{
        load_object,
        log::{ChangeType, generate_diff, get_changed_files_for_commit},
    },
    common_utils::parse_commit_msg,
    internal::tag,
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult},
        object_ext::TreeExt,
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

/// Executes the show command.
pub async fn execute(args: ShowArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Resolves a revision (commit, tag, tree, blob, or
/// `<rev>:<path>`) and prints its contents with diff formatting.
pub async fn execute_safe(args: ShowArgs) -> CliResult<()> {
    let object_ref = args.object.as_deref().unwrap_or("HEAD");

    // Handle `<revision>:<path>` lookups before generic revision resolution.
    if let Some((rev, path)) = object_ref.split_once(':') {
        return show_commit_file(rev, path, &args).await;
    }

    // Resolve refs first so tags keep their custom rendering.
    if let Ok(commit_hash) = util::get_commit_base(object_ref).await {
        // Use find_tag_and_commit to check if it's a tag and get tag info
        match tag::find_tag_and_commit(object_ref).await {
            Ok(Some((object, _))) => {
                // It is a tag - show tag first
                if object.get_type() == ObjectType::Tag {
                    // For annotated tags, show tag details
                    let tag_hash = if let tag::TagObject::Tag(tag_obj) = &object {
                        tag_obj.id
                    } else {
                        commit_hash
                    };
                    return show_tag_by_hash(&tag_hash, &args).await;
                } else {
                    // Lightweight tag points directly to commit
                    return show_commit(&commit_hash, &args).await;
                }
            }
            _ => {
                // Not a tag or tag doesn't exist, show as commit
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

        let obj_type = storage
            .get_object_type(hash)
            .map_err(|e| CliError::fatal(format!("could not read object {}: {}", hash, e)))?;

        match obj_type {
            ObjectType::Commit => show_commit(hash, args).await,
            ObjectType::Tag => show_tag_by_hash(hash, args).await,
            ObjectType::Tree => show_tree(hash).await,
            ObjectType::Blob => show_blob(hash).await,
            _ => Err(CliError::fatal(format!(
                "unsupported object type for {}",
                hash
            ))),
        }
    })
}

/// Shows a commit together with optional diff output.
async fn show_commit(commit_hash: &ObjectHash, args: &ShowArgs) -> CliResult<()> {
    // Load the commit before rendering any metadata or diff output.
    let commit = load_object::<Commit>(commit_hash)
        .map_err(|e| CliError::fatal(format!("could not load commit {}: {}", commit_hash, e)))?;

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
            return Err(CliError::fatal("tag points to unsupported object type"));
        }
        Err(e) => {
            return Err(CliError::fatal(e.to_string()));
        }
    }
    Ok(())
}

/// Shows a tree object.
async fn show_tree(hash: &ObjectHash) -> CliResult<()> {
    let tree = load_object::<Tree>(hash)
        .map_err(|e| CliError::fatal(format!("could not load tree {}: {}", hash, e)))?;

    println!("{} {}\n", "tree".yellow(), hash);

    for item in &tree.tree_items {
        println!(
            "{:06o} {:?} {}\t{}",
            item.mode as u32, item.mode, item.id, item.name
        );
    }
    Ok(())
}

/// Shows a blob as text when possible.
async fn show_blob(hash: &ObjectHash) -> CliResult<()> {
    let blob = load_object::<Blob>(hash)
        .map_err(|e| CliError::fatal(format!("could not load blob {}: {}", hash, e)))?;

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

    let commit = load_object::<Commit>(&commit_hash)
        .map_err(|e| CliError::fatal(format!("could not load commit {}: {}", commit_hash, e)))?;

    // Load the tree for the resolved commit.
    let tree = load_object::<Tree>(&commit.tree_id)
        .map_err(|e| CliError::fatal(format!("could not load tree {}: {}", commit.tree_id, e)))?;

    // Find the target path inside the tree.
    let items = tree.get_plain_items();
    let target_path = PathBuf::from(file_path);

    if let Some((_, blob_hash)) = items.iter().find(|(path, _)| path == &target_path) {
        show_blob(blob_hash).await?;
    } else {
        return Err(CliError::fatal(format!(
            "path '{}' does not exist in '{}'",
            file_path, rev
        )));
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
    .with_hint("use '--' to separate paths from revisions, for example 'libra show -- <file>'.")
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
}
