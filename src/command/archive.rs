//! Command-line surface for creating archives from committed tree snapshots.

use std::path::{Component, Path, PathBuf};

use clap::Parser;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::OutputConfig,
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
/// None yet. This skeleton only reserves the CLI surface for later archive
/// implementation commits.
///
/// # Errors
///
/// Returns `CliInvalidArguments` for unsupported formats or unsafe prefixes.
/// Returns `Unsupported` until archive creation is implemented.
pub async fn execute_safe(args: ArchiveArgs, _output: &OutputConfig) -> CliResult<()> {
    let _format = ArchiveFormat::parse_strict(&args.format).map_err(|message| {
        CliError::command_usage(message).with_stable_code(StableErrorCode::CliInvalidArguments)
    })?;
    let _prefix = validate_prefix(args.prefix.as_deref())?;

    Err(CliError::failure(
        "archive command is registered but archive creation is not implemented yet",
    )
    .with_stable_code(StableErrorCode::Unsupported))
}

#[cfg(test)]
mod tests {
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
}
