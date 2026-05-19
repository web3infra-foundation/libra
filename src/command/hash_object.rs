//! Implements `hash-object` for computing Git-compatible blob object IDs.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use clap::Parser;
use git_internal::internal::object::blob::Blob;
use serde::Serialize;

use crate::{
    command::save_object,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

const HASH_OBJECT_EXAMPLES: &str = "\
Examples:
  libra hash-object README.md
  libra hash-object -w src/main.rs
  printf 'hello' | libra hash-object --stdin
  printf 'hello' | libra hash-object --stdin --json
";

#[derive(Parser, Debug)]
#[command(after_help = HASH_OBJECT_EXAMPLES)]
pub struct HashObjectArgs {
    /// Actually write the object into the object database
    #[arg(short = 'w', long)]
    pub write: bool,

    /// Read the object contents from standard input
    #[arg(long, conflicts_with = "paths")]
    pub stdin: bool,

    /// Object type to hash. Only `blob` is currently supported.
    #[arg(
        short = 't',
        long = "type",
        default_value = "blob",
        value_name = "TYPE"
    )]
    pub object_type: String,

    /// File paths to hash
    #[arg(value_name = "PATH", required_unless_present = "stdin")]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct HashObjectOutput {
    object_type: &'static str,
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
    if args.object_type != "blob" {
        return Err(
            CliError::fatal(format!("unsupported object type '{}'", args.object_type))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
                .with_hint("libra hash-object currently supports only blob objects."),
        );
    }

    if output.is_json() {
        let result = hash_objects(&args)?;
        return render_hash_object_output(&result, output);
    }

    hash_objects_streaming(&args, output)
}

fn hash_objects(args: &HashObjectArgs) -> CliResult<HashObjectOutput> {
    let objects = if args.stdin {
        vec![hash_one_source("-", read_stdin()?, args.write)?]
    } else {
        let mut entries = Vec::with_capacity(args.paths.len());
        for path in &args.paths {
            entries.push(hash_one_source(
                path.display().to_string(),
                read_file(path)?,
                args.write,
            )?);
        }
        entries
    };

    Ok(HashObjectOutput {
        object_type: "blob",
        write: args.write,
        objects,
    })
}

fn hash_objects_streaming(args: &HashObjectArgs, output: &OutputConfig) -> CliResult<()> {
    if output.quiet {
        return hash_objects(args).map(|_| ());
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();

    if args.stdin {
        let entry = hash_one_source("-", read_stdin()?, args.write)?;
        write_hash_line(&mut writer, &entry.oid)?;
        return Ok(());
    }

    for path in &args.paths {
        let entry = hash_one_source(path.display().to_string(), read_file(path)?, args.write)?;
        write_hash_line(&mut writer, &entry.oid)?;
    }

    Ok(())
}

fn hash_one_source(
    source: impl Into<String>,
    data: Vec<u8>,
    write: bool,
) -> CliResult<HashObjectEntry> {
    let size = data.len();
    let blob = Blob::from_content_bytes(data);
    let oid = blob.id.to_string();

    if write {
        save_object(&blob, &blob.id).map_err(|error| {
            CliError::fatal(format!("failed to write blob object {oid}: {error}"))
                .with_stable_code(StableErrorCode::IoWriteFailed)
                .with_hint("check repository object storage permissions and available disk space.")
        })?;
    }

    Ok(HashObjectEntry {
        source: source.into(),
        oid,
        size,
        written: write,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_one_source_matches_git_empty_blob_hash() {
        let entry = hash_one_source("-", Vec::new(), false).expect("hash empty source");
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
