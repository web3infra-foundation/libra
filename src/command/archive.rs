//! Command-line surface for creating archives from committed tree snapshots.

use std::path::{Component, Path, PathBuf};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{
        commit::Commit,
        tree::{Tree, TreeItemMode},
    },
};

use crate::{
    command::load_object,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        util,
    },
};

const ARCHIVE_EXAMPLES: &str = "\
EXAMPLES:
    libra archive -o project.tar HEAD
    libra archive --format=tar.gz --prefix=project-v1/ -o project-v1.tar.gz v1.0
    libra archive --format=zip -o feature.zip feature-branch";

/// Supported archive output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    /// Uncompressed tarball.
    Tar,
    /// Gzip-compressed tarball.
    TarGz,
    /// Bzip2-compressed tarball.
    TarBz2,
    /// ZIP archive.
    Zip,
}

impl ArchiveFormat {
    /// All supported format name strings, listed in preferred order.
    const ALL: &[&str] = &["tar", "tar.gz", "tar.bz2", "zip"];

    /// Parse a format string strictly, returning an error for unknown formats.
    fn parse_strict(value: &str) -> Result<Self, String> {
        match value {
            "tar" => Ok(Self::Tar),
            "tar.gz" | "tgz" => Ok(Self::TarGz),
            "tar.bz2" | "tbz2" | "tbz" => Ok(Self::TarBz2),
            "zip" => Ok(Self::Zip),
            other => Err(format!(
                "unknown archive format: '{other}'. Supported formats: {}",
                Self::ALL.join(", ")
            )),
        }
    }
}

/// Create an archive of files from a named tree.
#[derive(Parser, Debug)]
#[command(after_help = ARCHIVE_EXAMPLES)]
pub struct ArchiveArgs {
    /// Commit, branch, tag, or abbreviated commit hash to archive. Defaults to HEAD.
    #[arg(default_value = "HEAD", value_name = "TREEISH")]
    pub treeish: String,

    /// Archive format: tar, tar.gz, tar.bz2, or zip.
    #[arg(short = 'f', long, default_value = "tar", value_name = "FMT")]
    pub format: String,

    /// Write archive bytes to a file instead of stdout.
    #[arg(short = 'o', long, value_name = "FILE")]
    pub output: Option<String>,

    /// Prepend a relative directory prefix to each archived path.
    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,
}

/// Collected metadata about a single tree entry for archiving.
struct ArchiveEntry {
    /// The logical path within the archive before the optional prefix is applied.
    path: PathBuf,
    /// The blob hash to read content from.
    hash: ObjectHash,
    /// The file mode from the tree entry.
    mode: TreeItemMode,
}

/// Recursively collect archiveable file entries from a tree.
fn collect_tree_entries(
    tree: &Tree,
    base: &Path,
    entries: &mut Vec<ArchiveEntry>,
) -> Result<(), CliError> {
    for item in &tree.tree_items {
        let path = base.join(&item.name);
        match item.mode {
            TreeItemMode::Tree => {
                let sub_tree: Tree = load_object(&item.id).map_err(|error| {
                    CliError::fatal(format!(
                        "failed to load subtree '{}' at '{}': {error}",
                        item.id,
                        path.display()
                    ))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
                })?;
                collect_tree_entries(&sub_tree, &path, entries)?;
            }
            TreeItemMode::Commit => {
                // Gitlink/submodule entries point at commits that Libra does not
                // materialize as files.
            }
            _ => entries.push(ArchiveEntry {
                path,
                hash: item.id,
                mode: item.mode,
            }),
        }
    }

    Ok(())
}

fn entry_has_archive_metadata(entry: &ArchiveEntry) -> bool {
    !entry.path.as_os_str().is_empty()
        && !entry.hash.to_string().is_empty()
        && !matches!(entry.mode, TreeItemMode::Tree | TreeItemMode::Commit)
}

/// Resolve a tree-ish string to the archiveable entries from that commit tree.
async fn resolve_entries(treeish: &str) -> Result<Vec<ArchiveEntry>, CliError> {
    let commit_hash = util::get_commit_base(treeish).await.map_err(|error| {
        CliError::fatal(format!("failed to resolve '{treeish}': {error}"))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;

    let commit = load_object::<Commit>(&commit_hash).map_err(|error| {
        CliError::fatal(format!("failed to load commit {commit_hash}: {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    let tree: Tree = load_object(&commit.tree_id).map_err(|error| {
        CliError::fatal(format!(
            "failed to load tree {} for commit {commit_hash}: {error}",
            commit.tree_id
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    let mut entries = Vec::new();
    collect_tree_entries(&tree, Path::new(""), &mut entries)?;
    Ok(entries)
}

/// Validate a user-supplied archive prefix before it is joined with file paths.
fn validate_prefix(prefix: Option<&str>) -> Result<Option<PathBuf>, CliError> {
    let Some(prefix) = prefix else {
        return Ok(None);
    };

    if prefix.is_empty() {
        return Ok(Some(PathBuf::new()));
    }

    let path = Path::new(prefix);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CliError::command_usage(format!(
            "invalid archive prefix '{prefix}': use a relative path without '..'"
        ))
        .with_stable_code(StableErrorCode::CliInvalidArguments));
    }

    Ok(Some(path.to_path_buf()))
}

/// # Side Effects
///
/// Reads commit and tree objects from the local object store.
///
/// # Errors
///
/// Returns `CliInvalidArguments` for unsupported formats or unsafe prefixes.
/// Returns `CliInvalidTarget` when the tree-ish cannot be resolved.
/// Returns `RepoCorrupt` when referenced commit or tree objects cannot be read.
/// Returns `Unsupported` until archive creation is implemented.
pub async fn execute_safe(args: ArchiveArgs, _output: &OutputConfig) -> CliResult<()> {
    let _format = ArchiveFormat::parse_strict(&args.format).map_err(|message| {
        CliError::command_usage(message).with_stable_code(StableErrorCode::CliInvalidArguments)
    })?;
    let _prefix = validate_prefix(args.prefix.as_deref())?;
    let entries = resolve_entries(&args.treeish).await?;

    if entries.is_empty() {
        return Err(CliError::fatal(format!(
            "tree '{}' contains no files to archive",
            args.treeish
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget));
    }
    debug_assert!(entries.iter().all(entry_has_archive_metadata));

    Err(CliError::failure(
        "archive command is registered but archive creation is not implemented yet",
    )
    .with_stable_code(StableErrorCode::Unsupported))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use git_internal::internal::object::tree::TreeItem;

    use super::*;

    #[test]
    fn archive_format_accepts_supported_names() {
        assert_eq!(
            ArchiveFormat::parse_strict("tar").unwrap(),
            ArchiveFormat::Tar
        );
        assert_eq!(
            ArchiveFormat::parse_strict("tar.gz").unwrap(),
            ArchiveFormat::TarGz
        );
        assert_eq!(
            ArchiveFormat::parse_strict("tgz").unwrap(),
            ArchiveFormat::TarGz
        );
        assert_eq!(
            ArchiveFormat::parse_strict("tar.bz2").unwrap(),
            ArchiveFormat::TarBz2
        );
        assert_eq!(
            ArchiveFormat::parse_strict("tbz2").unwrap(),
            ArchiveFormat::TarBz2
        );
        assert_eq!(
            ArchiveFormat::parse_strict("tbz").unwrap(),
            ArchiveFormat::TarBz2
        );
        assert_eq!(
            ArchiveFormat::parse_strict("zip").unwrap(),
            ArchiveFormat::Zip
        );
    }

    #[test]
    fn archive_format_rejects_unknown_names() {
        let err = ArchiveFormat::parse_strict("rar").unwrap_err();

        assert!(err.contains("unknown archive format"));
        assert!(err.contains("tar.gz"));
        assert!(ArchiveFormat::parse_strict("").is_err());
    }

    #[test]
    fn validate_prefix_accepts_safe_relative_paths() {
        assert_eq!(validate_prefix(None).unwrap(), None);
        assert_eq!(
            validate_prefix(Some("release/")).unwrap(),
            Some(PathBuf::from("release/"))
        );
        assert_eq!(
            validate_prefix(Some("nested/release")).unwrap(),
            Some(PathBuf::from("nested/release"))
        );
        assert_eq!(validate_prefix(Some("")).unwrap(), Some(PathBuf::new()));
    }

    #[test]
    fn validate_prefix_rejects_archive_slip_paths() {
        assert!(validate_prefix(Some("../release")).is_err());
        assert!(validate_prefix(Some("release/../other")).is_err());
        assert!(validate_prefix(Some("/tmp/release")).is_err());
    }

    #[test]
    fn collect_tree_entries_keeps_blob_metadata() {
        let hash =
            ObjectHash::from_str("8ab686eafeb1f44702738c8b0f24f2567c36da6d").expect("valid hash");
        let tree = Tree::from_tree_items(vec![
            TreeItem::new(TreeItemMode::Blob, hash, "README.md".to_string()),
            TreeItem::new(TreeItemMode::BlobExecutable, hash, "script.sh".to_string()),
        ])
        .expect("valid test tree");
        let mut entries = Vec::new();

        collect_tree_entries(&tree, Path::new("docs"), &mut entries).expect("collect entries");

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("docs/README.md"));
        assert_eq!(entries[0].hash, hash);
        assert_eq!(entries[0].mode, TreeItemMode::Blob);
        assert_eq!(entries[1].path, PathBuf::from("docs/script.sh"));
        assert_eq!(entries[1].mode, TreeItemMode::BlobExecutable);
    }

    #[test]
    fn collect_tree_entries_skips_gitlinks() {
        let hash =
            ObjectHash::from_str("8ab686eafeb1f44702738c8b0f24f2567c36da6d").expect("valid hash");
        let tree = Tree::from_tree_items(vec![
            TreeItem::new(TreeItemMode::Commit, hash, "submodule".to_string()),
            TreeItem::new(TreeItemMode::Blob, hash, "README.md".to_string()),
        ])
        .expect("valid test tree");
        let mut entries = Vec::new();

        collect_tree_entries(&tree, Path::new(""), &mut entries).expect("collect entries");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("README.md"));
    }
}
