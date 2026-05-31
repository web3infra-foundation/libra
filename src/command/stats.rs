//! Stats command for counting files in the working directory by extension.
//!
//! This module implements a `stats` command that scans the current working
//! directory and counts files grouped by file extension. Files without an
//! extension are grouped under `no_extension`.  The `.libra/` and `target/`
//! directories (plus all hidden directories starting with `.`) are skipped.
//!
//! - **Argument parsing** is handled by [`StatsArgs`], which accepts an
//!   optional `--json` flag.
//! - **Execution entrypoints**:
//!   - [`execute`] is the user-facing entrypoint used by the CLI dispatcher.
//!   - [`execute_safe`] is the safe entrypoint honouring global JSON/quiet
//!     mode via [`OutputConfig`].
//! - **Collection** is performed by [`collect_stats`], which recursively
//!   walks the working directory while skipping configured directories.
//! - **Rendering** is split between [`render_stats`] (human-readable text)
//!   and JSON emission via [`emit_json_data`].

use std::{collections::BTreeMap, fs, io::Write, path::Path};

use clap::Parser;
use serde::Serialize;

use crate::utils::{
    error::{CliError, CliResult},
    output::{OutputConfig, emit_json_data},
};

/// Directories to skip during file-system scanning.
const SKIP_DIRS: &[&str] = &[".libra", "target"];

#[derive(Parser, Debug)]
pub struct StatsArgs {}

#[derive(Debug, Clone, Serialize)]
struct StatsOutput {
    /// Total number of files scanned.
    total_files: usize,
    /// Extension → count mapping, sorted alphabetically.
    extensions: BTreeMap<String, usize>,
}

/// User-facing entrypoint; prints errors to stderr and exits.
pub fn execute(args: StatsArgs) {
    if let Err(e) = execute_safe(&args, &OutputConfig::default()) {
        e.print_stderr();
    }
}

/// Safe entrypoint that returns a structured [`CliResult`].
///
/// # Side Effects
/// - Reads the filesystem: walks the current working directory recursively.
///
/// # Errors
/// - `CliError::fatal` when the current directory cannot be determined or
///   when a directory entry cannot be read.
pub fn execute_safe(_args: &StatsArgs, output: &OutputConfig) -> CliResult<()> {
    let stats = collect_stats()?;

    if output.is_json() {
        emit_json_data("stats", &stats, output)?;
    } else if !output.quiet {
        let mut stdout = std::io::stdout();
        render_stats(&stats, &mut stdout)?;
    }

    Ok(())
}

/// Walk the current working directory and build extension-count statistics.
fn collect_stats() -> CliResult<StatsOutput> {
    let current_dir = std::env::current_dir()
        .map_err(|e| CliError::fatal(format!("failed to get current directory: {e}")))?;

    let mut extensions: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_files = 0usize;

    scan_directory(&current_dir, &mut extensions, &mut total_files)
        .map_err(|e| CliError::fatal(format!("failed to scan directory: {e}")))?;

    Ok(StatsOutput {
        total_files,
        extensions,
    })
}

/// Recursively scan `dir` for regular files, grouping by extension.
///
/// Hidden directories (names starting with `.`) and directories in
/// [`SKIP_DIRS`] are skipped entirely.
fn scan_directory(
    dir: &Path,
    extensions: &mut BTreeMap<String, usize>,
    total_files: &mut usize,
) -> std::io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            // Tolerate permission-denied on individual directories so the
            // command still reports whatever it can read.
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return Ok(());
            }
            return Err(e);
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if path.is_dir() {
            // Skip hidden directories and the explicit skip list.
            if file_name.starts_with('.') || SKIP_DIRS.contains(&file_name) {
                continue;
            }
            scan_directory(&path, extensions, total_files)?;
        } else if path.is_file() {
            *total_files += 1;
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_else(|| "no_extension".to_string());
            *extensions.entry(ext).or_insert(0) += 1;
        }
    }

    Ok(())
}

/// Render the statistics as human-readable text to `writer`.
fn render_stats(stats: &StatsOutput, writer: &mut impl Write) -> CliResult<()> {
    writeln!(writer, "Total files: {}", stats.total_files)
        .map_err(|e| CliError::fatal(format!("stats output error: {e}")))?;

    writeln!(writer).map_err(|e| CliError::fatal(format!("stats output error: {e}")))?;

    for (ext, count) in &stats.extensions {
        writeln!(writer, "  {:>6}  {}", count, ext)
            .map_err(|e| CliError::fatal(format!("stats output error: {e}")))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_parse_args() {
        let args = StatsArgs::parse_from(["stats"]);
        // StatsArgs currently has no fields; this just ensures clap parsing
        // does not reject the bare subcommand.
        let _ = args;
    }

    #[test]
    fn test_collect_stats_basic() {
        let temp = tempdir().unwrap();
        // Create a few files with known extensions.
        fs::write(temp.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(temp.path().join("lib.rs"), "pub fn hello() {}").unwrap();
        fs::write(temp.path().join("README"), "docs").unwrap(); // no extension
        fs::write(temp.path().join("Cargo.toml"), "[package]").unwrap();
        // Create a dir we should skip.
        fs::create_dir_all(temp.path().join(".libra")).unwrap();
        fs::write(temp.path().join(".libra").join("ignored.txt"), "hidden").unwrap();
        // Also create a target dir we should skip.
        fs::create_dir_all(temp.path().join("target")).unwrap();
        fs::write(temp.path().join("target").join("build.bin"), "binary").unwrap();

        // Collect stats from the temp dir.
        let stats = collect_stats_in(temp.path()).unwrap();

        assert_eq!(stats.total_files, 4);
        assert_eq!(stats.extensions.get("rs").copied().unwrap_or(0), 2);
        assert_eq!(stats.extensions.get("toml").copied().unwrap_or(0), 1);
        assert_eq!(
            stats.extensions.get("no_extension").copied().unwrap_or(0),
            1
        );
        // Hidden and skipped dirs must not appear.
        assert!(!stats.extensions.contains_key("txt"));
        assert!(!stats.extensions.contains_key("bin"));
    }

    /// Like [`collect_stats`] but operates on an explicit directory for testing.
    fn collect_stats_in(root: &Path) -> CliResult<StatsOutput> {
        let mut extensions: BTreeMap<String, usize> = BTreeMap::new();
        let mut total_files = 0usize;
        scan_directory(root, &mut extensions, &mut total_files)
            .map_err(|e| CliError::fatal(format!("failed to scan directory: {e}")))?;
        Ok(StatsOutput {
            total_files,
            extensions,
        })
    }

    #[test]
    fn test_extension_case_insensitive_grouping() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("a.RS"), "a").unwrap();
        fs::write(temp.path().join("b.rs"), "b").unwrap();
        fs::write(temp.path().join("c.Rs"), "c").unwrap();

        let stats = collect_stats_in(temp.path()).unwrap();
        assert_eq!(stats.total_files, 3);
        // All three should be grouped under "rs" (lowercased).
        assert_eq!(stats.extensions.get("rs").copied().unwrap_or(0), 3);
    }

    #[test]
    fn test_empty_directory() {
        let temp = tempdir().unwrap();
        let stats = collect_stats_in(temp.path()).unwrap();
        assert_eq!(stats.total_files, 0);
        assert!(stats.extensions.is_empty());
    }

    #[test]
    fn test_skips_hidden_directories() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".hidden_dir")).unwrap();
        fs::write(temp.path().join(".hidden_dir").join("secret.txt"), "secret").unwrap();
        fs::write(temp.path().join("visible.txt"), "visible").unwrap();

        let stats = collect_stats_in(temp.path()).unwrap();
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.extensions.get("txt").copied().unwrap_or(0), 1);
    }

    #[test]
    fn test_nested_directories() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src").join("utils")).unwrap();
        fs::write(temp.path().join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(
            temp.path().join("src").join("utils").join("helper.rs"),
            "pub fn help() {}",
        )
        .unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[package]").unwrap();

        let stats = collect_stats_in(temp.path()).unwrap();
        assert_eq!(stats.total_files, 3);
        assert_eq!(stats.extensions.get("rs").copied().unwrap_or(0), 2);
        assert_eq!(stats.extensions.get("toml").copied().unwrap_or(0), 1);
    }
}
