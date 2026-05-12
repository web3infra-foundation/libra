//! `libra publish` — read-only Cloudflare publishing.
//!
//! Per `docs/improvement/publish.md`, the publish CLI surface is
//! `init` / `sync` / `status` / `deploy` / `unpublish`. `init` now
//! materialises the embedded Worker template; the remaining subcommands
//! still surface a clear "not yet implemented" message until their
//! sync/deploy plumbing ships.
//!
//! Each subcommand returns a `CliInvalidArguments`-style error
//! pointing the user at:
//!
//!   * the relevant `libra cloud sync` baseline that is implemented
//!     (Phase 1's `run_cloud_sync` helper),
//!   * the publish.md design doc,
//!   * the planned Phase 4 release.
//!
//! Codex pass-7 P1 registered the CLI surface so the `clap` parser
//! would not reject `libra publish ...`. `init` is the first concrete
//! slice: it writes only source-template files and a local manifest,
//! never generated Worker output or credentials.

use std::{
    collections::BTreeMap,
    fs, io,
    io::Write,
    path::{Component, Path, PathBuf},
};

use clap::{Parser, Subcommand};
use ring::digest::{SHA256, digest};
use serde::Serialize;

use crate::{
    internal::publish::worker_template::{
        MANIFEST, RenderPolicy, WorkerTemplate, embed_path_is_allowed,
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{self, CommandOutput, OutputConfig},
        util,
    },
};

#[derive(Parser, Debug)]
#[command(about = "Materialise the read-only Cloudflare Worker template")]
pub struct PublishArgs {
    #[command(subcommand)]
    pub command: PublishCommand,
}

#[derive(Subcommand, Debug)]
pub enum PublishCommand {
    /// Materialise the local Worker template scaffold.
    Init(InitArgs),
    /// Reserved for the planned D1/R2 sync implementation.
    Sync(SyncArgs),
    /// Reserved for the planned local/cloud status report.
    Status(StatusArgs),
    /// Reserved for the planned Cloudflare Worker deploy flow.
    Deploy(DeployArgs),
    /// Reserved for the planned unpublish flow.
    Unpublish(UnpublishArgs),
}

#[derive(Parser, Debug)]
pub struct InitArgs {
    /// URL-safe slug; uniqueness scoped to `--clone-domain`.
    #[arg(long)]
    pub slug: Option<String>,

    /// Public clone domain, e.g. `code.example.com`.
    #[arg(long)]
    pub clone_domain: Option<String>,

    /// Browser-facing origin URL, e.g. `https://code.example.com`.
    /// Codex pass-8 P2: documented in publish.md / docs/commands.
    #[arg(long)]
    pub display_origin: Option<String>,

    /// Display name shown in the Worker UI header.
    /// Codex pass-8 P2: documented in publish.md / docs/commands.
    #[arg(long)]
    pub name: Option<String>,

    /// `public` (browser-readable) or `private` (Cloudflare Access).
    #[arg(long)]
    pub visibility: Option<String>,

    /// Worker name; defaults to `libra-publish`.
    #[arg(long)]
    pub worker_name: Option<String>,

    /// Per-file preview cap in bytes. Files larger than this fall
    /// back to metadata-only. Must be positive — pass `0` is rejected
    /// because a zero cap defeats the purpose of code-preview
    /// publishing. Codex pass-8 P2 + pass-9 P2: documented flag with
    /// clap-side `> 0` validation.
    #[arg(long, value_parser = parse_max_preview_bytes)]
    pub max_preview_bytes: Option<u64>,
}

#[derive(Parser, Debug)]
pub struct SyncArgs {
    /// Sync only the named ref (`refs/heads/main` or `main`).
    #[arg(long)]
    pub r#ref: Option<String>,

    /// Print the plan without writing to D1/R2.
    #[arg(long)]
    pub dry_run: bool,

    /// Fail on dirty working tree instead of warning.
    #[arg(long)]
    pub fail_on_dirty: bool,

    /// Redaction policy: `default` or `strict`.
    #[arg(long, default_value = "default")]
    pub ai_redaction: String,

    /// Allow a path that the deny list would normally block. Only
    /// honored on `private` sites. Codex pass-8 P2: documented in
    /// publish.md `.librapublishignore` section.
    #[arg(long, value_name = "path")]
    pub allow_sensitive_path: Vec<String>,

    /// Force re-upload of every file/object even if `is_synced`
    /// is set. Codex pass-8 P2: documented in publish.md hardening
    /// criteria for the CAS latest-revision conflict path.
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser, Debug)]
pub struct StatusArgs {}

#[derive(Parser, Debug)]
pub struct DeployArgs {
    /// Skip the Wrangler deploy step (useful for CI smoke tests).
    #[arg(long)]
    pub skip_deploy: bool,
}

#[derive(Parser, Debug)]
pub struct UnpublishArgs {
    /// Confirm the unpublish operation.
    #[arg(long)]
    pub yes: bool,
}

const NOT_YET_IMPLEMENTED: &str = "`libra publish` Phase 4 sync/deploy plumbing is not ready yet. \
     `libra publish init` can materialise the Worker template; track \
     docs/improvement/publish.md for the remaining v1 release window.";
const WORKER_TEMPLATE_MANIFEST_SCHEMA_VERSION: u32 = 1;
const WORKER_TEMPLATE_MANIFEST_PATH: &str = ".libra/publish/worker-template-manifest.json";

/// clap value parser for `--max-preview-bytes`.
///
/// Codex pass-9 P2: enforce `> 0` at the parse layer so a zero value
/// is caught before the stub runs. The SQL schema currently allows
/// `>= 0`, but at the CLI level a zero cap publishes no file
/// previews — that is unambiguously a misuse.
fn parse_max_preview_bytes(raw: &str) -> Result<u64, String> {
    let parsed: u64 = raw
        .parse()
        .map_err(|_| format!("'{raw}' is not a valid byte count"))?;
    if parsed == 0 {
        // Codex pass-10 P3: include the offending input verbatim so
        // the error message reads naturally in scripts that pipe
        // user input through.
        return Err(format!(
            "'{raw}' is not a valid byte count: must be > 0; pass a positive byte count or \
             omit the flag",
        ));
    }
    Ok(parsed)
}

fn unsupported_publish_subcommand(subcommand: &'static str) -> CliResult<()> {
    // Codex pass-8 P2: tag the typed error with `Unsupported` so the
    // stable-code surface is `LBR-UNSUPPORTED-001`, not the generic
    // internal-invariant fallback. Downstream tooling that classifies
    // errors by stable code (CI matrix, telemetry) can match on
    // "feature not yet implemented" rather than treating this as a
    // crash bug.
    Err(CliError::fatal(NOT_YET_IMPLEMENTED)
        .with_stable_code(StableErrorCode::Unsupported)
        .with_detail("operation", "publish")
        .with_detail("component", "publish")
        .with_detail("subcommand", subcommand)
        .with_detail("phase", "4"))
}

pub async fn execute(args: PublishArgs) -> CliResult<()> {
    execute_safe(args, &OutputConfig::default()).await
}

pub async fn execute_safe(args: PublishArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    match args.command {
        PublishCommand::Init(init_args) => {
            let repo_root = util::try_working_dir().map_err(|source| {
                CliError::fatal(format!("failed to resolve Libra repository root: {source}"))
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            })?;
            let result = run_publish_init_at_root(&repo_root, &init_args)?;
            output::emit(&result, output)
        }
        PublishCommand::Sync(_) => unsupported_publish_subcommand("sync"),
        PublishCommand::Status(_) => unsupported_publish_subcommand("status"),
        PublishCommand::Deploy(_) => unsupported_publish_subcommand("deploy"),
        PublishCommand::Unpublish(_) => unsupported_publish_subcommand("unpublish"),
    }
}

#[derive(Debug)]
struct TemplateFile {
    path: String,
    bytes: Vec<u8>,
    sha256: String,
    render_policy: RenderPolicy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerTemplateManifest {
    schema_version: u32,
    template_version: &'static str,
    worker_dir: &'static str,
    files: Vec<WorkerTemplateManifestFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerTemplateManifestFile {
    path: String,
    render_policy: &'static str,
    sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishInitOutput {
    worker_dir: String,
    manifest_path: String,
    template_version: &'static str,
    files_written: usize,
    files_current: usize,
}

impl CommandOutput for PublishInitOutput {
    fn render_human(&self, writer: &mut dyn Write, output: &OutputConfig) -> io::Result<()> {
        if output.quiet {
            return Ok(());
        }
        writeln!(writer, "Initialized publish Worker template")?;
        writeln!(writer, "  worker: {}", self.worker_dir)?;
        writeln!(writer, "  manifest: {}", self.manifest_path)?;
        writeln!(writer, "  files written: {}", self.files_written)?;
        writeln!(writer, "  files current: {}", self.files_current)?;
        Ok(())
    }
}

fn run_publish_init_at_root(repo_root: &Path, _args: &InitArgs) -> CliResult<PublishInitOutput> {
    let files = collect_worker_template_files()?;
    let worker_dir = repo_root.join("worker");
    let manifest_path = repo_root.join(WORKER_TEMPLATE_MANIFEST_PATH);

    let conflicts = find_worker_template_conflicts(&worker_dir, &files)?;
    if !conflicts.is_empty() {
        return Err(conflicting_worker_template_error(&conflicts));
    }

    let mut files_written = 0usize;
    let mut files_current = 0usize;
    for file in &files {
        let destination = worker_dir.join(&file.path);
        if destination.exists() {
            files_current += 1;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                CliError::fatal(format!(
                    "failed to create Worker template directory '{}': {source}",
                    parent.display()
                ))
                .with_stable_code(StableErrorCode::IoWriteFailed)
            })?;
        }
        fs::write(&destination, &file.bytes).map_err(|source| {
            CliError::fatal(format!(
                "failed to write Worker template file '{}': {source}",
                destination.display()
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
        files_written += 1;
    }

    let manifest = WorkerTemplateManifest {
        schema_version: WORKER_TEMPLATE_MANIFEST_SCHEMA_VERSION,
        template_version: env!("CARGO_PKG_VERSION"),
        worker_dir: "worker",
        files: files
            .iter()
            .map(|file| WorkerTemplateManifestFile {
                path: file.path.clone(),
                render_policy: render_policy_name(file.render_policy),
                sha256: file.sha256.clone(),
            })
            .collect(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        CliError::internal(format!(
            "failed to encode Worker template manifest: {source}"
        ))
    })?;
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            CliError::fatal(format!(
                "failed to create publish manifest directory '{}': {source}",
                parent.display()
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    }
    fs::write(&manifest_path, manifest_bytes).map_err(|source| {
        CliError::fatal(format!(
            "failed to write Worker template manifest '{}': {source}",
            manifest_path.display()
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)
    })?;

    Ok(PublishInitOutput {
        worker_dir: "worker".to_string(),
        manifest_path: WORKER_TEMPLATE_MANIFEST_PATH.to_string(),
        template_version: env!("CARGO_PKG_VERSION"),
        files_written,
        files_current,
    })
}

fn collect_worker_template_files() -> CliResult<Vec<TemplateFile>> {
    let policy_by_path: BTreeMap<&'static str, RenderPolicy> = MANIFEST
        .iter()
        .map(|entry| (entry.path, entry.render_policy))
        .collect();
    let mut paths: Vec<String> = WorkerTemplate::iter()
        .map(|path| path.to_string())
        .collect();
    paths.sort();

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        validate_template_relative_path(&path)?;
        if !embed_path_is_allowed(&path) {
            return Err(CliError::internal(format!(
                "embedded Worker template path '{path}' is denied by publish safety rules"
            )));
        }
        let embedded = WorkerTemplate::get(&path).ok_or_else(|| {
            CliError::internal(format!(
                "embedded Worker template path '{path}' was listed but could not be read"
            ))
        })?;
        let bytes = embedded.data.into_owned();
        let sha256 = hex::encode(digest(&SHA256, &bytes).as_ref());
        let render_policy = policy_by_path
            .get(path.as_str())
            .copied()
            .unwrap_or(RenderPolicy::ManagedTemplate);
        files.push(TemplateFile {
            path,
            bytes,
            sha256,
            render_policy,
        });
    }

    Ok(files)
}

fn validate_template_relative_path(path: &str) -> CliResult<()> {
    let relative = Path::new(path);
    if relative.is_absolute() {
        return Err(CliError::internal(format!(
            "embedded Worker template path '{path}' must be relative"
        )));
    }
    for component in relative.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(CliError::internal(format!(
                    "embedded Worker template path '{path}' contains an invalid component"
                )));
            }
        }
    }
    Ok(())
}

fn find_worker_template_conflicts(
    worker_dir: &Path,
    files: &[TemplateFile],
) -> CliResult<Vec<String>> {
    let mut conflicts = Vec::new();
    for file in files {
        if let Some(symlink_path) = first_existing_symlink_path(worker_dir, &file.path)? {
            conflicts.push(symlink_path);
            continue;
        }

        let destination = worker_dir.join(&file.path);
        if !destination.exists() {
            continue;
        }
        let metadata = fs::metadata(&destination).map_err(|source| {
            CliError::io(format!(
                "failed to inspect Worker template file '{}': {source}",
                destination.display()
            ))
        })?;
        if !metadata.is_file() {
            conflicts.push(file.path.clone());
            continue;
        }
        let existing = fs::read(&destination).map_err(|source| {
            CliError::io(format!(
                "failed to read existing Worker template file '{}': {source}",
                destination.display()
            ))
        })?;
        if existing != file.bytes {
            conflicts.push(file.path.clone());
        }
    }
    conflicts.sort();
    conflicts.dedup();
    Ok(conflicts)
}

fn first_existing_symlink_path(
    worker_dir: &Path,
    relative_path: &str,
) -> CliResult<Option<String>> {
    if let Ok(metadata) = fs::symlink_metadata(worker_dir)
        && metadata.file_type().is_symlink()
    {
        return Ok(Some("worker".to_string()));
    }

    let mut current = PathBuf::from(worker_dir);
    let mut relative = PathBuf::new();
    for component in Path::new(relative_path).components() {
        let Component::Normal(segment) = component else {
            continue;
        };
        current.push(segment);
        relative.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Ok(Some(format!("worker/{}", relative.display())));
            }
            Ok(_) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CliError::io(format!(
                    "failed to inspect Worker template path '{}': {source}",
                    current.display()
                )));
            }
        }
    }
    Ok(None)
}

fn conflicting_worker_template_error(conflicts: &[String]) -> CliError {
    let display = conflicts
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if conflicts.len() > 5 {
        format!(" and {} more", conflicts.len() - 5)
    } else {
        String::new()
    };
    CliError::conflict(format!(
        "Worker template files would be overwritten: {display}{suffix}"
    ))
    .with_detail("operation", "publish init")
    .with_detail("conflicts", serde_json::json!(conflicts))
    .with_hint("merge or move the listed worker files, then rerun 'libra publish init'.")
}

fn render_policy_name(policy: RenderPolicy) -> &'static str {
    match policy {
        RenderPolicy::ManagedTemplate => "managed_template",
        RenderPolicy::RenderedConfig => "rendered_config",
        RenderPolicy::UserOwned => "user_owned",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;

    use super::*;

    fn default_init_args() -> InitArgs {
        InitArgs {
            slug: Some("demo".to_string()),
            clone_domain: Some("code.example.com".to_string()),
            display_origin: None,
            name: None,
            visibility: None,
            worker_name: None,
            max_preview_bytes: None,
        }
    }

    /// Codex pass-10 P1: pin the `--max-preview-bytes` parser
    /// behaviour. The CLI surface must reject 0 (zero cap publishes
    /// no previews — pure misuse) and non-numeric input, and accept
    /// any positive `u64`.
    #[test]
    fn max_preview_bytes_rejects_zero() {
        let err = parse_max_preview_bytes("0").expect_err("zero must be rejected");
        assert!(
            err.contains("must be > 0"),
            "error must mention the positive-only constraint: {err}",
        );
        assert!(
            err.contains("'0'"),
            "error must include the offending input: {err}",
        );
    }

    #[test]
    fn max_preview_bytes_rejects_non_numeric() {
        let err = parse_max_preview_bytes("abc").expect_err("non-numeric must be rejected");
        assert!(
            err.contains("'abc'"),
            "error must include the offending input: {err}",
        );
    }

    #[test]
    fn max_preview_bytes_accepts_positive() {
        assert_eq!(parse_max_preview_bytes("1").unwrap(), 1);
        assert_eq!(
            parse_max_preview_bytes("1048576").unwrap(),
            1024 * 1024,
            "1 MiB byte count must round-trip",
        );
        assert_eq!(
            parse_max_preview_bytes("18446744073709551615").unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn max_preview_bytes_rejects_negative() {
        // u64 cannot represent negatives so parse fails as
        // "not a valid byte count" — pin the message shape.
        let err = parse_max_preview_bytes("-1").expect_err("negative must be rejected");
        assert!(
            err.contains("not a valid byte count"),
            "negative input must hit the type-parse error: {err}",
        );
    }

    /// Codex pass-11 P1: prove `--max-preview-bytes` is wired
    /// through clap end-to-end, not just through the standalone
    /// parser fn. `try_parse_from` exercises the actual derive macro
    /// output, so a future regression that drops the
    /// `value_parser = ...` attribute is caught.
    #[test]
    fn clap_init_max_preview_bytes_rejects_zero() {
        use clap::Parser;
        let err = PublishArgs::try_parse_from([
            "publish",
            "init",
            "--slug",
            "demo",
            "--clone-domain",
            "code.example.com",
            "--max-preview-bytes",
            "0",
        ])
        .expect_err("clap must reject --max-preview-bytes=0");
        let rendered = err.to_string();
        assert!(
            rendered.contains("must be > 0"),
            "clap error must surface the positive-only constraint: {rendered}",
        );
    }

    #[test]
    fn clap_init_max_preview_bytes_accepts_positive() {
        use clap::Parser;
        let parsed = PublishArgs::try_parse_from([
            "publish",
            "init",
            "--slug",
            "demo",
            "--clone-domain",
            "code.example.com",
            "--max-preview-bytes",
            "1048576",
        ])
        .expect("clap must accept a positive --max-preview-bytes");
        match parsed.command {
            PublishCommand::Init(args) => {
                assert_eq!(args.max_preview_bytes, Some(1024 * 1024));
            }
            _ => panic!("expected `init` subcommand"),
        }
    }

    #[test]
    fn clap_init_max_preview_bytes_rejects_non_numeric() {
        use clap::Parser;
        let err = PublishArgs::try_parse_from([
            "publish",
            "init",
            "--slug",
            "demo",
            "--clone-domain",
            "code.example.com",
            "--max-preview-bytes",
            "abc",
        ])
        .expect_err("clap must reject non-numeric --max-preview-bytes");
        let rendered = err.to_string();
        assert!(
            rendered.contains("not a valid byte count"),
            "clap error must surface the parse failure: {rendered}",
        );
    }

    #[test]
    fn clap_sync_accepts_force_and_allow_sensitive_path() {
        use clap::Parser;
        let parsed = PublishArgs::try_parse_from([
            "publish",
            "sync",
            "--ref",
            "main",
            "--force",
            "--allow-sensitive-path",
            ".env.local",
            "--allow-sensitive-path",
            "config/api-secret.json",
        ])
        .expect("clap must accept the documented sync flag set");
        match parsed.command {
            PublishCommand::Sync(args) => {
                assert!(args.force);
                assert_eq!(args.r#ref.as_deref(), Some("main"));
                assert_eq!(
                    args.allow_sensitive_path,
                    vec![
                        ".env.local".to_string(),
                        "config/api-secret.json".to_string()
                    ],
                );
            }
            _ => panic!("expected `sync` subcommand"),
        }
    }

    #[test]
    fn publish_init_materializes_worker_template_and_manifest() {
        let temp = tempfile::tempdir().expect("temp dir must be created");

        let output = run_publish_init_at_root(temp.path(), &default_init_args())
            .expect("publish init must materialize the embedded worker template");

        assert!(output.files_written > 0);
        assert_eq!(output.files_current, 0);

        let package_json = temp.path().join("worker/package.json");
        let expected_package_json = WorkerTemplate::get("package.json")
            .expect("embedded package.json must exist")
            .data
            .into_owned();
        assert_eq!(
            fs::read(&package_json).expect("materialized package.json must be readable"),
            expected_package_json
        );

        let manifest_path = temp.path().join(WORKER_TEMPLATE_MANIFEST_PATH);
        let manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest must be readable"))
                .expect("manifest must be valid JSON");
        assert_eq!(
            manifest["schemaVersion"],
            WORKER_TEMPLATE_MANIFEST_SCHEMA_VERSION
        );
        assert_eq!(manifest["templateVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest["workerDir"], "worker");

        let files = manifest["files"]
            .as_array()
            .expect("manifest files must be an array");
        assert!(
            files.iter().any(|file| {
                file["path"] == "package.json"
                    && file["renderPolicy"] == "managed_template"
                    && file["sha256"].as_str().is_some_and(|hash| hash.len() == 64)
            }),
            "manifest must record package.json with its template hash"
        );

        let rerun = run_publish_init_at_root(temp.path(), &default_init_args())
            .expect("publish init must be idempotent for byte-identical files");
        assert_eq!(rerun.files_written, 0);
        assert_eq!(rerun.files_current, output.files_written);
    }

    #[test]
    fn publish_init_refuses_to_overwrite_modified_template_file() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        let worker_dir = temp.path().join("worker");
        fs::create_dir_all(&worker_dir).expect("worker dir must be created");
        fs::write(worker_dir.join("package.json"), b"{\"custom\":true}\n")
            .expect("custom package.json must be writable");

        let err = run_publish_init_at_root(temp.path(), &default_init_args())
            .expect_err("publish init must fail closed on modified template files");

        assert_eq!(err.stable_code(), StableErrorCode::ConflictOperationBlocked);
        assert!(
            err.message().contains("package.json"),
            "conflict error must identify the changed file: {}",
            err.message()
        );
        assert_eq!(
            fs::read_to_string(worker_dir.join("package.json"))
                .expect("custom package.json must remain readable"),
            "{\"custom\":true}\n"
        );
        assert!(
            !temp.path().join(WORKER_TEMPLATE_MANIFEST_PATH).exists(),
            "manifest must not be written after a template conflict"
        );
    }

    #[cfg(unix)]
    #[test]
    fn publish_init_refuses_worker_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp dir must be created");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).expect("outside dir must be created");
        symlink(&outside, temp.path().join("worker")).expect("worker symlink must be created");

        let err = run_publish_init_at_root(temp.path(), &default_init_args())
            .expect_err("publish init must refuse symlinked worker roots");

        assert_eq!(err.stable_code(), StableErrorCode::ConflictOperationBlocked);
        assert!(
            err.message().contains("worker"),
            "conflict error must identify the symlinked worker root: {}",
            err.message()
        );
        assert!(
            !outside.join("package.json").exists(),
            "publish init must not write template files through a worker symlink"
        );
    }
}
