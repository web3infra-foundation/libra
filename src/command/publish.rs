//! `libra publish` — read-only Cloudflare publishing.
//!
//! Per `docs/improvement/publish.md`, the publish CLI surface is
//! `init` / `sync` / `status` / `deploy` / `unpublish`. `init` now
//! materialises the embedded Worker template, `sync --dry-run` plans
//! local refs without cloud writes, `status` reports local template
//! drift, `deploy` validates/builds the Worker before optionally
//! applying D1 migrations and deploying with Wrangler, and
//! `unpublish --yes` disables a site through Wrangler D1 execute. The
//! remaining full sync upload and cloud status comparison still surface
//! a clear unsupported error until their cloud mutation plumbing ships.
//!
//! Unsupported full-sync/cloud-comparison paths return a typed error
//! pointing the user at:
//!
//!   * the relevant `libra cloud sync` baseline that is implemented
//!     (Phase 1's `run_cloud_sync` helper),
//!   * the publish.md design doc,
//!   * the remaining publish.md v1 sync/status work.
//!
//! Codex pass-7 P1 registered the CLI surface so the `clap` parser
//! would not reject `libra publish ...`. Later slices filled in local
//! template management, dry-run planning, Worker deployment, and
//! unpublish orchestration without changing the full-upload boundary.

use std::{
    collections::BTreeMap,
    fs, io,
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
    str::FromStr,
};

use clap::{Parser, Subcommand};
use git_internal::{
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
};
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};

use crate::{
    command::{load_object, status},
    internal::{
        branch::Branch,
        config::ConfigKv,
        head::Head,
        publish::{
            preflight::{DenyReason, Preflight, PreflightDecision},
            snapshot::{RefInput, detect_ambiguous_short_refs, validate_oid, validate_ref_name},
            worker_template::{MANIFEST, RenderPolicy, WorkerTemplate, embed_path_is_allowed},
        },
        tag::{self, TagObject},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        object_ext::TreeExt,
        output::{self, CommandOutput, OutputConfig},
        util,
    },
};

#[derive(Parser, Debug)]
#[command(about = "Manage read-only Cloudflare Worker publishing")]
pub struct PublishArgs {
    #[command(subcommand)]
    pub command: PublishCommand,
}

#[derive(Subcommand, Debug)]
pub enum PublishCommand {
    /// Materialise the local Worker template scaffold.
    Init(InitArgs),
    /// Plan publish sync; full D1/R2 upload is still unsupported unless --dry-run is used.
    Sync(SyncArgs),
    /// Inspect local Worker template status; cloud ref comparison is still pending.
    Status(StatusArgs),
    /// Build and optionally deploy the Cloudflare Worker.
    Deploy(DeployArgs),
    /// Disable a published site without deleting D1/R2 data.
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
    /// Skip Cloudflare mutation steps after the local Worker build.
    #[arg(long)]
    pub skip_deploy: bool,
}

#[derive(Parser, Debug)]
pub struct UnpublishArgs {
    /// Confirm the unpublish operation.
    #[arg(long)]
    pub yes: bool,

    /// Site UUID to disable. Defaults to `publish.site_id` config.
    #[arg(long)]
    pub site_id: Option<String>,
}

const NOT_YET_IMPLEMENTED: &str = "`libra publish sync` full D1/R2 upload is not ready yet. \
     `libra publish init`, `libra publish sync --dry-run`, `libra publish status`, \
     `libra publish deploy`, and `libra publish unpublish --yes` are available; track \
     docs/improvement/publish.md for the remaining v1 sync/status work.";
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
        PublishCommand::Sync(sync_args) => {
            if !sync_args.dry_run {
                return unsupported_publish_subcommand("sync");
            }
            let result = run_publish_sync_dry_run(&sync_args).await?;
            output::emit(&result, output)
        }
        PublishCommand::Status(_) => {
            let repo_root = util::try_working_dir().map_err(|source| {
                CliError::fatal(format!("failed to resolve Libra repository root: {source}"))
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            })?;
            let result = run_publish_status_at_root(&repo_root)?;
            output::emit(&result, output)
        }
        PublishCommand::Deploy(deploy_args) => {
            let repo_root = util::try_working_dir().map_err(|source| {
                CliError::fatal(format!("failed to resolve Libra repository root: {source}"))
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            })?;
            let mut runner = ProcessPublishWorkerCommandRunner;
            let result = run_publish_deploy_at_root(&repo_root, &deploy_args, &mut runner)?;
            output::emit(&result, output)
        }
        PublishCommand::Unpublish(unpublish_args) => {
            if !unpublish_args.yes {
                return Err(CliError::command_usage(
                    "publish unpublish requires --yes to confirm disabling the site",
                ));
            }
            let repo_root = util::try_working_dir().map_err(|source| {
                CliError::fatal(format!("failed to resolve Libra repository root: {source}"))
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            })?;
            let site_id = resolve_unpublish_site_id(&unpublish_args).await?;
            let mut runner = ProcessPublishWorkerCommandRunner;
            let result =
                run_publish_unpublish_at_root(&repo_root, &unpublish_args, &site_id, &mut runner)?;
            output::emit(&result, output)
        }
    }
}

#[derive(Debug)]
struct TemplateFile {
    path: String,
    bytes: Vec<u8>,
    sha256: String,
    render_policy: RenderPolicy,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerTemplateManifest {
    schema_version: u32,
    template_version: String,
    worker_dir: String,
    files: Vec<WorkerTemplateManifestFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerTemplateManifestFile {
    path: String,
    render_policy: String,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishSyncDryRunOutput {
    dry_run: bool,
    site_id: Option<String>,
    selected_ref: Option<String>,
    refs_count: usize,
    revision_count: usize,
    default_ref: Option<String>,
    latest_revision_oid: Option<String>,
    file_count: usize,
    ai_object_count: usize,
    ai_bundle_count: usize,
    updates_full_refs_generation: bool,
    refs: Vec<PublishSyncRefOutput>,
    revisions: Vec<PublishSyncRevisionOutput>,
    warnings: Vec<String>,
}

impl CommandOutput for PublishSyncDryRunOutput {
    fn render_human(&self, writer: &mut dyn Write, output: &OutputConfig) -> io::Result<()> {
        if output.quiet {
            return Ok(());
        }
        writeln!(writer, "Publish dry-run plan")?;
        writeln!(writer, "  refs: {}", self.refs_count)?;
        writeln!(writer, "  revisions: {}", self.revision_count)?;
        writeln!(
            writer,
            "  default ref: {}",
            self.default_ref.as_deref().unwrap_or("<none>")
        )?;
        writeln!(
            writer,
            "  latest revision: {}",
            self.latest_revision_oid.as_deref().unwrap_or("<none>")
        )?;
        writeln!(writer, "  files: {}", self.file_count)?;
        writeln!(writer, "  AI objects: {}", self.ai_object_count)?;
        writeln!(writer, "  AI bundles: {}", self.ai_bundle_count)?;
        writeln!(
            writer,
            "  updates full refs generation: {}",
            self.updates_full_refs_generation
        )?;
        if !self.warnings.is_empty() {
            writeln!(writer, "  warnings:")?;
            for warning in &self.warnings {
                writeln!(writer, "    - {warning}")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PublishDeployStepState {
    Completed,
    Skipped,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishDeployStepSummary {
    name: String,
    command: Vec<String>,
    state: PublishDeployStepState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishDeployOutput {
    worker_dir: String,
    template_status: WorkerTemplateStatus,
    steps: Vec<PublishDeployStepSummary>,
    deploy_url: Option<String>,
}

impl CommandOutput for PublishDeployOutput {
    fn render_human(&self, writer: &mut dyn Write, output: &OutputConfig) -> io::Result<()> {
        if output.quiet {
            return Ok(());
        }
        writeln!(writer, "Publish Worker deploy")?;
        writeln!(writer, "  worker: {}", self.worker_dir)?;
        writeln!(
            writer,
            "  template status: {}",
            self.template_status.as_str()
        )?;
        for step in &self.steps {
            let status = match step.state {
                PublishDeployStepState::Completed => "completed",
                PublishDeployStepState::Skipped => "skipped",
            };
            writeln!(writer, "  {status}: {}", step.command.join(" "))?;
        }
        writeln!(
            writer,
            "  deploy URL: {}",
            self.deploy_url.as_deref().unwrap_or("<skipped>")
        )?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishUnpublishOutput {
    worker_dir: String,
    site_id: String,
    status: String,
    command: Vec<String>,
}

impl CommandOutput for PublishUnpublishOutput {
    fn render_human(&self, writer: &mut dyn Write, output: &OutputConfig) -> io::Result<()> {
        if output.quiet {
            return Ok(());
        }
        writeln!(writer, "Unpublished site {}", self.site_id)?;
        writeln!(writer, "  worker: {}", self.worker_dir)?;
        writeln!(writer, "  status: {}", self.status)?;
        writeln!(writer, "  command: {}", self.command.join(" "))?;
        Ok(())
    }
}

#[derive(Debug)]
struct PublishWorkerCommandOutput {
    success: bool,
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
}

trait PublishWorkerCommandRunner {
    fn run(
        &mut self,
        worker_dir: &Path,
        program: &str,
        args: &[&str],
    ) -> io::Result<PublishWorkerCommandOutput>;
}

struct ProcessPublishWorkerCommandRunner;

impl PublishWorkerCommandRunner for ProcessPublishWorkerCommandRunner {
    fn run(
        &mut self,
        worker_dir: &Path,
        program: &str,
        args: &[&str],
    ) -> io::Result<PublishWorkerCommandOutput> {
        let output = Command::new(program)
            .args(args)
            .current_dir(worker_dir)
            .env("CI", "1")
            .output()?;
        Ok(PublishWorkerCommandOutput {
            success: output.status.success(),
            status_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn run_publish_deploy_at_root(
    repo_root: &Path,
    args: &DeployArgs,
    runner: &mut dyn PublishWorkerCommandRunner,
) -> CliResult<PublishDeployOutput> {
    let status = run_publish_status_at_root(repo_root)?;
    ensure_publish_deploy_template_ready(&status)?;
    let worker_dir = repo_root.join("worker");
    ensure_publish_deploy_files(&worker_dir)?;

    let mut steps = Vec::new();
    run_publish_deploy_step(runner, &worker_dir, "build", "pnpm", &["build"], &mut steps)?;

    let mut deploy_url = None;
    if args.skip_deploy {
        steps.push(PublishDeployStepSummary {
            name: "d1_migrations".to_string(),
            command: command_summary(
                "pnpm",
                &[
                    "exec",
                    "wrangler",
                    "d1",
                    "migrations",
                    "apply",
                    "LIBRA_PUBLISH_DB",
                    "--remote",
                ],
            ),
            state: PublishDeployStepState::Skipped,
        });
        steps.push(PublishDeployStepSummary {
            name: "deploy".to_string(),
            command: command_summary("pnpm", &["exec", "opennextjs-cloudflare", "deploy"]),
            state: PublishDeployStepState::Skipped,
        });
    } else {
        run_publish_deploy_step(
            runner,
            &worker_dir,
            "d1_migrations",
            "pnpm",
            &[
                "exec",
                "wrangler",
                "d1",
                "migrations",
                "apply",
                "LIBRA_PUBLISH_DB",
                "--remote",
            ],
            &mut steps,
        )?;
        let output = run_publish_deploy_step(
            runner,
            &worker_dir,
            "deploy",
            "pnpm",
            &["exec", "opennextjs-cloudflare", "deploy"],
            &mut steps,
        )?;
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        deploy_url = extract_first_url(&combined);
        if deploy_url.is_none() {
            return Err(CliError::fatal(
                "publish deploy completed but no deployment URL was found in Wrangler output",
            )
            .with_stable_code(StableErrorCode::NetworkProtocol)
            .with_hint("inspect the deploy output and verify the Worker route/domain."));
        }
    }

    Ok(PublishDeployOutput {
        worker_dir: "worker".to_string(),
        template_status: status.status,
        steps,
        deploy_url,
    })
}

async fn resolve_unpublish_site_id(args: &UnpublishArgs) -> CliResult<String> {
    if let Some(site_id) = args.site_id.as_deref() {
        return validate_publish_site_id(site_id);
    }

    let entry = ConfigKv::get("publish.site_id").await.map_err(|source| {
        CliError::fatal(format!(
            "failed to read publish.site_id from repository config: {source}"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    let Some(entry) = entry else {
        return Err(CliError::failure(
            "publish unpublish requires --site-id or publish.site_id config",
        )
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint("pass '--site-id <uuid>' or configure publish.site_id before unpublishing."));
    };
    validate_publish_site_id(&entry.value)
}

fn validate_publish_site_id(site_id: &str) -> CliResult<String> {
    uuid::Uuid::parse_str(site_id)
        .map(|uuid| uuid.to_string())
        .map_err(|source| {
            CliError::failure(format!(
                "publish site id '{site_id}' is not a valid UUID: {source}"
            ))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint("use the UUID stored in publish.site_id or the D1 publish_sites row.")
        })
}

fn run_publish_unpublish_at_root(
    repo_root: &Path,
    args: &UnpublishArgs,
    site_id: &str,
    runner: &mut dyn PublishWorkerCommandRunner,
) -> CliResult<PublishUnpublishOutput> {
    if !args.yes {
        return Err(CliError::command_usage(
            "publish unpublish requires --yes to confirm disabling the site",
        ));
    }

    let status = run_publish_status_at_root(repo_root)?;
    ensure_publish_deploy_template_ready(&status)?;
    let worker_dir = repo_root.join("worker");
    ensure_publish_deploy_files(&worker_dir)?;

    let sql = format!(
        "UPDATE publish_sites SET status = 'disabled', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE site_id = '{site_id}';"
    );
    let command = command_summary(
        "pnpm",
        &[
            "exec",
            "wrangler",
            "d1",
            "execute",
            "LIBRA_PUBLISH_DB",
            "--remote",
            "--yes",
            "--command",
            &sql,
        ],
    );
    let output = runner
        .run(
            &worker_dir,
            "pnpm",
            &[
                "exec",
                "wrangler",
                "d1",
                "execute",
                "LIBRA_PUBLISH_DB",
                "--remote",
                "--yes",
                "--command",
                &sql,
            ],
        )
        .map_err(|source| {
            CliError::fatal(format!(
                "failed to start publish unpublish command: {source}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    if !output.success {
        return Err(CliError::fatal(format!(
            "publish unpublish failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output
                .status_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string()),
            output.stdout.trim(),
            output.stderr.trim(),
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
        .with_hint("fix the Wrangler D1 error and rerun 'libra publish unpublish --yes'."));
    }

    Ok(PublishUnpublishOutput {
        worker_dir: "worker".to_string(),
        site_id: site_id.to_string(),
        status: "disabled".to_string(),
        command,
    })
}

fn ensure_publish_deploy_template_ready(status: &PublishStatusOutput) -> CliResult<()> {
    match status.status {
        WorkerTemplateStatus::Current | WorkerTemplateStatus::Modified => Ok(()),
        WorkerTemplateStatus::Missing => Err(CliError::failure(
            "publish deploy requires a local Worker template, but it is missing",
        )
        .with_stable_code(StableErrorCode::RepoStateInvalid)
        .with_hint("run 'libra publish init' before deploying.")),
        WorkerTemplateStatus::Outdated => Err(CliError::failure(
            "publish deploy requires the Worker template to be current or intentionally modified",
        )
        .with_stable_code(StableErrorCode::RepoStateInvalid)
        .with_hint("rerun 'libra publish init' and review any Worker template changes.")),
        WorkerTemplateStatus::Conflicted => Err(CliError::conflict(
            "publish deploy cannot continue while Worker template paths are conflicted",
        )
        .with_hint("resolve symlinks or non-file paths under worker/, then rerun deploy.")),
    }
}

fn ensure_publish_deploy_files(worker_dir: &Path) -> CliResult<()> {
    for relative in [
        "package.json",
        "pnpm-lock.yaml",
        "wrangler.jsonc",
        "migrations/0001_publish.sql",
    ] {
        let path = worker_dir.join(relative);
        if !path.is_file() {
            return Err(
                CliError::failure(format!("publish deploy requires '{}'", path.display()))
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
                    .with_hint("run 'libra publish init' to materialize the Worker template."),
            );
        }
    }

    let wrangler_path = worker_dir.join("wrangler.jsonc");
    let wrangler = fs::read_to_string(&wrangler_path).map_err(|source| {
        CliError::fatal(format!(
            "failed to read Worker Wrangler config '{}': {source}",
            wrangler_path.display()
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for required in ["LIBRA_PUBLISH_DB", "LIBRA_PUBLISH_BUCKET", "ASSETS"] {
        if !wrangler.contains(required) {
            return Err(CliError::failure(format!(
                "publish deploy requires Worker binding '{required}' in '{}'",
                wrangler_path.display()
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("restore the Worker bindings generated by 'libra publish init'."));
        }
    }
    if wrangler.contains("REPLACE_WITH_D1_DATABASE_ID") {
        return Err(CliError::failure(format!(
            "publish deploy requires a real D1 database_id in '{}' instead of \
             REPLACE_WITH_D1_DATABASE_ID",
            wrangler_path.display(),
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
        .with_hint("create a Cloudflare D1 database and replace REPLACE_WITH_D1_DATABASE_ID."));
    }
    Ok(())
}

fn run_publish_deploy_step(
    runner: &mut dyn PublishWorkerCommandRunner,
    worker_dir: &Path,
    name: &str,
    program: &str,
    args: &[&str],
    steps: &mut Vec<PublishDeployStepSummary>,
) -> CliResult<PublishWorkerCommandOutput> {
    let output = runner.run(worker_dir, program, args).map_err(|source| {
        CliError::fatal(format!(
            "failed to start publish deploy step '{name}' ({}): {source}",
            command_summary(program, args).join(" ")
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    if !output.success {
        return Err(CliError::fatal(format!(
            "publish deploy step '{name}' failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output
                .status_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string()),
            output.stdout.trim(),
            output.stderr.trim(),
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
        .with_hint("fix the Worker build/deploy error and rerun 'libra publish deploy'."));
    }
    steps.push(PublishDeployStepSummary {
        name: name.to_string(),
        command: command_summary(program, args),
        state: PublishDeployStepState::Completed,
    });
    Ok(output)
}

fn command_summary(program: &str, args: &[&str]) -> Vec<String> {
    std::iter::once(program.to_string())
        .chain(args.iter().map(|arg| (*arg).to_string()))
        .collect()
}

fn extract_first_url(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | ','
                )
            })
        })
        .find(|token| token.starts_with("https://") || token.starts_with("http://"))
        .map(|token| {
            token
                .trim_end_matches(['.', ',', ';', ')', ']', '>'])
                .to_string()
        })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishSyncRefOutput {
    ref_name: String,
    target_oid: String,
    revision_oid: String,
    is_default: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishSyncRevisionOutput {
    revision_oid: String,
    ref_count: usize,
    file_count: usize,
    preflight_denied_count: usize,
    ai_object_count: usize,
    ai_bundle_count: usize,
}

async fn run_publish_sync_dry_run(args: &SyncArgs) -> CliResult<PublishSyncDryRunOutput> {
    validate_publish_sync_args(args)?;

    let all_refs = collect_publish_refs().await?;
    if all_refs.is_empty() {
        return Err(
            CliError::failure("no local branch or tag refs are available to publish")
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("create a commit on a local branch or tag a commit before publishing."),
        );
    }

    let selected_refs = select_publish_refs(&all_refs, args.r#ref.as_deref())?;
    let default_ref = resolve_publish_default_ref(&all_refs).await?;
    let selected_ref = if args.r#ref.is_some() {
        selected_refs
            .first()
            .map(|publish_ref| publish_ref.ref_name.clone())
    } else {
        None
    };
    let mut warnings = inspect_publish_dirty(args.fail_on_dirty).await?;
    if selected_ref.is_some() {
        warnings.push(
            "targeted --ref dry-run will not update the complete published refs generation"
                .to_string(),
        );
    }
    if args.force {
        warnings.push("--force has no effect during dry-run".to_string());
    }
    if !args.allow_sensitive_path.is_empty() {
        warnings.push(
            "--allow-sensitive-path is recorded for sync planning but dry-run does not evaluate \
             site visibility"
                .to_string(),
        );
    }

    let mut revision_ref_counts: BTreeMap<String, usize> = BTreeMap::new();
    for publish_ref in &selected_refs {
        *revision_ref_counts
            .entry(publish_ref.revision_oid.clone())
            .or_default() += 1;
    }

    let mut revisions = Vec::with_capacity(revision_ref_counts.len());
    for (revision_oid, ref_count) in revision_ref_counts {
        let scan = scan_revision_files(&revision_oid)?;
        for denied in &scan.denied_paths {
            warnings.push(format!(
                "publish preflight denied '{}' in revision {} ({})",
                denied.path,
                revision_oid,
                preflight_reason_label(denied.reason)
            ));
        }
        revisions.push(PublishSyncRevisionOutput {
            revision_oid,
            ref_count,
            file_count: scan.file_count,
            preflight_denied_count: scan.denied_paths.len(),
            ai_object_count: 0,
            ai_bundle_count: 0,
        });
    }

    let file_count = revisions.iter().map(|revision| revision.file_count).sum();
    let ai_object_count = revisions
        .iter()
        .map(|revision| revision.ai_object_count)
        .sum();
    let ai_bundle_count = revisions
        .iter()
        .map(|revision| revision.ai_bundle_count)
        .sum();
    let latest_revision_oid = default_ref
        .as_ref()
        .and_then(|name| {
            selected_refs
                .iter()
                .find(|publish_ref| &publish_ref.ref_name == name)
        })
        .or_else(|| selected_refs.first())
        .map(|publish_ref| publish_ref.revision_oid.clone());

    let refs = selected_refs
        .into_iter()
        .map(|publish_ref| {
            let is_default = default_ref
                .as_ref()
                .is_some_and(|name| name == &publish_ref.ref_name);
            PublishSyncRefOutput {
                ref_name: publish_ref.ref_name,
                target_oid: publish_ref.target_oid,
                revision_oid: publish_ref.revision_oid,
                is_default,
            }
        })
        .collect::<Vec<_>>();

    Ok(PublishSyncDryRunOutput {
        dry_run: true,
        site_id: None,
        selected_ref,
        refs_count: refs.len(),
        revision_count: revisions.len(),
        default_ref,
        latest_revision_oid,
        file_count,
        ai_object_count,
        ai_bundle_count,
        updates_full_refs_generation: args.r#ref.is_none(),
        refs,
        revisions,
        warnings,
    })
}

fn validate_publish_sync_args(args: &SyncArgs) -> CliResult<()> {
    match args.ai_redaction.as_str() {
        "default" | "strict" => Ok(()),
        value => Err(CliError::command_usage(format!(
            "invalid --ai-redaction value '{value}'; expected 'default' or 'strict'"
        ))),
    }
}

async fn collect_publish_refs() -> CliResult<Vec<RefInput>> {
    let branches = Branch::list_branches_result(None).await.map_err(|source| {
        CliError::fatal(format!(
            "failed to list local branches for publish dry-run: {source}"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    let mut refs = Vec::new();
    for branch in branches {
        let target_oid = branch.commit.to_string();
        refs.push(RefInput {
            ref_name: format!("refs/heads/{}", branch.name),
            target_oid: target_oid.clone(),
            revision_oid: target_oid,
        });
    }

    let tags = tag::list().await.map_err(|source| {
        CliError::fatal(format!(
            "failed to list local tags for publish dry-run: {source}"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    for publish_tag in tags {
        let ref_name = format!("refs/tags/{}", publish_tag.name);
        let (target_oid, revision_oid) = match publish_tag.object {
            TagObject::Commit(commit) => {
                let oid = commit.id.to_string();
                (oid.clone(), oid)
            }
            TagObject::Tag(tag_object) => {
                let revision_oid = match tag_object.object_type {
                    ObjectType::Commit => tag_object.object_hash,
                    ObjectType::Tag => util::get_commit_base_typed(&publish_tag.name)
                        .await
                        .map_err(|source| {
                            CliError::fatal(format!(
                                "failed to peel publish tag '{}' to a commit: {source}",
                                publish_tag.name
                            ))
                            .with_stable_code(StableErrorCode::RepoStateInvalid)
                        })?,
                    target_type => {
                        return Err(CliError::failure(format!(
                            "publish tag '{}' does not point to a commit; target type is \
                             {target_type}",
                            publish_tag.name
                        ))
                        .with_stable_code(StableErrorCode::CliInvalidTarget)
                        .with_hint("publish only branch and tag refs that resolve to commits."));
                    }
                };
                (tag_object.id.to_string(), revision_oid.to_string())
            }
            TagObject::Tree(_) | TagObject::Blob(_) => {
                return Err(CliError::failure(format!(
                    "publish tag '{}' does not point to a commit",
                    publish_tag.name
                ))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("publish only branch and tag refs that resolve to commits."));
            }
        };
        refs.push(RefInput {
            ref_name,
            target_oid,
            revision_oid,
        });
    }

    refs.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    for publish_ref in &refs {
        validate_ref_name(&publish_ref.ref_name).map_err(snapshot_ref_error)?;
        validate_oid(&publish_ref.target_oid).map_err(snapshot_ref_error)?;
        validate_oid(&publish_ref.revision_oid).map_err(snapshot_ref_error)?;
    }
    Ok(refs)
}

fn select_publish_refs(all_refs: &[RefInput], selected: Option<&str>) -> CliResult<Vec<RefInput>> {
    let Some(raw_ref) = selected else {
        return Ok(all_refs.to_vec());
    };
    let trimmed = raw_ref.trim();
    if trimmed.is_empty() || trimmed != raw_ref {
        return Err(CliError::command_usage(
            "--ref must be a non-empty branch, tag, or full refs/heads/* / refs/tags/* name",
        ));
    }

    let selected_full_ref = if raw_ref.starts_with("refs/") {
        validate_ref_name(raw_ref).map_err(snapshot_ref_error)?;
        raw_ref.to_string()
    } else {
        let ambiguous = detect_ambiguous_short_refs(all_refs);
        if ambiguous.iter().any(|short| short == raw_ref) {
            return Err(CliError::failure(format!(
                "ambiguous publish ref '{raw_ref}' matches both a branch and a tag"
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint(format!(
                "use 'refs/heads/{raw_ref}' or 'refs/tags/{raw_ref}' to select one."
            )));
        }

        let matches = all_refs
            .iter()
            .filter(|publish_ref| publish_short_ref_name(&publish_ref.ref_name) == Some(raw_ref))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [publish_ref] => publish_ref.ref_name.clone(),
            [] => {
                return Err(CliError::failure(format!(
                    "publish ref '{raw_ref}' was not found among local branches or tags"
                ))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("run 'libra show-ref --heads --tags' to inspect publishable refs."));
            }
            _ => {
                return Err(CliError::failure(format!(
                    "ambiguous publish ref '{raw_ref}' matches multiple refs"
                ))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_hint("use a full refs/heads/* or refs/tags/* name to select one."));
            }
        }
    };

    all_refs
        .iter()
        .find(|publish_ref| publish_ref.ref_name == selected_full_ref)
        .cloned()
        .map(|publish_ref| vec![publish_ref])
        .ok_or_else(|| {
            CliError::failure(format!(
                "publish ref '{selected_full_ref}' was not found among local branches or tags"
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint("run 'libra show-ref --heads --tags' to inspect publishable refs.")
        })
}

fn publish_short_ref_name(full_ref: &str) -> Option<&str> {
    full_ref
        .strip_prefix("refs/heads/")
        .or_else(|| full_ref.strip_prefix("refs/tags/"))
}

async fn resolve_publish_default_ref(all_refs: &[RefInput]) -> CliResult<Option<String>> {
    let head = Head::current_result().await.map_err(|source| {
        CliError::fatal(format!(
            "failed to resolve HEAD while planning publish dry-run: {source}"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    if let Head::Branch(branch_name) = head {
        let full_ref = format!("refs/heads/{branch_name}");
        if all_refs
            .iter()
            .any(|publish_ref| publish_ref.ref_name == full_ref)
        {
            return Ok(Some(full_ref));
        }
    }

    Ok(all_refs
        .iter()
        .find(|publish_ref| publish_ref.ref_name == "refs/heads/main")
        .or_else(|| {
            all_refs
                .iter()
                .find(|publish_ref| publish_ref.ref_name.starts_with("refs/heads/"))
        })
        .or_else(|| all_refs.first())
        .map(|publish_ref| publish_ref.ref_name.clone()))
}

async fn inspect_publish_dirty(fail_on_dirty: bool) -> CliResult<Vec<String>> {
    let staged = status::changes_to_be_committed_safe()
        .await
        .map_err(CliError::from)?;
    let unstaged = status::changes_to_be_staged().map_err(CliError::from)?;
    let staged_count = staged.polymerization().len();
    let unstaged_count = unstaged.polymerization().len();
    if staged_count == 0 && unstaged_count == 0 {
        return Ok(Vec::new());
    }

    let message = format!(
        "dirty working tree has {staged_count} staged path(s) and {unstaged_count} unstaged or \
         untracked path(s); dry-run plans committed refs only"
    );
    if fail_on_dirty {
        Err(CliError::fatal(message)
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint(
                "commit, stash, or discard local changes before running with --fail-on-dirty.",
            ))
    } else {
        Ok(vec![message])
    }
}

#[derive(Debug)]
struct RevisionDryRunScan {
    file_count: usize,
    denied_paths: Vec<PreflightDeniedPath>,
}

#[derive(Debug)]
struct PreflightDeniedPath {
    path: String,
    reason: DenyReason,
}

fn scan_revision_files(revision_oid: &str) -> CliResult<RevisionDryRunScan> {
    let commit_oid = ObjectHash::from_str(revision_oid).map_err(|source| {
        CliError::fatal(format!(
            "publish revision oid '{revision_oid}' is invalid: {source}"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    let commit: Commit = load_object(&commit_oid).map_err(|source| {
        CliError::fatal(format!(
            "failed to load publish revision commit '{revision_oid}': {source}"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    let tree: Tree = load_object(&commit.tree_id).map_err(|source| {
        CliError::fatal(format!(
            "failed to load publish revision tree '{}' for commit '{revision_oid}': {source}",
            commit.tree_id
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    let items = tree.get_plain_items();
    let preflight = preflight_for_revision_items(&items, revision_oid)?;
    let mut denied_paths = Vec::new();
    for (path, _) in &items {
        if let PreflightDecision::Deny(reason) = preflight.evaluate(path, false) {
            denied_paths.push(PreflightDeniedPath {
                path: path.display().to_string(),
                reason,
            });
        }
    }

    Ok(RevisionDryRunScan {
        file_count: items.len(),
        denied_paths,
    })
}

fn preflight_for_revision_items(
    items: &[(PathBuf, ObjectHash)],
    revision_oid: &str,
) -> CliResult<Preflight> {
    let mut preflight = Preflight::new();
    let ignore_path = Path::new(".librapublishignore");
    let Some((_, ignore_oid)) = items.iter().find(|(path, _)| path == ignore_path) else {
        return Ok(preflight);
    };

    let blob: Blob = load_object(ignore_oid).map_err(|source| {
        CliError::fatal(format!(
            "failed to load .librapublishignore for publish revision '{revision_oid}': {source}"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    let text = std::str::from_utf8(&blob.data).map_err(|source| {
        CliError::failure(format!(
            ".librapublishignore in publish revision '{revision_oid}' is not valid UTF-8: \
             {source}"
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("commit .librapublishignore as UTF-8 text before publishing.")
    })?;
    preflight.extend_with_ignore_text(text);
    Ok(preflight)
}

fn preflight_reason_label(reason: DenyReason) -> &'static str {
    match reason {
        DenyReason::BuiltinCredential => "builtin_credential",
        DenyReason::UserIgnore => "user_ignore",
    }
}

fn snapshot_ref_error(source: impl std::error::Error) -> CliError {
    CliError::failure(format!("invalid publish ref plan: {source}"))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("publish only refs/heads/* and refs/tags/* entries with valid object ids.")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkerTemplateStatus {
    Missing,
    Current,
    Modified,
    Outdated,
    Conflicted,
}

impl WorkerTemplateStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Current => "current",
            Self::Modified => "modified",
            Self::Outdated => "outdated",
            Self::Conflicted => "conflicted",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishStatusOutput {
    worker_dir: String,
    manifest_path: String,
    template_version: &'static str,
    status: WorkerTemplateStatus,
    files_total: usize,
    files_current: usize,
    files_missing: usize,
    files_modified: usize,
    files_outdated: usize,
    files_conflicted: usize,
}

impl CommandOutput for PublishStatusOutput {
    fn render_human(&self, writer: &mut dyn Write, output: &OutputConfig) -> io::Result<()> {
        if output.quiet {
            return Ok(());
        }
        writeln!(writer, "Publish Worker template status")?;
        writeln!(writer, "  status: {}", self.status.as_str())?;
        writeln!(writer, "  worker: {}", self.worker_dir)?;
        writeln!(writer, "  manifest: {}", self.manifest_path)?;
        writeln!(writer, "  template version: {}", self.template_version)?;
        writeln!(writer, "  files total: {}", self.files_total)?;
        writeln!(writer, "  files current: {}", self.files_current)?;
        writeln!(writer, "  files missing: {}", self.files_missing)?;
        writeln!(writer, "  files modified: {}", self.files_modified)?;
        writeln!(writer, "  files outdated: {}", self.files_outdated)?;
        writeln!(writer, "  files conflicted: {}", self.files_conflicted)?;
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
        template_version: env!("CARGO_PKG_VERSION").to_string(),
        worker_dir: "worker".to_string(),
        files: files
            .iter()
            .map(|file| WorkerTemplateManifestFile {
                path: file.path.clone(),
                render_policy: render_policy_name(file.render_policy).to_string(),
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

fn run_publish_status_at_root(repo_root: &Path) -> CliResult<PublishStatusOutput> {
    let files = collect_worker_template_files()?;
    let worker_dir = repo_root.join("worker");
    let manifest_path = repo_root.join(WORKER_TEMPLATE_MANIFEST_PATH);
    let manifest = read_worker_template_manifest(&manifest_path)?;
    let manifest_hashes: BTreeMap<&str, &str> = manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .files
                .iter()
                .map(|file| (file.path.as_str(), file.sha256.as_str()))
                .collect()
        })
        .unwrap_or_default();

    let mut files_current = 0usize;
    let mut files_missing = 0usize;
    let mut files_modified = 0usize;
    let mut files_outdated = 0usize;
    let mut files_conflicted = 0usize;

    for file in &files {
        if first_existing_symlink_path(&worker_dir, &file.path)?.is_some() {
            files_conflicted += 1;
            continue;
        }

        let destination = worker_dir.join(&file.path);
        let metadata = match fs::metadata(&destination) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                files_missing += 1;
                continue;
            }
            Err(source) => {
                return Err(CliError::io(format!(
                    "failed to inspect Worker template file '{}': {source}",
                    destination.display()
                )));
            }
        };
        if !metadata.is_file() {
            files_conflicted += 1;
            continue;
        }

        let existing = fs::read(&destination).map_err(|source| {
            CliError::io(format!(
                "failed to read existing Worker template file '{}': {source}",
                destination.display()
            ))
        })?;
        let existing_sha = hex::encode(digest(&SHA256, &existing).as_ref());
        if existing_sha == file.sha256 {
            files_current += 1;
        } else if manifest_hashes
            .get(file.path.as_str())
            .is_some_and(|hash| *hash == existing_sha)
        {
            files_outdated += 1;
        } else {
            files_modified += 1;
        }
    }

    let status = if files_conflicted > 0 {
        WorkerTemplateStatus::Conflicted
    } else if files_modified > 0 {
        WorkerTemplateStatus::Modified
    } else if files_outdated > 0 {
        WorkerTemplateStatus::Outdated
    } else if files_missing > 0 || manifest.is_none() {
        WorkerTemplateStatus::Missing
    } else {
        WorkerTemplateStatus::Current
    };

    Ok(PublishStatusOutput {
        worker_dir: "worker".to_string(),
        manifest_path: WORKER_TEMPLATE_MANIFEST_PATH.to_string(),
        template_version: env!("CARGO_PKG_VERSION"),
        status,
        files_total: files.len(),
        files_current,
        files_missing,
        files_modified,
        files_outdated,
        files_conflicted,
    })
}

fn read_worker_template_manifest(path: &Path) -> CliResult<Option<WorkerTemplateManifest>> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(CliError::io(format!(
                "failed to read Worker template manifest '{}': {source}",
                path.display()
            )));
        }
    };

    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|source| {
            CliError::fatal(format!(
                "failed to parse Worker template manifest '{}': {source}",
                path.display()
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
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
    use std::{collections::VecDeque, fs};

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

    #[derive(Default)]
    struct FakePublishWorkerCommandRunner {
        calls: Vec<Vec<String>>,
        outputs: VecDeque<PublishWorkerCommandOutput>,
    }

    impl FakePublishWorkerCommandRunner {
        fn with_outputs(outputs: impl IntoIterator<Item = PublishWorkerCommandOutput>) -> Self {
            Self {
                calls: Vec::new(),
                outputs: outputs.into_iter().collect(),
            }
        }
    }

    impl PublishWorkerCommandRunner for FakePublishWorkerCommandRunner {
        fn run(
            &mut self,
            worker_dir: &Path,
            program: &str,
            args: &[&str],
        ) -> io::Result<PublishWorkerCommandOutput> {
            assert!(
                worker_dir.ends_with("worker"),
                "deploy commands must run from worker/: {}",
                worker_dir.display()
            );
            self.calls.push(command_summary(program, args));
            Ok(self
                .outputs
                .pop_front()
                .unwrap_or_else(successful_deploy_command_output))
        }
    }

    fn successful_deploy_command_output() -> PublishWorkerCommandOutput {
        PublishWorkerCommandOutput {
            success: true,
            status_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn successful_deploy_command_output_with_stdout(stdout: &str) -> PublishWorkerCommandOutput {
        PublishWorkerCommandOutput {
            success: true,
            status_code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn materialize_deployable_worker(repo_root: &Path) {
        run_publish_init_at_root(repo_root, &default_init_args())
            .expect("publish init must materialize the template");
        let wrangler_path = repo_root.join("worker/wrangler.jsonc");
        let wrangler =
            fs::read_to_string(&wrangler_path).expect("materialized wrangler config is readable");
        fs::write(
            &wrangler_path,
            wrangler.replace(
                "REPLACE_WITH_D1_DATABASE_ID",
                "00000000-0000-0000-0000-000000000000",
            ),
        )
        .expect("wrangler config placeholder should be replaceable");
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
    fn publish_status_reports_missing_before_init() {
        let temp = tempfile::tempdir().expect("temp dir must be created");

        let output = run_publish_status_at_root(temp.path())
            .expect("status should inspect missing template");

        assert_eq!(output.status, WorkerTemplateStatus::Missing);
        assert_eq!(output.files_current, 0);
        assert!(output.files_missing > 0);
    }

    #[test]
    fn publish_status_reports_current_after_init() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        run_publish_init_at_root(temp.path(), &default_init_args())
            .expect("publish init must materialize the template");

        let output =
            run_publish_status_at_root(temp.path()).expect("status should inspect template");

        assert_eq!(output.status, WorkerTemplateStatus::Current);
        assert_eq!(output.files_missing, 0);
        assert_eq!(output.files_modified, 0);
        assert_eq!(output.files_outdated, 0);
        assert_eq!(output.files_conflicted, 0);
        assert_eq!(output.files_current, output.files_total);
    }

    #[test]
    fn publish_deploy_skip_deploy_builds_worker_and_skips_cloud_mutations() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        materialize_deployable_worker(temp.path());
        let args = DeployArgs { skip_deploy: true };
        let mut runner =
            FakePublishWorkerCommandRunner::with_outputs([successful_deploy_command_output()]);

        let output = run_publish_deploy_at_root(temp.path(), &args, &mut runner)
            .expect("deploy --skip-deploy should build and skip cloud mutations");

        assert_eq!(runner.calls, vec![command_summary("pnpm", &["build"])]);
        assert_eq!(output.deploy_url, None);
        assert_eq!(output.steps.len(), 3);
        assert_eq!(output.steps[0].state, PublishDeployStepState::Completed);
        assert_eq!(output.steps[1].state, PublishDeployStepState::Skipped);
        assert_eq!(output.steps[2].state, PublishDeployStepState::Skipped);
        assert_eq!(
            output.steps[1].command,
            command_summary(
                "pnpm",
                &[
                    "exec",
                    "wrangler",
                    "d1",
                    "migrations",
                    "apply",
                    "LIBRA_PUBLISH_DB",
                    "--remote",
                ],
            )
        );
    }

    #[test]
    fn publish_deploy_applies_migrations_deploys_and_extracts_url() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        materialize_deployable_worker(temp.path());
        let args = DeployArgs { skip_deploy: false };
        let mut runner = FakePublishWorkerCommandRunner::with_outputs([
            successful_deploy_command_output(),
            successful_deploy_command_output(),
            successful_deploy_command_output_with_stdout(
                "Uploaded libra-publish\nPublished at https://libra-publish.example.workers.dev.",
            ),
        ]);

        let output = run_publish_deploy_at_root(temp.path(), &args, &mut runner)
            .expect("deploy should run build, migrations, and Worker deploy");

        assert_eq!(
            runner.calls,
            vec![
                command_summary("pnpm", &["build"]),
                command_summary(
                    "pnpm",
                    &[
                        "exec",
                        "wrangler",
                        "d1",
                        "migrations",
                        "apply",
                        "LIBRA_PUBLISH_DB",
                        "--remote",
                    ],
                ),
                command_summary("pnpm", &["exec", "opennextjs-cloudflare", "deploy"]),
            ],
        );
        assert_eq!(
            output.deploy_url.as_deref(),
            Some("https://libra-publish.example.workers.dev")
        );
        assert!(
            output
                .steps
                .iter()
                .all(|step| step.state == PublishDeployStepState::Completed)
        );
    }

    #[test]
    fn publish_deploy_requires_configured_d1_database_id_before_commands() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        run_publish_init_at_root(temp.path(), &default_init_args())
            .expect("publish init must materialize the template");
        let args = DeployArgs { skip_deploy: true };
        let mut runner = FakePublishWorkerCommandRunner::default();

        let err = run_publish_deploy_at_root(temp.path(), &args, &mut runner)
            .expect_err("deploy must fail before running commands with placeholder D1 config");

        assert_eq!(err.stable_code(), StableErrorCode::RepoStateInvalid);
        assert!(
            err.message().contains("REPLACE_WITH_D1_DATABASE_ID"),
            "error must identify the placeholder config: {}",
            err.message()
        );
        assert!(
            runner.calls.is_empty(),
            "deploy must not run build or cloud commands before config validation"
        );
    }

    #[test]
    fn publish_unpublish_requires_yes() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        materialize_deployable_worker(temp.path());
        let args = UnpublishArgs {
            yes: false,
            site_id: Some("00000000-0000-0000-0000-000000000001".to_string()),
        };
        let mut runner = FakePublishWorkerCommandRunner::default();

        let err = run_publish_unpublish_at_root(
            temp.path(),
            &args,
            "00000000-0000-0000-0000-000000000001",
            &mut runner,
        )
        .expect_err("unpublish must require explicit confirmation");

        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidArguments);
        assert!(
            runner.calls.is_empty(),
            "unpublish must not run D1 mutation before --yes confirmation"
        );
    }

    #[test]
    fn publish_unpublish_marks_site_disabled_with_wrangler_d1_execute() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        materialize_deployable_worker(temp.path());
        let args = UnpublishArgs {
            yes: true,
            site_id: Some("00000000-0000-0000-0000-000000000001".to_string()),
        };
        let mut runner =
            FakePublishWorkerCommandRunner::with_outputs([successful_deploy_command_output()]);

        let output = run_publish_unpublish_at_root(
            temp.path(),
            &args,
            "00000000-0000-0000-0000-000000000001",
            &mut runner,
        )
        .expect("unpublish should execute a D1 update through Wrangler");

        assert_eq!(output.status, "disabled");
        assert_eq!(output.site_id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(runner.calls.len(), 1);
        assert_eq!(
            &runner.calls[0][..7],
            &[
                "pnpm".to_string(),
                "exec".to_string(),
                "wrangler".to_string(),
                "d1".to_string(),
                "execute".to_string(),
                "LIBRA_PUBLISH_DB".to_string(),
                "--remote".to_string(),
            ]
        );
        assert!(
            runner.calls[0]
                .iter()
                .any(|arg| arg.contains("status = 'disabled'")
                    && arg.contains("00000000-0000-0000-0000-000000000001")),
            "D1 command must disable the selected site: {:?}",
            runner.calls[0],
        );
    }

    #[test]
    fn publish_site_id_validation_rejects_non_uuid() {
        let err =
            validate_publish_site_id("not-a-uuid").expect_err("site id must be a parseable UUID");
        assert_eq!(err.stable_code(), StableErrorCode::CliInvalidArguments);
        assert!(
            err.message().contains("not-a-uuid"),
            "error must echo the bad site id: {}",
            err.message()
        );
    }

    #[test]
    fn publish_status_reports_modified_template_file() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        run_publish_init_at_root(temp.path(), &default_init_args())
            .expect("publish init must materialize the template");
        fs::write(
            temp.path().join("worker/package.json"),
            b"{\"custom\":true}\n",
        )
        .expect("custom package.json must be writable");

        let output =
            run_publish_status_at_root(temp.path()).expect("status should inspect template");

        assert_eq!(output.status, WorkerTemplateStatus::Modified);
        assert_eq!(output.files_modified, 1);
    }

    #[test]
    fn publish_status_reports_outdated_template_file() {
        let temp = tempfile::tempdir().expect("temp dir must be created");
        run_publish_init_at_root(temp.path(), &default_init_args())
            .expect("publish init must materialize the template");
        let old_package = b"{\"old\":true}\n";
        fs::write(temp.path().join("worker/package.json"), old_package)
            .expect("old package.json must be writable");

        let manifest_path = temp.path().join(WORKER_TEMPLATE_MANIFEST_PATH);
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest must be readable"))
                .expect("manifest must be valid JSON");
        let old_sha = hex::encode(digest(&SHA256, old_package).as_ref());
        let files = manifest["files"]
            .as_array_mut()
            .expect("manifest files must be an array");
        let package = files
            .iter_mut()
            .find(|file| file["path"] == "package.json")
            .expect("manifest must contain package.json");
        package["sha256"] = Value::String(old_sha);
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("manifest must serialize"),
        )
        .expect("manifest must be writable");

        let output =
            run_publish_status_at_root(temp.path()).expect("status should inspect template");

        assert_eq!(output.status, WorkerTemplateStatus::Outdated);
        assert_eq!(output.files_outdated, 1);
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

    #[cfg(unix)]
    #[test]
    fn publish_status_reports_worker_symlink_conflict() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp dir must be created");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).expect("outside dir must be created");
        symlink(&outside, temp.path().join("worker")).expect("worker symlink must be created");

        let output =
            run_publish_status_at_root(temp.path()).expect("status should inspect symlink");

        assert_eq!(output.status, WorkerTemplateStatus::Conflicted);
        assert!(output.files_conflicted > 0);
    }
}
