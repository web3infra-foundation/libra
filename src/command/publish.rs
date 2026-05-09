//! `libra publish` — read-only Cloudflare publishing.
//!
//! Per `docs/improvement/publish.md`, the publish CLI surface is
//! `init` / `sync` / `status` / `deploy` / `unpublish`. Phase 4 lands
//! the actual implementations; this module is the Phase 6/7 CLI
//! registration so the commands parse and surface a clear "not yet
//! implemented" message until Phase 4 ships.
//!
//! Each subcommand returns a `CliInvalidArguments`-style error
//! pointing the user at:
//!
//!   * the relevant `libra cloud sync` baseline that is implemented
//!     (Phase 1's `run_cloud_sync` helper),
//!   * the publish.md design doc,
//!   * the planned Phase 4 release.
//!
//! Codex pass-7 P1: registering the CLI surface as stubs prevents the
//! `clap` parser from rejecting `libra publish ...` and gives users
//! actionable feedback while Phase 4 work proceeds. Replacing each
//! stub with a real implementation does not require any further CLI
//! wiring — the subcommand structs already carry every flag the
//! design doc lists.

use clap::{Parser, Subcommand};

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::OutputConfig,
    util,
};

#[derive(Parser, Debug)]
#[command(about = "Read-only publish to Cloudflare Workers (D1/R2)")]
pub struct PublishArgs {
    #[command(subcommand)]
    pub command: PublishCommand,
}

#[derive(Subcommand, Debug)]
pub enum PublishCommand {
    /// Initialise the local publish config + Worker scaffold.
    Init(InitArgs),
    /// Sync code, refs and AI object model to D1/R2.
    Sync(SyncArgs),
    /// Show the local↔cloud publish state.
    Status(StatusArgs),
    /// Build + deploy the Cloudflare Worker.
    Deploy(DeployArgs),
    /// Mark the published site disabled (410 from Worker API).
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

    /// Emit machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct StatusArgs {
    /// Emit machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
}

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

const NOT_YET_IMPLEMENTED: &str =
    "`libra publish` Phase 4 lands the implementation; the CLI surface is wired so the \
     command parses, but the executor is not yet ready. Track docs/improvement/publish.md \
     for the v1 release window.";

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

pub async fn execute(args: PublishArgs) -> CliResult<()> {
    let subcommand = match args.command {
        PublishCommand::Init(_) => "init",
        PublishCommand::Sync(_) => "sync",
        PublishCommand::Status(_) => "status",
        PublishCommand::Deploy(_) => "deploy",
        PublishCommand::Unpublish(_) => "unpublish",
    };
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

pub async fn execute_safe(args: PublishArgs, _output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    execute(args).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse_max_preview_bytes("18446744073709551615").unwrap(), u64::MAX);
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
                    vec![".env.local".to_string(), "config/api-secret.json".to_string()],
                );
            }
            _ => panic!("expected `sync` subcommand"),
        }
    }
}
