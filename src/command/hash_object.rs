//! Implements `hash-object` for computing Git-compatible blob object IDs.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::object::{blob::Blob, types::ObjectType},
};
use serde::Serialize;

use crate::{
    command::save_object,
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        path,
    },
};

const HASH_OBJECT_EXAMPLES: &str = "\
EXAMPLES:
    libra hash-object README.md                         Compute the blob id (no write)
    libra hash-object -w src/main.rs                    Compute and write the object to .libra/objects/
    printf 'hello' | libra hash-object --stdin          Hash stdin instead of a file
    printf 'a.txt\\nb.txt\\n' | libra hash-object --stdin-paths    Hash paths listed on stdin
    libra hash-object -t commit -w commit.txt           Hash and write a commit object
    libra hash-object -t tag --literally bad.txt        Hash content as-is, skipping validation
    printf 'hello' | libra hash-object --stdin --json   Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = HASH_OBJECT_EXAMPLES)]
pub struct HashObjectArgs {
    /// Actually write the object into the object database
    #[arg(short = 'w', long)]
    pub write: bool,

    /// Read the object contents from standard input
    #[arg(long, conflicts_with_all = ["paths", "stdin_paths"])]
    pub stdin: bool,

    /// Read newline-delimited file paths from standard input
    #[arg(long = "stdin-paths", conflicts_with_all = ["paths", "stdin"])]
    pub stdin_paths: bool,

    /// Object type to hash: `blob` (default), `commit`, `tree`, or `tag`.
    #[arg(
        short = 't',
        long = "type",
        default_value = "blob",
        value_name = "TYPE"
    )]
    pub object_type: String,

    /// Hash the content as-is, without checking that it is a well-formed object
    /// of the given type.
    #[arg(long)]
    pub literally: bool,

    /// File paths to hash
    #[arg(
        value_name = "PATH",
        required_unless_present_any = ["stdin", "stdin_paths"]
    )]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct HashObjectOutput {
    object_type: String,
    write: bool,
    objects: Vec<HashObjectEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct HashObjectEntry {
    source: String,
    oid: String,
    size: usize,
    written: bool,
}

pub async fn execute(args: HashObjectArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// # Side Effects
///
/// With `-w`/`--write`, stores each computed blob object in the current
/// repository object database. Without `--write`, this command only reads input
/// and prints Git-compatible object IDs.
///
/// # Errors
///
/// Returns structured CLI errors for unsupported object types, unreadable input,
/// object-write failures, and stdout write failures.
pub async fn execute_safe(args: HashObjectArgs, output: &OutputConfig) -> CliResult<()> {
    // Resolve the object type before any input is read, so an unsupported type
    // is reported as a usage error up front.
    let object_type = resolve_object_type(&args.object_type)?;

    if output.is_json() {
        let result = hash_objects(&args, object_type)?;
        return render_hash_object_output(&result, output);
    }

    hash_objects_streaming(&args, output, object_type)
}

/// Resolve a `-t <type>` argument to a Git object type. Only the four Git object
/// types are accepted; anything else is a usage error (exit 129).
fn resolve_object_type(value: &str) -> CliResult<ObjectType> {
    match value {
        "blob" => Ok(ObjectType::Blob),
        "commit" => Ok(ObjectType::Commit),
        "tree" => Ok(ObjectType::Tree),
        "tag" => Ok(ObjectType::Tag),
        other => Err(
            CliError::fatal(format!("unsupported object type '{other}'"))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("supported object types: blob, commit, tree, tag"),
        ),
    }
}

fn hash_objects(args: &HashObjectArgs, object_type: ObjectType) -> CliResult<HashObjectOutput> {
    let objects = if args.stdin {
        vec![hash_one_source(
            "-",
            read_stdin()?,
            args.write,
            object_type,
            args.literally,
        )?]
    } else if args.stdin_paths {
        hash_path_entries(
            &read_stdin_paths()?,
            args.write,
            object_type,
            args.literally,
        )?
    } else {
        hash_path_entries(&args.paths, args.write, object_type, args.literally)?
    };

    Ok(HashObjectOutput {
        object_type: object_type.to_string(),
        write: args.write,
        objects,
    })
}

fn hash_objects_streaming(
    args: &HashObjectArgs,
    output: &OutputConfig,
    object_type: ObjectType,
) -> CliResult<()> {
    if output.quiet {
        return hash_objects(args, object_type).map(|_| ());
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();

    if args.stdin {
        let entry = hash_one_source("-", read_stdin()?, args.write, object_type, args.literally)?;
        write_hash_line(&mut writer, &entry.oid)?;
        return Ok(());
    }

    let paths = if args.stdin_paths {
        read_stdin_paths()?
    } else {
        args.paths.clone()
    };
    for path in paths {
        let entry = hash_one_source(
            path.display().to_string(),
            read_file(&path)?,
            args.write,
            object_type,
            args.literally,
        )?;
        write_hash_line(&mut writer, &entry.oid)?;
    }

    Ok(())
}

fn hash_path_entries(
    paths: &[PathBuf],
    write: bool,
    object_type: ObjectType,
    literally: bool,
) -> CliResult<Vec<HashObjectEntry>> {
    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        entries.push(hash_one_source(
            path.display().to_string(),
            read_file(path)?,
            write,
            object_type,
            literally,
        )?);
    }
    Ok(entries)
}

fn hash_one_source(
    source: impl Into<String>,
    data: Vec<u8>,
    write: bool,
    object_type: ObjectType,
    literally: bool,
) -> CliResult<HashObjectEntry> {
    let size = data.len();

    // Unless `--literally` is given, non-blob input must be a well-formed object.
    if !literally && object_type != ObjectType::Blob {
        validate_object_format(object_type, &data)?;
    }

    let oid = if object_type == ObjectType::Blob {
        let blob = Blob::from_content_bytes(data);
        let oid = blob.id.to_string();
        if write {
            save_object(&blob, &blob.id).map_err(|error| write_object_error(&oid, &error))?;
        }
        oid
    } else {
        let oid_hash = ObjectHash::from_type_and_data(object_type, &data);
        let oid = oid_hash.to_string();
        if write {
            save_raw_object(&oid_hash, &data, object_type)
                .map_err(|error| write_object_error(&oid, &error))?;
        }
        oid
    };

    Ok(HashObjectEntry {
        source: source.into(),
        oid,
        size,
        written: write,
    })
}

/// Build the fatal error for an object-write failure.
fn write_object_error(oid: &str, error: &impl std::fmt::Display) -> CliError {
    CliError::fatal(format!("failed to write object {oid}: {error}"))
        .with_stable_code(StableErrorCode::IoWriteFailed)
        .with_hint("check repository object storage permissions and available disk space.")
}

/// Persist a raw object body (commit/tree/tag) into the object database. `put`
/// prepends the `<type> <len>\0` header internally.
fn save_raw_object(
    oid: &ObjectHash,
    data: &[u8],
    object_type: ObjectType,
) -> Result<(), io::Error> {
    let storage = ClientStorage::init(path::objects());
    storage.put(oid, data, object_type).map(|_| ())
}

fn read_file(path: &Path) -> CliResult<Vec<u8>> {
    fs::read(path).map_err(|error| {
        CliError::fatal(format!(
            "failed to read '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
        .with_hint("verify the path exists and is readable.")
    })
}

fn read_stdin() -> CliResult<Vec<u8>> {
    let mut data = Vec::new();
    io::stdin().read_to_end(&mut data).map_err(|error| {
        CliError::fatal(format!("failed to read standard input: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    Ok(data)
}

fn read_stdin_paths() -> CliResult<Vec<PathBuf>> {
    let data = read_stdin()?;
    let text = String::from_utf8(data).map_err(|error| {
        CliError::fatal(format!(
            "failed to parse standard input paths as UTF-8: {error}"
        ))
        .with_stable_code(StableErrorCode::CliInvalidArguments)
    })?;
    Ok(text.lines().map(PathBuf::from).collect())
}

fn render_hash_object_output(result: &HashObjectOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("hash-object", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();
    for entry in &result.objects {
        write_hash_line(&mut writer, &entry.oid)?;
    }
    Ok(())
}

fn write_hash_line<W: Write>(writer: &mut W, oid: &str) -> CliResult<()> {
    match writeln!(writer, "{oid}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(
            CliError::fatal(format!("failed to write hash-object output: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed),
        ),
    }
}

fn format_io_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "No such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => error.to_string(),
    }
}

/// Build the fatal "corrupt object" error (exit 128) for a failed validation.
fn corrupt_object_error(object_type: ObjectType, reason: &str) -> CliError {
    CliError::fatal(format!("corrupt {object_type} object: {reason}"))
        .with_stable_code(StableErrorCode::RepoCorrupt)
        .with_hint("pass --literally to hash the content as-is without validation.")
}

/// Validate that `data` is a well-formed object of `object_type`. These checks
/// run entirely at the command layer (no `from_bytes`), so malformed input
/// produces a clean error rather than a panic.
fn validate_object_format(object_type: ObjectType, data: &[u8]) -> CliResult<()> {
    match object_type {
        ObjectType::Blob => Ok(()),
        ObjectType::Commit => validate_commit_format(data),
        ObjectType::Tag => validate_tag_format(data),
        ObjectType::Tree => validate_tree_format(data),
        // `resolve_object_type` restricts callers to the four Git types.
        _ => Ok(()),
    }
}

/// Whether `line` is a `<prefix> <value>` header with a non-empty value.
fn header_has_value(line: &str, prefix: &str) -> bool {
    line.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix(' '))
        .is_some_and(|value| !value.is_empty())
}

/// Whether `line` is a `<prefix> <oid>` header whose oid is a hex string of the
/// repository's hash width.
fn is_header_oid(line: &str, prefix: &str) -> bool {
    let Some(rest) = line
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return false;
    };
    rest.len() == get_hash_kind().size() * 2 && rest.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `line` is a `<prefix> Name <email> <timestamp> <tz>` identity header:
/// it must carry an email in angle brackets followed by a numeric timestamp.
fn is_ident_header(line: &str, prefix: &str) -> bool {
    let Some(rest) = line
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return false;
    };
    let (Some(open), Some(close)) = (rest.find('<'), rest.rfind('>')) else {
        return false;
    };
    if open >= close {
        return false;
    }
    rest[close + 1..]
        .split_whitespace()
        .next()
        .is_some_and(|ts| !ts.is_empty() && ts.bytes().all(|b| b.is_ascii_digit()))
}

/// Validate a commit's header lines in order: `tree`, zero or more `parent`,
/// `author`, `committer`. Git does not require a blank line before the message,
/// so trailing content is unrestricted.
fn validate_commit_format(data: &[u8]) -> CliResult<()> {
    let text = std::str::from_utf8(data)
        .map_err(|_| corrupt_object_error(ObjectType::Commit, "not valid UTF-8"))?;
    let mut lines = text.lines();

    if !lines.next().is_some_and(|line| is_header_oid(line, "tree")) {
        return Err(corrupt_object_error(
            ObjectType::Commit,
            "missing or malformed 'tree' header",
        ));
    }

    let mut line = lines.next();
    while line.is_some_and(|l| is_header_oid(l, "parent")) {
        line = lines.next();
    }

    if !line.is_some_and(|l| is_ident_header(l, "author")) {
        return Err(corrupt_object_error(
            ObjectType::Commit,
            "missing or malformed 'author' line",
        ));
    }
    if !lines
        .next()
        .is_some_and(|l| is_ident_header(l, "committer"))
    {
        return Err(corrupt_object_error(
            ObjectType::Commit,
            "missing or malformed 'committer' line",
        ));
    }
    Ok(())
}

/// Validate a tag's header lines: `object`, `type`, `tag`, `tagger`.
fn validate_tag_format(data: &[u8]) -> CliResult<()> {
    let text = std::str::from_utf8(data)
        .map_err(|_| corrupt_object_error(ObjectType::Tag, "not valid UTF-8"))?;
    let mut lines = text.lines();

    if !lines
        .next()
        .is_some_and(|line| is_header_oid(line, "object"))
    {
        return Err(corrupt_object_error(
            ObjectType::Tag,
            "missing or malformed 'object' header",
        ));
    }
    match lines.next() {
        Some(line) if header_has_value(line, "type") => {}
        _ => {
            return Err(corrupt_object_error(
                ObjectType::Tag,
                "missing 'type' header",
            ));
        }
    }
    match lines.next() {
        Some(line) if header_has_value(line, "tag") => {}
        _ => {
            return Err(corrupt_object_error(
                ObjectType::Tag,
                "missing 'tag' header",
            ));
        }
    }
    if !lines.next().is_some_and(|l| is_ident_header(l, "tagger")) {
        return Err(corrupt_object_error(
            ObjectType::Tag,
            "missing or malformed 'tagger' line",
        ));
    }
    Ok(())
}

/// Validate a tree's binary body: a sequence of `<octal-mode> <name>\0<oid>`
/// entries, where the oid is the repository's hash width in raw bytes.
fn validate_tree_format(data: &[u8]) -> CliResult<()> {
    let hash_len = get_hash_kind().size();
    let mut index = 0;
    while index < data.len() {
        let space = data[index..]
            .iter()
            .position(|&byte| byte == b' ')
            .ok_or_else(|| {
                corrupt_object_error(ObjectType::Tree, "entry missing mode separator")
            })?;
        let mode = &data[index..index + space];
        if mode.is_empty() || !mode.iter().all(|byte| byte.is_ascii_digit()) {
            return Err(corrupt_object_error(
                ObjectType::Tree,
                "entry has an invalid mode",
            ));
        }
        index += space + 1;

        let nul = data[index..]
            .iter()
            .position(|&byte| byte == 0)
            .ok_or_else(|| {
                corrupt_object_error(ObjectType::Tree, "entry missing name terminator")
            })?;
        if nul == 0 {
            return Err(corrupt_object_error(
                ObjectType::Tree,
                "entry has an empty name",
            ));
        }
        index += nul + 1;

        if index + hash_len > data.len() {
            return Err(corrupt_object_error(
                ObjectType::Tree,
                "entry has a truncated object id",
            ));
        }
        index += hash_len;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ident_header_requires_email_and_timestamp() {
        assert!(is_ident_header(
            "author A <a@b.c> 1700000000 +0000",
            "author"
        ));
        assert!(!is_ident_header("author A no-email 1700000000", "author"));
        assert!(!is_ident_header("author A <a@b.c>", "author"));
        assert!(!is_ident_header("committer A <a@b.c> 1 +0000", "author"));
    }

    #[test]
    fn hash_one_source_matches_git_empty_blob_hash() {
        let entry = hash_one_source("-", Vec::new(), false, ObjectType::Blob, false)
            .expect("hash empty source");
        assert_eq!(entry.oid, "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(entry.size, 0);
        assert!(!entry.written);
    }

    #[test]
    fn write_hash_line_ignores_broken_pipe() {
        struct BrokenPipeWriter;

        impl Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut writer = BrokenPipeWriter;
        write_hash_line(&mut writer, "abc").expect("broken pipe should be ignored");
    }
}
