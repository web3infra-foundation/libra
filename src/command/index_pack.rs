use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
};

use clap::Parser;
pub use index_pack_v1::build_index_v1;
pub use index_pack_v2::build_index_v2;
use serde::Serialize;

use crate::{
    command::{
        index_pack_support::{format_io_error, index_pack_error, keep_file_path, write_keep_file},
        index_pack_v1, index_pack_v2,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

const INDEX_PACK_EXAMPLES: &str = "\
EXAMPLES:
    libra index-pack pack-123.pack                  Build pack-123.idx alongside the .pack
    libra index-pack --keep pack-123.pack           Build an idx and empty pack-123.keep
    libra index-pack --keep=message pack-123.pack   Build an idx and write message to .keep
    libra index-pack --stdin -o pack-123.idx        Read pack bytes from stdin
    libra index-pack --progress pack-123.pack       Accept Git-style progress request
    libra index-pack --no-progress pack-123.pack    Accept Git-style progress suppression
    libra index-pack pack-123.pack -o pack-123.idx  Write the index to a specific path
    libra index-pack pack-123.pack --json           Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = INDEX_PACK_EXAMPLES)]
pub struct IndexPackArgs {
    #[arg(help = "Pack file path", required_unless_present = "stdin")]
    pub pack_file: Option<String>,

    #[arg(
        short = 'o',
        help = "Output index file path. Defaults to replacing .pack with .idx"
    )]
    pub index_file: Option<String>,

    #[arg(
        long,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "",
        value_name = "MSG",
        help = "Write a .keep file beside the pack, optionally containing MSG"
    )]
    pub keep: Option<String>,

    #[arg(long, help = "Force pack index version 1 or 2")]
    pub index_version: Option<u8>,

    #[arg(long, help = "Read pack data from standard input; requires -o")]
    pub stdin: bool,

    #[arg(
        long = "fix-thin",
        help = "Accept Git's --fix-thin (thin-pack completion) flag as a no-op: Libra requires \
                self-contained packs and does not resolve external delta bases, and never \
                produces thin packs, so on the packs it indexes there is nothing to complete"
    )]
    pub fix_thin: bool,
}

#[derive(Debug, Clone, Serialize)]
struct IndexPackOutput {
    pack_file: String,
    index_file: String,
    index_version: u8,
    keep_file: Option<String>,
}

pub fn execute(args: IndexPackArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()) {
        err.print_stderr();
    }
}

pub fn execute_safe(args: IndexPackArgs, output: &OutputConfig) -> CliResult<()> {
    let IndexPackArgs {
        pack_file,
        index_file,
        keep,
        index_version,
        stdin,
        fix_thin,
    } = args;

    // `--fix-thin` is accepted for Git CLI compatibility but is a no-op. A thin
    // pack carries `REF_DELTA` objects whose base objects live outside the pack;
    // completing it means resolving those bases from the repository and appending
    // them. Libra's pack decoder (git-internal) has no external-base resolver — it
    // requires self-contained packs — and Libra never produces thin packs, so any
    // pack that indexes successfully already had no external delta bases to add.
    // This matches Git, where `--fix-thin` on a complete pack does nothing.
    let _ = fix_thin;

    let index_file = match index_file {
        Some(index_file) => index_file,
        None if stdin => {
            return Err(
                CliError::fatal("index-pack --stdin requires -o <INDEX_FILE>")
                    .with_stable_code(StableErrorCode::CliInvalidArguments),
            );
        }
        None => {
            let Some(pack_file) = pack_file.as_deref() else {
                return Err(
                    CliError::fatal("pack-file is required unless --stdin is used")
                        .with_stable_code(StableErrorCode::CliInvalidArguments),
                );
            };
            if !pack_file.ends_with(".pack") {
                return Err(CliError::fatal("pack-file does not end with '.pack'")
                    .with_stable_code(StableErrorCode::CliInvalidArguments));
            }
            pack_file.replace(".pack", ".idx")
        }
    };

    let pack_file = if stdin {
        if pack_file.is_some() {
            return Err(
                CliError::fatal("index-pack --stdin cannot be combined with <PACK_FILE>")
                    .with_stable_code(StableErrorCode::CliInvalidArguments),
            );
        }
        derive_stdin_pack_file(&index_file)
    } else {
        pack_file.ok_or_else(|| {
            CliError::fatal("pack-file is required unless --stdin is used")
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        })
    }?;

    if index_file == pack_file {
        return Err(
            CliError::fatal("pack-file and index-file are the same file")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }

    let keep_path = keep.as_ref().map(|_| keep_file_path(&pack_file));
    if keep_path.as_ref() == Some(&std::path::PathBuf::from(&index_file)) {
        return Err(
            CliError::fatal("keep-file and index-file are the same file")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }
    let keep_file = keep_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());

    if stdin {
        read_stdin_to_pack_file(&pack_file)?;
    }

    std::fs::File::open(&pack_file).map_err(|e| {
        CliError::fatal(format!(
            "could not open '{}' for reading: {}",
            pack_file,
            format_io_error(&e)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;

    let index_version = match index_version {
        Some(1) => {
            build_index_v1(&pack_file, &index_file).map_err(index_pack_error)?;
            1
        }
        Some(2) => {
            build_index_v2(&pack_file, &index_file).map_err(index_pack_error)?;
            2
        }
        Some(_) => {
            return Err(CliError::fatal("unsupported index version")
                .with_stable_code(StableErrorCode::CliInvalidArguments));
        }
        None => {
            build_index_v1(&pack_file, &index_file).map_err(index_pack_error)?;
            1
        }
    };

    if let (Some(keep_file), Some(message)) = (keep_file.as_deref(), keep.as_deref()) {
        write_keep_file(keep_file, message)?;
    }

    let result = IndexPackOutput {
        pack_file,
        index_file,
        index_version,
        keep_file,
    };

    if output.is_json() {
        emit_json_data("index-pack", &result, output)?;
    } else if !output.quiet {
        println!("{}", result.index_file);
    }

    Ok(())
}

fn derive_stdin_pack_file(index_file: &str) -> CliResult<String> {
    let pack_file = PathBuf::from(index_file).with_extension("pack");
    if pack_file.as_os_str().is_empty() {
        return Err(
            CliError::fatal("index-pack --stdin requires a valid -o <INDEX_FILE> path")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }
    Ok(pack_file.to_string_lossy().into_owned())
}

fn read_stdin_to_pack_file(pack_file: &str) -> CliResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(pack_file)
        .map_err(|e| {
            CliError::fatal(format!(
                "could not create derived pack file '{}' for writing: {}",
                pack_file,
                format_io_error(&e)
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;

    io::copy(&mut io::stdin().lock(), &mut file).map_err(|e| {
        CliError::fatal(format!(
            "could not read pack data from stdin into '{}': {}",
            pack_file,
            format_io_error(&e)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    file.flush().map_err(|e| {
        CliError::fatal(format!(
            "could not flush derived pack file '{}': {}",
            pack_file,
            format_io_error(&e)
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)
    })
}
