//! `libra bundle` — create and inspect Git v2 bundle files, a focused subset of
//! `git bundle`.
//!
//! A bundle is a small text header followed by a pack:
//!
//! ```text
//! # v2 git bundle
//! <tip-oid> <ref-name>      (one per included ref)
//!                           (blank line)
//! PACK……                    (a v2 pack of every object reachable from the tips)
//! ```
//!
//! This version writes full (non-thin, no-prerequisite) bundles and can
//! `verify` / `list-heads` any v2 bundle. Prerequisite (incremental) bundles,
//! `unbundle`, and rev-range arguments are deferred.

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{Parser, Subcommand};
use git_internal::{
    hash::{ObjectHash, get_hash_kind, set_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
        object::{
            blob::Blob,
            tree::{Tree, TreeItemMode},
        },
        pack::{encode::PackEncoder, entry::Entry},
    },
};
use tokio::sync::mpsc;

use crate::{
    command::load_object,
    internal::head::Head,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        util,
    },
};

const BUNDLE_SIGNATURE_V2: &str = "# v2 git bundle";

pub const BUNDLE_EXAMPLES: &str = "\
EXAMPLES:
    libra bundle create repo.bundle main      Bundle everything reachable from main
    libra bundle create all.bundle HEAD       Bundle the current branch
    libra bundle verify repo.bundle           Check a bundle's header and pack
    libra bundle list-heads repo.bundle       List the refs a bundle carries";

/// Create and inspect Git v2 bundle files.
#[derive(Parser, Debug)]
#[command(after_help = BUNDLE_EXAMPLES)]
pub struct BundleArgs {
    #[command(subcommand)]
    pub command: BundleSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum BundleSubcommand {
    /// Write a bundle of everything reachable from the given revisions.
    Create {
        /// The bundle file to write.
        #[clap(value_name = "FILE")]
        file: PathBuf,
        /// Revisions whose reachable history to include (each becomes a head).
        #[clap(value_name = "REV", required = true)]
        revs: Vec<String>,
    },
    /// Check that a bundle's header is well-formed and its pack is present.
    Verify {
        #[clap(value_name = "FILE")]
        file: PathBuf,
    },
    /// Print the `<oid> <ref>` head lines a bundle carries.
    ListHeads {
        #[clap(value_name = "FILE")]
        file: PathBuf,
    },
}

pub async fn execute(args: BundleArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

pub async fn execute_safe(args: BundleArgs, _output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    match args.command {
        BundleSubcommand::Create { file, revs } => create(&file, &revs).await,
        BundleSubcommand::Verify { file } => verify(&file),
        BundleSubcommand::ListHeads { file } => list_heads(&file),
    }
}

// ----------------------------------------------------------------------------
// create
// ----------------------------------------------------------------------------

async fn create(file: &Path, revs: &[String]) -> CliResult<()> {
    // Resolve each rev to (tip oid, ref name) — these become the bundle heads.
    let mut heads: Vec<(ObjectHash, String)> = Vec::new();
    for rev in revs {
        let tip = util::get_commit_base(rev).await.map_err(|error| {
            CliError::fatal(format!("not a valid revision '{rev}': {error}"))
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::CliInvalidTarget)
        })?;
        heads.push((tip, resolve_ref_name(rev).await));
    }

    // Collect every object reachable from the tips (deduplicated).
    let mut seen: HashSet<ObjectHash> = HashSet::new();
    let mut entries: Vec<Entry> = Vec::new();
    for (tip, _) in &heads {
        let commits = crate::command::log::get_reachable_commits(tip.to_string(), None)
            .await
            .map_err(|error| error.with_exit_code(128))?;
        for commit in commits {
            let tree_id = commit.tree_id;
            if seen.insert(commit.id) {
                entries.push(commit.into());
            }
            collect_tree(&tree_id, &mut seen, &mut entries)?;
        }
    }

    if entries.is_empty() {
        return Err(
            CliError::fatal("bundle would contain no objects".to_string())
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::CliInvalidTarget),
        );
    }

    let pack = encode_pack(entries).await?;

    // Write header + pack to a temporary file, then rename into place so a
    // failure never leaves a half-written bundle.
    let parent = file.parent().filter(|p| !p.as_os_str().is_empty());
    let tmp = match parent {
        Some(dir) => dir.join(format!(
            ".{}.tmp",
            file.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("bundle")
        )),
        None => PathBuf::from(format!(
            ".{}.tmp",
            file.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("bundle")
        )),
    };

    let write_result = (|| -> std::io::Result<()> {
        let mut out = std::io::BufWriter::new(fs::File::create(&tmp)?);
        writeln!(out, "{BUNDLE_SIGNATURE_V2}")?;
        for (oid, name) in &heads {
            writeln!(out, "{oid} {name}")?;
        }
        out.write_all(b"\n")?;
        out.write_all(&pack)?;
        out.flush()
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(write_err(error));
    }
    if let Err(error) = fs::rename(&tmp, file) {
        let _ = fs::remove_file(&tmp);
        return Err(write_err(error));
    }

    println!(
        "Created bundle '{}' with {} ref(s).",
        file.display(),
        heads.len()
    );
    Ok(())
}

/// Recursively add a tree and everything beneath it to the object set.
fn collect_tree(
    tree_id: &ObjectHash,
    seen: &mut HashSet<ObjectHash>,
    entries: &mut Vec<Entry>,
) -> CliResult<()> {
    if !seen.insert(*tree_id) {
        return Ok(());
    }
    let tree: Tree = load_object(tree_id).map_err(|error| object_error(tree_id, error))?;
    for item in &tree.tree_items {
        match item.mode {
            TreeItemMode::Tree => collect_tree(&item.id, seen, entries)?,
            // A gitlink points at a commit in another repository; not our object.
            TreeItemMode::Commit => {}
            _ => {
                if seen.insert(item.id) {
                    let blob: Blob =
                        load_object(&item.id).map_err(|error| object_error(&item.id, error))?;
                    entries.push(blob.into());
                }
            }
        }
    }
    entries.push(tree.into());
    Ok(())
}

/// Encode objects into a v2 pack, hashing with the repository's hash kind.
async fn encode_pack(entries: Vec<Entry>) -> CliResult<Vec<u8>> {
    let count = entries.len();
    let (pack_tx, mut pack_rx) = mpsc::channel::<Vec<u8>>(128);
    let (entry_tx, entry_rx) = mpsc::channel::<MetaAttached<Entry, EntryMeta>>(count.max(1));
    let mut encoder = PackEncoder::new(count, 0, pack_tx);
    let kind = get_hash_kind();
    let handle = tokio::spawn(async move {
        set_hash_kind(kind);
        encoder.encode(entry_rx).await
    });

    for entry in entries {
        entry_tx
            .send(MetaAttached {
                inner: entry,
                meta: EntryMeta::new(),
            })
            .await
            .map_err(|error| {
                CliError::fatal(format!("failed to feed pack encoder: {error}"))
                    .with_exit_code(128)
                    .with_stable_code(StableErrorCode::InternalInvariant)
            })?;
    }
    drop(entry_tx);

    let mut bytes = Vec::new();
    while let Some(chunk) = pack_rx.recv().await {
        bytes.extend_from_slice(&chunk);
    }
    handle
        .await
        .map_err(|error| {
            CliError::fatal(format!("pack encoder task failed: {error}"))
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::InternalInvariant)
        })?
        .map_err(|error| {
            CliError::fatal(format!("pack encoding failed: {error}"))
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::InternalInvariant)
        })?;
    Ok(bytes)
}

// ----------------------------------------------------------------------------
// verify / list-heads
// ----------------------------------------------------------------------------

/// The parsed text header of a bundle.
struct BundleHeader {
    prerequisites: Vec<(String, String)>,
    heads: Vec<(String, String)>,
    /// Byte offset where the pack begins (just after the blank line).
    pack_offset: usize,
}

fn verify(file: &Path) -> CliResult<()> {
    let bytes = fs::read(file).map_err(read_err)?;
    let header = parse_header(&bytes)?;

    // Any prerequisite object must already exist locally.
    let storage = util::objects_storage();
    let mut missing = Vec::new();
    for (oid, _) in &header.prerequisites {
        match ObjectHash::from_str(oid) {
            Ok(hash) if storage.get(&hash).is_ok() => {}
            _ => missing.push(oid.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(CliError::fatal(format!(
            "bundle requires objects this repository does not have:\n  {}",
            missing.join("\n  ")
        ))
        .with_exit_code(1)
        .with_stable_code(StableErrorCode::CliInvalidTarget));
    }

    // The pack must start with the v2 PACK magic.
    let pack = &bytes[header.pack_offset..];
    if pack.len() < 8 || &pack[0..4] != b"PACK" || pack[4..8] != [0, 0, 0, 2] {
        return Err(
            CliError::fatal("bundle pack is missing or not a version-2 pack".to_string())
                .with_exit_code(1)
                .with_stable_code(StableErrorCode::CliInvalidTarget),
        );
    }

    println!("{} is okay", file.display());
    for (oid, name) in &header.heads {
        println!("{oid} {name}");
    }
    Ok(())
}

fn list_heads(file: &Path) -> CliResult<()> {
    let bytes = fs::read(file).map_err(read_err)?;
    let header = parse_header(&bytes)?;
    for (oid, name) in &header.heads {
        println!("{oid} {name}");
    }
    Ok(())
}

/// Parse the text header up to the blank line that precedes the pack.
fn parse_header(bytes: &[u8]) -> CliResult<BundleHeader> {
    // A malformed bundle is a verification failure (exit 1), matching
    // `git bundle verify` — exit 128 is reserved for usage errors.
    let invalid = |message: &str| {
        CliError::fatal(format!("not a valid bundle: {message}"))
            .with_exit_code(1)
            .with_stable_code(StableErrorCode::CliInvalidTarget)
    };

    let mut prerequisites = Vec::new();
    let mut heads = Vec::new();
    let mut offset = 0usize;
    let mut first_line = true;

    loop {
        let Some(nl) = bytes[offset..].iter().position(|&b| b == b'\n') else {
            return Err(invalid("missing header terminator"));
        };
        let line = &bytes[offset..offset + nl];
        let next = offset + nl + 1;

        if first_line {
            if line != BUNDLE_SIGNATURE_V2.as_bytes() {
                if line.starts_with(b"# v3 git bundle") {
                    return Err(invalid("v3 bundles are not supported"));
                }
                return Err(invalid("missing `# v2 git bundle` signature"));
            }
            first_line = false;
            offset = next;
            continue;
        }

        if line.is_empty() {
            // Blank line: header ends, pack begins.
            return Ok(BundleHeader {
                prerequisites,
                heads,
                pack_offset: next,
            });
        }

        let text = std::str::from_utf8(line).map_err(|_| invalid("non-UTF-8 header line"))?;
        if let Some(rest) = text.strip_prefix('-') {
            // Prerequisite: `-<oid> [comment]`.
            let (oid, comment) = split_oid_rest(rest);
            prerequisites.push((oid, comment));
        } else {
            let (oid, name) = split_oid_rest(text);
            heads.push((oid, name));
        }
        offset = next;
    }
}

/// Split a header line into its leading oid and the remaining label/comment.
fn split_oid_rest(line: &str) -> (String, String) {
    match line.split_once(' ') {
        Some((oid, rest)) => (oid.to_string(), rest.to_string()),
        None => (line.to_string(), String::new()),
    }
}

// ----------------------------------------------------------------------------
// shared helpers
// ----------------------------------------------------------------------------

/// Resolve the ref name a revision should be recorded under in the header.
async fn resolve_ref_name(rev: &str) -> String {
    if rev == "HEAD" {
        return match Head::current().await {
            Head::Branch(name) => format!("refs/heads/{name}"),
            Head::Detached(_) => "HEAD".to_string(),
        };
    }
    if rev.starts_with("refs/") {
        rev.to_string()
    } else {
        format!("refs/heads/{rev}")
    }
}

fn object_error(id: &ObjectHash, error: git_internal::errors::GitError) -> CliError {
    CliError::fatal(format!("failed to load object {id}: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

/// A bundle that cannot be read is a verification failure (exit 1), matching
/// `git bundle verify` — only usage errors use 128.
fn read_err(error: std::io::Error) -> CliError {
    CliError::fatal(format!("failed to read bundle: {error}"))
        .with_exit_code(1)
        .with_stable_code(StableErrorCode::CliInvalidTarget)
}

fn write_err(error: std::io::Error) -> CliError {
    CliError::fatal(format!("failed to write bundle: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::IoWriteFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_v2_header_with_heads() {
        let bytes = b"# v2 git bundle\nabc123 refs/heads/main\n\nPACK";
        let header = parse_header(bytes).unwrap();
        assert_eq!(
            header.heads,
            vec![("abc123".into(), "refs/heads/main".into())]
        );
        assert_eq!(&bytes[header.pack_offset..], b"PACK");
    }

    #[test]
    fn parses_prerequisites() {
        let bytes = b"# v2 git bundle\n-dead comment here\nbeef refs/heads/x\n\nPACK";
        let header = parse_header(bytes).unwrap();
        assert_eq!(header.prerequisites.len(), 1);
        assert_eq!(header.prerequisites[0].0, "dead");
        assert_eq!(header.heads.len(), 1);
    }

    #[test]
    fn rejects_a_missing_signature() {
        assert!(parse_header(b"not a bundle\n\nPACK").is_err());
    }

    #[test]
    fn rejects_v3() {
        assert!(parse_header(b"# v3 git bundle\n\nPACK").is_err());
    }
}
