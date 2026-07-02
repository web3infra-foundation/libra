//! `libra metadata` — branch/repo metadata KV (lore.md §1.5, a Libra
//! extension; the nearest Git analogue is `git config branch.<name>.*`).
//!
//! The minimal v1 surface: `get`/`set`/`unset`(alias `clear`)/`list` over two
//! scopes — `--branch <name>` (the unified `metadata_kv` table, single owner
//! API [`MetadataKv`]) and `--repo` (the `config_kv` store under the
//! `metadata.*` namespace, so `libra config` tooling keeps working on the same
//! keys — an intended dual surface). `protect`/`archive`/`lineage.*` are plain
//! keys here; nothing enforces them yet (enforcement lands once, in the future
//! branch-policy layer — lore.md 1.13). The typed-metadata command family
//! (revision/file scopes, typed values) is lore.md 1.10 and extends this same
//! command.

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    internal::{
        branch::Branch,
        config::{ConfigKv, is_sensitive_key},
        metadata::{
            KEY_ARCHIVE, KEY_PROTECT, MetadataKv, MetadataScope, MetadataValueType,
            REPO_METADATA_PREFIX, validate_key, validate_typed_value, validate_value,
        },
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

pub const METADATA_EXAMPLES: &str = "\
EXAMPLES:
    libra metadata set protect true --branch main     Record branch protection (not yet enforced)
    libra metadata get protect --branch main          Read one branch metadata key
    libra metadata list --branch main                 List a branch's metadata
    libra metadata list --branch main --prefix lineage.  List only lineage.* keys
    libra metadata set owner platform-team --repo     Repo-scope metadata (stored as config metadata.owner)
    libra metadata unset owner --repo                 Remove a key (alias: clear)
    libra --json metadata get protect --branch main   Structured JSON output for agents

NOTES:
    Branch metadata lives in the metadata_kv table and follows the branch
    through rename/copy/delete. Repo metadata lives in config under the
    metadata.* namespace, so `libra config get metadata.owner` sees the same
    value. protect/archive are recorded but not yet enforced.";

/// Branch/repo metadata key-value store (Libra extension).
#[derive(Parser, Debug)]
#[command(after_help = METADATA_EXAMPLES)]
pub struct MetadataArgs {
    #[command(subcommand)]
    pub command: MetadataCommand,
}

/// Scope selector shared by every verb: exactly one of `--branch <name>` /
/// `--repo` is required (no default scope in v1 — lore.md 1.10 may add more
/// scopes and choose ergonomics later without breaking anyone).
#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
pub struct ScopeArgs {
    /// Operate on a LOCAL branch's metadata (remote-tracking branches carry none).
    #[arg(long, value_name = "NAME")]
    pub branch: Option<String>,

    /// Operate on repository-level metadata (stored in config under `metadata.*`).
    #[arg(long)]
    pub repo: bool,
}

#[derive(Subcommand, Debug)]
pub enum MetadataCommand {
    /// Read one metadata key (exit 1 when absent, like `config` key misses).
    Get {
        key: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Set one metadata key (creates or overwrites).
    Set {
        key: String,
        value: String,
        #[command(flatten)]
        scope: ScopeArgs,
        /// Declare the value numeric: an integer or finite decimal with no
        /// surrounding whitespace (validated at set time; stored exactly as
        /// given, no canonicalization).
        #[arg(long, conflicts_with = "binary")]
        numeric: bool,
        /// Declare the value binary: the VALUE argument is standard base64
        /// (validated; the encoded text is stored, so raw payloads cap at
        /// ~3/4 of the 1 MiB value limit). Decode with `| base64 -d`.
        #[arg(long)]
        binary: bool,
    },
    /// Remove one metadata key. `clear` is accepted as an alias (Lore parity).
    #[command(visible_alias = "clear")]
    Unset {
        key: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// List metadata keys for the scope, optionally filtered by key prefix.
    List {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Only list keys starting with this prefix (e.g. `lineage.`).
        #[arg(long, value_name = "PREFIX")]
        prefix: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
enum MetadataOutput {
    Get {
        scope: &'static str,
        target: String,
        key: String,
        /// `null` when the key is absent (the command then exits 1).
        value: Option<String>,
        value_type: Option<String>,
    },
    Set {
        scope: &'static str,
        target: String,
        key: String,
        value: String,
        /// The declared value type (`text`/`numeric`/`binary`) — additive field.
        value_type: String,
        /// The overwritten value, when the key already existed.
        previous: Option<String>,
    },
    Unset {
        scope: &'static str,
        target: String,
        key: String,
        removed: bool,
    },
    List {
        scope: &'static str,
        target: String,
        entries: Vec<MetadataListEntry>,
    },
}

#[derive(Debug, Serialize)]
struct MetadataListEntry {
    key: String,
    /// `<REDACTED>` for encrypted repo-scope values (same as `config list`).
    value: String,
    value_type: String,
}

enum Scope {
    Branch(String),
    Repo,
}

impl Scope {
    fn label(&self) -> &'static str {
        match self {
            Scope::Branch(_) => "branch",
            Scope::Repo => "repo",
        }
    }

    fn target(&self) -> String {
        match self {
            Scope::Branch(name) => name.clone(),
            Scope::Repo => String::new(),
        }
    }
}

/// Resolve and validate the scope: a `--branch` name must be an EXISTING local
/// branch for every verb (an explicit error beats a silently empty store).
async fn resolve_scope(scope: &ScopeArgs) -> Result<Scope, CliError> {
    if scope.repo {
        return Ok(Scope::Repo);
    }
    // clap's required group guarantees branch is Some here.
    let name = scope.branch.clone().unwrap_or_default();
    let found = Branch::find_branch_result(&name, None).await.map_err(|e| {
        CliError::fatal(format!("failed to look up branch '{name}': {e}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    if found.is_none() {
        let mut error = CliError::fatal(format!("branch '{name}' not found"))
            .with_stable_code(StableErrorCode::CliInvalidTarget);
        if name.contains('/') {
            error = error
                .with_hint("remote-tracking branches carry no metadata; use a local branch name");
        }
        return Err(error);
    }
    Ok(Scope::Branch(name))
}

fn usage_error(message: String) -> CliError {
    CliError::command_usage(message).with_stable_code(StableErrorCode::CliInvalidArguments)
}

/// Map a `ConfigKv` multi-value failure ("N values exist") to an actionable
/// usage error — the dual surface (`config set --add metadata.x`) can create
/// multi-valued keys that single-value `metadata set/unset --repo` refuses.
fn map_repo_store_error(key: &str, error: anyhow::Error) -> CliError {
    let message = error.to_string();
    if message.contains("values exist") {
        usage_error(format!(
            "repo metadata key '{key}' has multiple values (created via `config --add`)"
        ))
        .with_hint(format!(
            "run 'libra config unset-all {REPO_METADATA_PREFIX}{key}' first, then set it again"
        ))
    } else {
        CliError::fatal(format!("failed to access repo metadata '{key}': {message}"))
            .with_stable_code(StableErrorCode::IoWriteFailed)
    }
}

pub async fn execute(args: MetadataArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

pub async fn execute_safe(args: MetadataArgs, output: &OutputConfig) -> CliResult<()> {
    if crate::utils::util::require_repo().is_err() {
        return Err(CliError::repo_not_found());
    }
    match args.command {
        MetadataCommand::Get { key, scope } => {
            validate_key(&key).map_err(usage_error)?;
            let scope = resolve_scope(&scope).await?;
            let (value, value_type): (Option<String>, Option<String>) = match &scope {
                Scope::Branch(name) => MetadataKv::get(MetadataScope::Branch, name, &key)
                    .await
                    .map_err(|e| {
                        CliError::fatal(format!("failed to read metadata: {e}"))
                            .with_stable_code(StableErrorCode::IoReadFailed)
                    })?
                    .map(|entry| (Some(entry.value), Some(entry.value_type)))
                    .unwrap_or((None, None)),
                Scope::Repo => ConfigKv::get(&format!("{REPO_METADATA_PREFIX}{key}"))
                    .await
                    .map_err(|e| map_repo_store_error(&key, e))?
                    .map(|entry| {
                        // Encrypted values stay redacted here (use
                        // `config get --reveal metadata.<key>` to decrypt).
                        let value = if entry.encrypted {
                            "<REDACTED>".to_string()
                        } else {
                            entry.value
                        };
                        (Some(value), Some("text".to_string()))
                    })
                    .unwrap_or((None, None)),
            };
            let missing = value.is_none();
            let report = MetadataOutput::Get {
                scope: scope.label(),
                target: scope.target(),
                key,
                value: value.clone(),
                value_type,
            };
            if output.is_json() {
                emit_json_data("metadata", &report, output)?;
            } else if !output.quiet
                && let Some(value) = value
            {
                println!("{value}");
            }
            if missing {
                // Key miss exits 1 (like `config` key misses), after the
                // envelope/omitted-output above.
                return Err(CliError::silent_exit(1));
            }
            Ok(())
        }
        MetadataCommand::Set {
            key,
            value,
            scope,
            numeric,
            binary,
        } => {
            validate_key(&key).map_err(usage_error)?;
            validate_value(&value).map_err(usage_error)?;
            let value_type = if numeric {
                MetadataValueType::Numeric
            } else if binary {
                MetadataValueType::Binary
            } else {
                MetadataValueType::Text
            };
            validate_typed_value(value_type, &value).map_err(usage_error)?;
            let scope = resolve_scope(&scope).await?;
            // The config store has no value_type column; repo typed values are
            // an explicit follow-up (documented), not a silent downgrade.
            if matches!(scope, Scope::Repo) && value_type != MetadataValueType::Text {
                return Err(usage_error(format!(
                    "--{} is not supported for --repo metadata (the config store is text-only)",
                    value_type.as_str()
                ))
                .with_hint(
                    "repo-scope typed values are a documented follow-up; store the value as text",
                ));
            }
            let previous = match &scope {
                Scope::Branch(name) => {
                    MetadataKv::set(MetadataScope::Branch, name, &key, &value, value_type)
                        .await
                        .map_err(|e| {
                            CliError::fatal(format!("failed to write metadata: {e}"))
                                .with_stable_code(StableErrorCode::IoWriteFailed)
                        })?
                }
                Scope::Repo => {
                    let full_key = format!("{REPO_METADATA_PREFIX}{key}");
                    let existing = ConfigKv::get(&full_key)
                        .await
                        .map_err(|e| map_repo_store_error(&key, e))?;
                    // Encrypted/sensitive repo metadata is managed through the
                    // config door, which owns the vault-encryption decision.
                    // Writing plaintext here would either corrupt an existing
                    // encrypted row (ConfigKv::set preserves encrypted=1) or
                    // store a secret unencrypted — refuse both.
                    if existing.as_ref().is_some_and(|entry| entry.encrypted)
                        || is_sensitive_key(&full_key)
                    {
                        return Err(usage_error(format!(
                            "repo metadata key '{key}' is encrypted or sensitive"
                        ))
                        .with_hint(format!(
                            "set it through the config door instead: libra config {full_key} <value>"
                        )));
                    }
                    let previous = existing.map(|entry| entry.value);
                    ConfigKv::set(&full_key, &value, false)
                        .await
                        .map_err(|e| map_repo_store_error(&key, e))?;
                    previous
                }
            };
            if matches!(scope, Scope::Branch(_)) && (key == KEY_PROTECT || key == KEY_ARCHIVE) {
                // Recorded now, enforced by the future branch-policy layer
                // (lore.md 1.13) — do not let users assume enforcement exists.
                eprintln!("note: branch {key} is recorded but not yet enforced");
            }
            let report = MetadataOutput::Set {
                scope: scope.label(),
                target: scope.target(),
                key,
                value,
                value_type: value_type.as_str().to_string(),
                previous,
            };
            if output.is_json() {
                emit_json_data("metadata", &report, output)?;
            }
            Ok(())
        }
        MetadataCommand::Unset { key, scope } => {
            validate_key(&key).map_err(usage_error)?;
            let scope = resolve_scope(&scope).await?;
            let removed = match &scope {
                Scope::Branch(name) => MetadataKv::unset(MetadataScope::Branch, name, &key)
                    .await
                    .map_err(|e| {
                    CliError::fatal(format!("failed to remove metadata: {e}"))
                        .with_stable_code(StableErrorCode::IoWriteFailed)
                })?,
                Scope::Repo => {
                    let full_key = format!("{REPO_METADATA_PREFIX}{key}");
                    match ConfigKv::unset(&full_key).await {
                        Ok(rows) => rows > 0,
                        Err(e) => return Err(map_repo_store_error(&key, e)),
                    }
                }
            };
            let report = MetadataOutput::Unset {
                scope: scope.label(),
                target: scope.target(),
                key,
                removed,
            };
            if output.is_json() {
                emit_json_data("metadata", &report, output)?;
            }
            if !removed {
                return Err(CliError::silent_exit(1));
            }
            Ok(())
        }
        MetadataCommand::List { scope, prefix } => {
            let scope = resolve_scope(&scope).await?;
            let entries: Vec<MetadataListEntry> = match &scope {
                Scope::Branch(name) => {
                    MetadataKv::list(MetadataScope::Branch, name, prefix.as_deref())
                        .await
                        .map_err(|e| {
                            CliError::fatal(format!("failed to list metadata: {e}"))
                                .with_stable_code(StableErrorCode::IoReadFailed)
                        })?
                        .into_iter()
                        .map(|entry| MetadataListEntry {
                            key: entry.key,
                            value: entry.value,
                            value_type: entry.value_type,
                        })
                        .collect()
                }
                Scope::Repo => {
                    let namespace_prefix = format!(
                        "{REPO_METADATA_PREFIX}{}",
                        prefix.as_deref().unwrap_or_default()
                    );
                    ConfigKv::get_by_prefix(&namespace_prefix)
                        .await
                        .map_err(|e| map_repo_store_error("<list>", e))?
                        .into_iter()
                        .map(|entry| MetadataListEntry {
                            key: entry
                                .key
                                .strip_prefix(REPO_METADATA_PREFIX)
                                .unwrap_or(&entry.key)
                                .to_string(),
                            // Encrypted values render redacted, like `config list`.
                            value: if entry.encrypted {
                                "<REDACTED>".to_string()
                            } else {
                                entry.value
                            },
                            value_type: "text".to_string(),
                        })
                        .collect()
                }
            };
            let report = MetadataOutput::List {
                scope: scope.label(),
                target: scope.target(),
                entries,
            };
            if output.is_json() {
                return emit_json_data("metadata", &report, output);
            }
            if output.quiet {
                return Ok(());
            }
            if let MetadataOutput::List { entries, .. } = &report {
                for entry in entries {
                    println!("{}={}", entry.key, entry.value);
                }
            }
            Ok(())
        }
    }
}
