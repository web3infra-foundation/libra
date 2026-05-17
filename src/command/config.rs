//! Config command for reading and writing settings across scopes.
//!
//! Supports subcommand style (`libra config set/get/list/unset/import/path`)
//! and Git-compatible flag style (`--get`, `--list`, etc.).

use std::{io::IsTerminal, path::PathBuf, process::Command};

use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{
    internal::{
        config::{ConfigKv, ConfigKvEntry, is_sensitive_key, is_vault_internal_key},
        db::{create_database, establish_connection, get_db_conn_instance},
        vault::{
            decrypt_token, encrypt_token, generate_pgp_key, generate_ssh_key_pair,
            lazy_init_vault_for_scope, load_unseal_key_for_scope,
        },
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode, emit_warning},
        output::{OutputConfig, emit_json_data},
        pager::LIBRA_TEST_ENV,
        text::levenshtein,
        util::{DATABASE, try_get_storage_path},
    },
};

/// Cached database connection for Global scope, paired with the resolved DB path.
static GLOBAL_CONFIG_CONN: Lazy<Mutex<Option<(PathBuf, DatabaseConnection)>>> =
    Lazy::new(|| Mutex::new(None));

const EXAMPLES: &str = r#"EXAMPLES:
    libra config set user.name "John Doe"              Set local config value
    libra config get user.name                         Get value (cascade lookup)
    libra config list                                  List all local entries
    libra config list --show-origin                    List with scope labels
    libra config set --global user.email "j@x.com"     Set global config
    libra config unset user.signingkey                 Remove a key
    libra config import --global                       Import from Git global config
    libra config set vault.env.GEMINI_API_KEY          Store API key (interactive)
    echo "$SECRET" | libra config set --stdin vault.env.KEY  Set from stdin (CI/CD)
    libra config set --encrypt custom.key "value"      Force-encrypt a value
    libra config list --vault                          List vault env entries
    libra config list --name-only                      List all key names
    libra config path                                  Show config DB path"#;

/// Configuration scope that determines where values are stored and retrieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    /// Repository-specific (`.libra/libra.db`). Default for writes.
    Local,
    /// User-level (`~/.libra/config.db`).
    Global,
}

impl ConfigScope {
    /// Cascade order for reads (highest to lowest precedence).
    pub const CASCADE_ORDER: [ConfigScope; 2] = [ConfigScope::Local, ConfigScope::Global];

    /// Get the config database path for this scope.
    pub fn get_config_path(&self) -> Option<PathBuf> {
        match self {
            ConfigScope::Local => None,
            ConfigScope::Global => {
                if let Some(p) = std::env::var_os("LIBRA_CONFIG_GLOBAL_DB") {
                    return Some(PathBuf::from(p));
                }
                dirs::home_dir().map(|home| home.join(".libra").join("config.db"))
            }
        }
    }

    pub async fn ensure_config_exists(&self) -> Result<(), String> {
        match self {
            ConfigScope::Local => Ok(()),
            ConfigScope::Global => {
                if let Some(config_path) = self.get_config_path() {
                    if let Some(parent_dir) = config_path.parent()
                        && !parent_dir.exists()
                    {
                        std::fs::create_dir_all(parent_dir).map_err(|e| {
                            format!("Failed to create global config directory: {e}")
                        })?;
                    }
                    if !config_path.exists() {
                        let config_path_str = config_path.to_string_lossy();
                        create_database(&config_path_str)
                            .await
                            .map_err(|e| format!("Failed to create global config database: {e}"))?;
                    }
                    Ok(())
                } else {
                    Err(
                        "Could not determine global config path: home directory not available"
                            .to_string(),
                    )
                }
            }
        }
    }
}

/// Scoped config access layer — resolves the correct database for each scope.
pub struct ScopedConfig;

impl ScopedConfig {
    /// Get a database connection for the specified scope.
    pub async fn get_connection(scope: ConfigScope) -> Result<DatabaseConnection, String> {
        match scope {
            ConfigScope::Local => {
                let storage = try_get_storage_path(None).map_err(|_| {
                    "fatal: not a libra repository (or any of the parent directories): .libra"
                        .to_string()
                })?;
                let db_path = storage.join(DATABASE);
                if !db_path.exists() {
                    return Err(format!(
                        "fatal: libra database not found at '{}'",
                        db_path.display()
                    ));
                }
                Ok(get_db_conn_instance().await.clone())
            }
            ConfigScope::Global => {
                Self::get_or_create_cached_connection(&GLOBAL_CONFIG_CONN, scope, "global").await
            }
        }
    }

    async fn get_or_create_cached_connection(
        cache: &Lazy<Mutex<Option<(PathBuf, DatabaseConnection)>>>,
        scope: ConfigScope,
        scope_name: &str,
    ) -> Result<DatabaseConnection, String> {
        let Some(config_path) = scope.get_config_path() else {
            return Err(format!(
                "Could not determine config path for {scope_name} scope"
            ));
        };
        let mut guard = cache.lock().await;
        if let Some((cached_path, cached_conn)) = guard.as_ref() {
            if cached_path == &config_path {
                return Ok(cached_conn.clone());
            }
            *guard = None;
        }
        scope.ensure_config_exists().await?;
        let config_path_str = config_path.to_string_lossy();
        let conn = establish_connection(&config_path_str)
            .await
            .map_err(|e| format!("Failed to connect to {scope_name} config database: {e}"))?;
        *guard = Some((config_path, conn.clone()));
        Ok(conn)
    }

    // ── ConfigKv wrappers with scope ─────────────────────────────────

    pub async fn get(scope: ConfigScope, key: &str) -> Result<Option<ConfigKvEntry>, String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::get_with_conn(&conn, key)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_all(scope: ConfigScope, key: &str) -> Result<Vec<ConfigKvEntry>, String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::get_all_with_conn(&conn, key)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn set(
        scope: ConfigScope,
        key: &str,
        value: &str,
        encrypted: bool,
    ) -> Result<(), String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::set_with_conn(&conn, key, value, encrypted)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn add(
        scope: ConfigScope,
        key: &str,
        value: &str,
        encrypted: bool,
    ) -> Result<(), String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::add_with_conn(&conn, key, value, encrypted)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn unset(scope: ConfigScope, key: &str) -> Result<usize, String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::unset_with_conn(&conn, key)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn unset_all(scope: ConfigScope, key: &str) -> Result<usize, String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::unset_all_with_conn(&conn, key)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn list_all(scope: ConfigScope) -> Result<Vec<ConfigKvEntry>, String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::list_all_with_conn(&conn)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_by_prefix(
        scope: ConfigScope,
        prefix: &str,
    ) -> Result<Vec<ConfigKvEntry>, String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::get_by_prefix_with_conn(&conn, prefix)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_regexp(
        scope: ConfigScope,
        pattern: &str,
    ) -> Result<Vec<ConfigKvEntry>, String> {
        let conn = Self::get_connection(scope).await?;
        ConfigKv::get_regexp_with_conn(&conn, pattern)
            .await
            .map_err(|e| e.to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI argument definitions
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    about = "Manage repository configurations",
    after_help = EXAMPLES
)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,

    // ── Git-compat flags (hidden, translated to subcommands) ─────────
    /// Get a configuration value
    #[clap(long, hide = true)]
    pub get: bool,
    /// Get all values for a key
    #[clap(long("get-all"), hide = true)]
    pub get_all: bool,
    /// Remove a configuration entry
    #[clap(long, hide = true)]
    pub unset: bool,
    /// Remove all entries for a key
    #[clap(long("unset-all"), hide = true)]
    pub unset_all: bool,
    /// List all entries
    #[clap(long, short, hide = true)]
    pub list: bool,
    /// Add a value (allows duplicates)
    #[clap(long, hide = true)]
    pub add: bool,
    /// Import from Git config
    #[clap(long, hide = true)]
    pub import: bool,
    /// Get entries matching a regex
    #[clap(long("get-regexp"), hide = true)]
    pub get_regexp: bool,
    /// Show which scope each value comes from
    #[clap(long("show-origin"), hide = true)]
    pub show_origin: bool,

    // ── Scope flags ──────────────────────────────────────────────────
    /// Use repository config (default)
    #[clap(long, global = true, group("scope"))]
    pub local: bool,
    /// Use global user config
    #[clap(long, global = true, group("scope"))]
    pub global: bool,
    /// System scope (removed — always errors)
    #[clap(long, global = true, group("scope"))]
    pub system: bool,

    // ── Positional args (Git-compat mode) ────────────────────────────
    /// Configuration key
    #[clap(value_name = "key")]
    pub key: Option<String>,
    /// Value or value pattern
    #[clap(value_name = "value")]
    pub valuepattern: Option<String>,
    /// Default value when key not found
    #[clap(long, short = 'd')]
    pub default: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Set a configuration value
    Set {
        /// Configuration key (dotted format, e.g. user.name)
        key: String,
        /// Value to set (interactive input for sensitive keys if omitted)
        value: Option<String>,
        /// Add as additional value (allows duplicates)
        #[clap(long)]
        add: bool,
        /// Force vault encryption
        #[clap(long)]
        encrypt: bool,
        /// Force plaintext storage (skip auto-encryption)
        #[clap(long)]
        plaintext: bool,
        /// Read value from stdin
        #[clap(long)]
        stdin: bool,
    },
    /// Get a configuration value
    Get {
        /// Configuration key (or regex pattern with --regexp)
        key: String,
        /// Get all values for this key
        #[clap(long)]
        all: bool,
        /// Show actual value for encrypted entries
        #[clap(long)]
        reveal: bool,
        /// Treat key as regex pattern
        #[clap(long)]
        regexp: bool,
        /// Default value if key not found
        #[clap(long, short = 'd')]
        default: Option<String>,
    },
    /// List configuration entries
    List {
        /// Show only key names
        #[clap(long("name-only"))]
        name_only: bool,
        /// Show scope origin for each entry
        #[clap(long("show-origin"))]
        show_origin: bool,
        /// Show only vault.env.* entries
        #[clap(long)]
        vault: bool,
        /// Show SSH keys
        #[clap(long("ssh-keys"))]
        ssh_keys: bool,
        /// Show GPG keys
        #[clap(long("gpg-keys"))]
        gpg_keys: bool,
    },
    /// Remove a configuration entry
    Unset {
        /// Configuration key to remove
        key: String,
        /// Remove all values for this key
        #[clap(long)]
        all: bool,
    },
    /// Import configuration from Git
    Import,
    /// Show config database file path
    Path,
    /// Open config in editor (not supported — SQLite storage)
    Edit,
    /// Generate SSH key for a remote
    GenerateSshKey {
        /// Remote name to generate key for
        #[clap(long)]
        remote: String,
    },
    /// Generate GPG key for signing
    GenerateGpgKey {
        /// User name for the key
        #[clap(long)]
        name: Option<String>,
        /// User email for the key
        #[clap(long)]
        email: Option<String>,
        /// Key usage (signing or encrypt)
        #[clap(long)]
        usage: Option<String>,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Serializable output types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct ConfigListEntry {
    key: String,
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigImportSummary {
    scope: &'static str,
    imported: usize,
    skipped_duplicates: usize,
    ignored_invalid: usize,
    auto_encrypted: usize,
    collapsed_multivalue_warnings: usize,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigSshKeyEntry {
    remote: String,
    #[serde(rename = "type")]
    key_type: String,
    public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigGpgKeyEntry {
    usage: String,
    #[serde(rename = "type")]
    key_type: String,
    pubkey_config_key: String,
    signing_enabled: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry points
// ─────────────────────────────────────────────────────────────────────────────

/// Execute the `config` command, printing any error to stderr.
pub async fn execute(args: ConfigArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Safe entry point returning structured [`CliResult`].
pub async fn execute_safe(args: ConfigArgs, output: &OutputConfig) -> CliResult<()> {
    execute_inner(args, output).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch logic
// ─────────────────────────────────────────────────────────────────────────────

async fn execute_inner(args: ConfigArgs, output: &OutputConfig) -> CliResult<()> {
    // Reject --system early
    if args.system {
        return Err(CliError::from_legacy_string(
            "error: --system scope is not supported\n\nhint: use --local or --global",
        ));
    }

    let scope = get_scope(&args);
    let use_cascade = !has_explicit_scope(&args);

    // Resolve subcommand: either explicit or translated from Git-compat flags
    let cmd = resolve_command(&args)?;

    match cmd {
        ResolvedCommand::Set {
            key,
            value,
            add,
            encrypt,
            plaintext,
            stdin,
        } => {
            handle_set(
                &key,
                value.as_deref(),
                add,
                encrypt,
                plaintext,
                stdin,
                scope,
                output,
            )
            .await
        }
        ResolvedCommand::Get {
            key,
            all,
            reveal,
            regexp,
            default,
        } => {
            handle_get(
                &key,
                all,
                reveal,
                regexp,
                default.as_deref(),
                scope,
                use_cascade,
                output,
            )
            .await
        }
        ResolvedCommand::List {
            name_only,
            show_origin,
            vault,
            ssh_keys,
            gpg_keys,
        } => {
            handle_list(
                name_only,
                show_origin,
                vault,
                ssh_keys,
                gpg_keys,
                scope,
                use_cascade,
                output,
            )
            .await
        }
        ResolvedCommand::Unset { key, all } => handle_unset(&key, all, scope, output).await,
        ResolvedCommand::Import => handle_import(scope, output).await,
        ResolvedCommand::Path => handle_path(scope, output).await,
        ResolvedCommand::Edit => Err(CliError::from_legacy_string(
            "error: config edit is not supported (SQLite storage does not support text-based editing)\n\nhint: use libra config set/unset/list to manage configuration\nhint: use libra config list --name-only to see all keys",
        )),
        ResolvedCommand::GenerateSshKey { remote } => {
            handle_generate_ssh_key(&remote, scope, output).await
        }
        ResolvedCommand::GenerateGpgKey { name, email, usage } => {
            handle_generate_gpg_key(
                name.as_deref(),
                email.as_deref(),
                usage.as_deref(),
                scope,
                output,
            )
            .await
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Command resolution (subcommand ↔ flag translation)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum ResolvedCommand {
    Set {
        key: String,
        value: Option<String>,
        add: bool,
        encrypt: bool,
        plaintext: bool,
        stdin: bool,
    },
    Get {
        key: String,
        all: bool,
        reveal: bool,
        regexp: bool,
        default: Option<String>,
    },
    List {
        name_only: bool,
        show_origin: bool,
        vault: bool,
        ssh_keys: bool,
        gpg_keys: bool,
    },
    Unset {
        key: String,
        all: bool,
    },
    Import,
    Path,
    Edit,
    GenerateSshKey {
        remote: String,
    },
    GenerateGpgKey {
        name: Option<String>,
        email: Option<String>,
        usage: Option<String>,
    },
}

fn resolve_command(args: &ConfigArgs) -> CliResult<ResolvedCommand> {
    // If an explicit subcommand was provided, use it directly
    if let Some(ref cmd) = args.command {
        return Ok(match cmd {
            ConfigCommand::Set {
                key,
                value,
                add,
                encrypt,
                plaintext,
                stdin,
            } => ResolvedCommand::Set {
                key: key.clone(),
                value: value.clone(),
                add: *add,
                encrypt: *encrypt,
                plaintext: *plaintext,
                stdin: *stdin,
            },
            ConfigCommand::Get {
                key,
                all,
                reveal,
                regexp,
                default,
            } => ResolvedCommand::Get {
                key: key.clone(),
                all: *all,
                reveal: *reveal,
                regexp: *regexp,
                default: default.clone(),
            },
            ConfigCommand::List {
                name_only,
                show_origin,
                vault,
                ssh_keys,
                gpg_keys,
            } => ResolvedCommand::List {
                name_only: *name_only,
                show_origin: *show_origin,
                vault: *vault,
                ssh_keys: *ssh_keys,
                gpg_keys: *gpg_keys,
            },
            ConfigCommand::Unset { key, all } => ResolvedCommand::Unset {
                key: key.clone(),
                all: *all,
            },
            ConfigCommand::Import => ResolvedCommand::Import,
            ConfigCommand::Path => ResolvedCommand::Path,
            ConfigCommand::Edit => ResolvedCommand::Edit,
            ConfigCommand::GenerateSshKey { remote } => ResolvedCommand::GenerateSshKey {
                remote: remote.clone(),
            },
            ConfigCommand::GenerateGpgKey { name, email, usage } => {
                ResolvedCommand::GenerateGpgKey {
                    name: name.clone(),
                    email: email.clone(),
                    usage: usage.clone(),
                }
            }
        });
    }

    // Git-compat flag translation
    if args.list {
        return Ok(ResolvedCommand::List {
            name_only: false,
            show_origin: args.show_origin,
            vault: false,
            ssh_keys: false,
            gpg_keys: false,
        });
    }
    if args.import || args.key.as_deref() == Some("import") {
        if args.import && args.key.is_some() {
            return Err(CliError::from_legacy_string(
                "error: `libra config --import` does not accept <key>",
            ));
        }
        return Ok(ResolvedCommand::Import);
    }

    // Check for "edit" positional
    if args.key.as_deref() == Some("edit") {
        return Ok(ResolvedCommand::Edit);
    }
    // Check for "path" positional
    if args.key.as_deref() == Some("path") {
        return Ok(ResolvedCommand::Path);
    }

    // All remaining modes need a key
    let key = args.key.as_deref().ok_or_else(|| {
        CliError::from_legacy_string("error: missing required argument: <key>").with_exit_code(2)
    })?;

    // Validate key format (must contain at least one dot)
    if !key.contains('.') {
        let mut msg = format!("error: key does not contain a section: {key}");
        if key == "init" || key == "clone" {
            msg.push_str(&format!(
                "\n\nhint: `{key}` is a top-level command. Try `libra {key}`."
            ));
        }
        return Err(CliError::from_legacy_string(msg).with_exit_code(1));
    }

    // --default (-d) is only valid with --get, --get-all, or get-regexp
    if args.default.is_some() && !args.get && !args.get_all && !args.get_regexp {
        return Err(CliError::from_legacy_string(
            "error: --default (-d) can only be used with --get, --get-all, or --get-regexp",
        )
        .with_exit_code(2));
    }

    if args.get_regexp {
        return Ok(ResolvedCommand::Get {
            key: key.to_string(),
            all: false,
            reveal: false,
            regexp: true,
            default: args.default.clone(),
        });
    }
    if args.get {
        return Ok(ResolvedCommand::Get {
            key: key.to_string(),
            all: false,
            reveal: false,
            regexp: false,
            default: args.default.clone(),
        });
    }
    if args.get_all {
        return Ok(ResolvedCommand::Get {
            key: key.to_string(),
            all: true,
            reveal: false,
            regexp: false,
            default: args.default.clone(),
        });
    }
    if args.unset {
        return Ok(ResolvedCommand::Unset {
            key: key.to_string(),
            all: false,
        });
    }
    if args.unset_all {
        return Ok(ResolvedCommand::Unset {
            key: key.to_string(),
            all: true,
        });
    }
    if args.add {
        let value = args.valuepattern.as_deref().ok_or_else(|| {
            CliError::from_legacy_string("error: missing required argument: <value>")
                .with_exit_code(2)
        })?;
        return Ok(ResolvedCommand::Set {
            key: key.to_string(),
            value: Some(value.to_string()),
            add: true,
            encrypt: false,
            plaintext: false,
            stdin: false,
        });
    }

    // Default: set mode (key + optional value).
    // When value is omitted, handle_set will trigger interactive input for
    // sensitive keys or report a missing-value error for ordinary keys.
    Ok(ResolvedCommand::Set {
        key: key.to_string(),
        value: args.valuepattern.clone(),
        add: false,
        encrypt: false,
        plaintext: false,
        stdin: false,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler implementations
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_set(
    key: &str,
    value: Option<&str>,
    add: bool,
    encrypt: bool,
    plaintext: bool,
    stdin: bool,
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    // Validate key format
    if !key.contains('.') {
        return Err(CliError::from_legacy_string(format!(
            "error: key does not contain a section: {key}"
        ))
        .with_exit_code(1));
    }

    // --encrypt and --plaintext are mutually exclusive
    if encrypt && plaintext {
        return Err(CliError::from_legacy_string(
            "error: --encrypt and --plaintext are mutually exclusive",
        ));
    }

    // --plaintext must not be used with vault internal/secret keys
    if plaintext && (is_vault_internal_key(key) || key.starts_with("vault.env.")) {
        return Err(CliError::from_legacy_string(
            "error: --plaintext cannot be used with vault internal/secret keys",
        ));
    }

    // Check encryption state inheritance from existing entries.
    let existing_entries = ScopedConfig::get_all(scope, key).await.map_err(|e| {
        config_read_cli_error(format!(
            "failed to read {} config while checking existing values for key '{}': {e}",
            scope_name(scope),
            key
        ))
    })?;
    let has_encrypted = existing_entries.iter().any(|e| e.encrypted);
    let has_plaintext = existing_entries.iter().any(|e| !e.encrypted);

    // Resolve the value
    let resolved_value = if stdin {
        if value.is_some() {
            return Err(CliError::from_legacy_string(
                "error: cannot use both value argument and --stdin",
            ));
        }
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).map_err(|e| {
            CliError::from_legacy_string(format!("error: failed to read from stdin: {e}"))
        })?;
        // Strip trailing newline (like Git)
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
        buf
    } else if let Some(v) = value {
        v.to_string()
    } else {
        // No value provided
        let needs_protected_input =
            !plaintext && (encrypt || is_sensitive_key(key) || has_encrypted);

        if needs_protected_input {
            // Check if interactive mode is available.
            // Also treat the test harness (`LIBRA_TEST=1`) as non-interactive
            // so that `rpassword::read_password()` never blocks a test run.
            let in_test = std::env::var_os(LIBRA_TEST_ENV).is_some();
            if output.is_json() || in_test || !std::io::stdin().is_terminal() {
                return Err(CliError::from_legacy_string(format!(
                    "error: missing value for protected key '{key}' (non-interactive environment)"
                ))
                .with_exit_code(2));
            }
            // Interactive secure input (no echo)
            eprint!("Enter value for {key}: ");
            rpassword::read_password().map_err(|e| {
                CliError::from_legacy_string(format!("error: failed to read input: {e}"))
            })?
        } else {
            return Err(CliError::from_legacy_string(format!(
                "error: missing value for key '{key}'"
            ))
            .with_exit_code(2));
        }
    };

    // Determine encryption
    let should_encrypt = if encrypt {
        true
    } else if plaintext {
        false
    } else if has_encrypted {
        true // Inherit encryption from existing entries
    } else {
        is_sensitive_key(key)
    };

    // Same-key-same-state constraint for --add.
    if add && ((should_encrypt && has_plaintext) || (!should_encrypt && has_encrypted)) {
        return Err(CliError::from_legacy_string(
            "error: cannot mix encrypted and plaintext values for the same key",
        ));
    }

    // Encrypt the value if needed
    let store_value = if should_encrypt {
        let sn = scope_name(scope);
        let unseal_key = match load_unseal_key_for_scope(sn).await {
            Some(key) => key,
            None => {
                // Lazy init
                let key = lazy_init_vault_for_scope(sn).await.map_err(|e| {
                    CliError::from_legacy_string(format!(
                        "error: failed to initialize vault for {sn} scope: {e}"
                    ))
                })?;
                if !output.quiet && !output.is_json() {
                    println!("Initialized vault for {sn} scope");
                }
                key
            }
        };
        let ciphertext = encrypt_token(&unseal_key, resolved_value.as_bytes())
            .map_err(|e| CliError::from_legacy_string(format!("error: encryption failed: {e}")))?;
        hex::encode(ciphertext)
    } else {
        resolved_value.clone()
    };

    if add {
        ScopedConfig::add(scope, key, &store_value, should_encrypt)
            .await
            .map_err(CliError::from_legacy_string)?;
        emit_set_ack("add", scope, key, should_encrypt, output)?;
    } else {
        ScopedConfig::set(scope, key, &store_value, should_encrypt)
            .await
            .map_err(|e| {
                let err = CliError::from_legacy_string(&e);
                if e.contains("values exist") {
                    err.with_exit_code(5)
                } else {
                    err
                }
            })?;
        emit_set_ack("set", scope, key, should_encrypt, output)?;
    }
    Ok(())
}

/// Decrypt a hex-encoded ciphertext from a config value using the vault unseal key.
/// The `scope` parameter determines which unseal key to load (local or global).
async fn decrypt_config_value(hex_value: &str, scope: &str) -> Result<String, String> {
    let unseal_key = load_unseal_key_for_scope(scope)
        .await
        .ok_or_else(|| format!("vault not initialized for {scope} scope — cannot decrypt"))?;
    let ciphertext =
        hex::decode(hex_value).map_err(|e| format!("failed to decode encrypted value: {e}"))?;
    decrypt_token(&unseal_key, &ciphertext).map_err(|e| format!("decryption failed: {e}"))
}

fn config_read_cli_error(message: impl Into<String>) -> CliError {
    CliError::fatal(message)
        .with_stable_code(StableErrorCode::IoReadFailed)
        .with_exit_code(128)
}

fn config_decrypt_cli_error(key: &str, scope_label: &str, error: impl Into<String>) -> CliError {
    CliError::fatal(format!(
        "failed to decrypt value for key '{key}' from {scope_label} config: {}",
        error.into()
    ))
    .with_stable_code(StableErrorCode::RepoStateInvalid)
    .with_exit_code(128)
}

async fn render_get_value(
    entry: &ConfigKvEntry,
    reveal: bool,
    scope: ConfigScope,
    _use_cascade: bool,
) -> CliResult<String> {
    if !entry.encrypted {
        return Ok(entry.value.clone());
    }

    if !reveal || is_vault_internal_key(&entry.key) {
        return Ok("<REDACTED>".to_string());
    }

    let scope_label = scope_name(scope);
    let decrypted = decrypt_config_value(&entry.value, scope_label)
        .await
        .map_err(|e| config_decrypt_cli_error(&entry.key, scope_label, e))?;

    Ok(decrypted)
}

#[allow(clippy::too_many_arguments)]
async fn handle_get(
    key: &str,
    all: bool,
    reveal: bool,
    regexp: bool,
    default: Option<&str>,
    scope: ConfigScope,
    use_cascade: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    // Block --reveal for vault internal keys on exact-key queries
    if reveal && !regexp && !all && is_vault_internal_key(key) {
        return Err(CliError::from_legacy_string(format!(
            "error: key '{}' is a vault internal credential and cannot be revealed",
            key
        )));
    }

    if regexp {
        // Regex search across all keys
        let entries: Vec<(ConfigKvEntry, ConfigScope)> = if use_cascade {
            let mut all_entries = Vec::new();
            for s in ConfigScope::CASCADE_ORDER {
                if s != ConfigScope::Local {
                    let Some(path) = s.get_config_path() else {
                        continue;
                    };
                    if !path.exists() {
                        continue;
                    }
                }
                let scope_entries = ScopedConfig::get_regexp(s, key).await.map_err(|e| {
                    config_read_cli_error(format!("failed to read {} config: {e}", scope_name(s)))
                })?;
                for e in scope_entries {
                    all_entries.push((e, s));
                }
            }
            all_entries
        } else {
            ScopedConfig::get_regexp(scope, key)
                .await
                .map_err(CliError::from_legacy_string)?
                .into_iter()
                .map(|e| (e, scope))
                .collect()
        };

        // Build display values with decryption support
        let mut display_entries = Vec::new();
        for (e, s) in &entries {
            let val = render_get_value(e, reveal, *s, use_cascade).await?;
            display_entries.push((e, s, val));
        }

        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "get-regexp",
                    "pattern": key,
                    "entries": display_entries.iter().map(|(e, s, val)| serde_json::json!({
                        "key": e.key,
                        "value": val,
                        "origin": scope_name(**s),
                        "encrypted": e.encrypted,
                    })).collect::<Vec<_>>(),
                }),
                output,
            )?;
        } else if !output.quiet {
            for (e, _, val) in &display_entries {
                println!("{} = {val}", e.key);
            }
        }
        return Ok(());
    }

    if all {
        // Get all values for a specific key
        let entries: Vec<(ConfigKvEntry, ConfigScope)> = if use_cascade {
            get_all_cascaded(key).await.map_err(config_read_cli_error)?
        } else {
            ScopedConfig::get_all(scope, key)
                .await
                .map_err(CliError::from_legacy_string)?
                .into_iter()
                .map(|e| (e, scope))
                .collect()
        };

        if entries.is_empty()
            && let Some(d) = default
        {
            if output.is_json() {
                emit_json_data(
                    "config",
                    &serde_json::json!({
                        "action": "get-all",
                        "key": key,
                        "entries": [{"value": d, "origin": serde_json::Value::Null}],
                        "default_applied": true,
                    }),
                    output,
                )?;
            } else if !output.quiet {
                println!("{d}");
            }
            return Ok(());
        }

        // Build display values with decryption support
        let mut display_entries = Vec::new();
        for (e, s) in &entries {
            let val = render_get_value(e, reveal, *s, use_cascade).await?;
            display_entries.push((e, s, val));
        }

        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "get-all",
                    "key": key,
                    "entries": display_entries.iter().map(|(e, s, val)| serde_json::json!({
                        "value": val,
                        "origin": scope_name(**s),
                        "encrypted": e.encrypted,
                    })).collect::<Vec<_>>(),
                    "default_applied": false,
                }),
                output,
            )?;
        } else if !output.quiet {
            for (_, _, val) in &display_entries {
                println!("{val}");
            }
        }
    } else {
        // Get single value (last-one-wins)
        let entry: Option<(ConfigKvEntry, ConfigScope)> = if use_cascade {
            get_cascaded(key).await.map_err(config_read_cli_error)?
        } else {
            ScopedConfig::get(scope, key)
                .await
                .map_err(CliError::from_legacy_string)?
                .map(|e| (e, scope))
        };

        let (display_value, default_applied, origin_scope) = match entry {
            Some((ref e, s)) => {
                let val = render_get_value(e, reveal, s, use_cascade).await?;
                (val, false, Some(s))
            }
            None => {
                if let Some(d) = default {
                    (d.to_string(), true, None)
                } else {
                    // Spell correction: find closest matching key
                    let all_keys = if use_cascade {
                        let mut keys = Vec::new();
                        for s in ConfigScope::CASCADE_ORDER {
                            if s != ConfigScope::Local {
                                let Some(path) = s.get_config_path() else {
                                    continue;
                                };
                                if !path.exists() {
                                    continue;
                                }
                            }
                            if let Ok(entries) = ScopedConfig::list_all(s).await {
                                for e in entries {
                                    if !keys.contains(&e.key) {
                                        keys.push(e.key);
                                    }
                                }
                            }
                        }
                        keys
                    } else {
                        ScopedConfig::list_all(scope)
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .map(|e| e.key)
                            .collect()
                    };

                    let mut best_match = None;
                    let mut best_dist = usize::MAX;
                    for k in &all_keys {
                        let dist = levenshtein(key, k);
                        if dist < best_dist && dist <= 3 {
                            best_dist = dist;
                            best_match = Some(k.clone());
                        }
                    }

                    let mut msg = format!("key '{key}' not found in any scope");
                    if let Some(suggestion) = best_match {
                        msg.push_str(&format!("\n\nhint: did you mean '{suggestion}'?"));
                    }
                    msg.push_str("\nhint: use libra config list to see all configured keys");
                    return Err(CliError::failure(msg)
                        .with_stable_code(StableErrorCode::CliInvalidArguments)
                        .with_exit_code(1));
                }
            }
        };

        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "get",
                    "key": key,
                    "value": display_value,
                    "origin": origin_scope.map(scope_name),
                    "default_applied": default_applied,
                }),
                output,
            )?;
        } else if !output.quiet {
            println!("{display_value}");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_list(
    name_only: bool,
    show_origin: bool,
    vault: bool,
    ssh_keys: bool,
    gpg_keys: bool,
    scope: ConfigScope,
    use_cascade: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    if ssh_keys {
        let entries = list_ssh_key_entries(scope).await?;
        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "list-ssh-keys",
                    "keys": entries,
                    "count": entries.len(),
                }),
                output,
            )?;
        } else if !output.quiet {
            if entries.is_empty() {
                println!("No SSH keys configured.");
            } else {
                println!("SSH keys:");
                for entry in &entries {
                    println!("  {:<10} {}", entry.remote, entry.public_key);
                }
                println!();
                println!("{} keys configured", entries.len());
                println!();
                println!("Tip: use libra config generate-ssh-key --remote <name> to add more");
            }
        }
        return Ok(());
    }

    if gpg_keys {
        let entries = list_gpg_key_entries(scope).await?;
        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "list-gpg-keys",
                    "keys": entries,
                    "count": entries.len(),
                }),
                output,
            )?;
        } else if !output.quiet {
            if entries.is_empty() {
                println!("No GPG keys configured.");
            } else {
                println!("GPG keys:");
                for entry in &entries {
                    let signing_suffix = if entry.usage == "signing" && entry.signing_enabled {
                        "  (vault.signing = true)"
                    } else {
                        ""
                    };
                    println!(
                        "  {:<10} {}{}",
                        entry.usage, entry.pubkey_config_key, signing_suffix
                    );
                }
                println!();
                println!("{} keys configured", entries.len());
            }
        }
        return Ok(());
    }

    if vault {
        // List vault.env.* entries across scopes
        let mut entries = Vec::new();
        for s in ConfigScope::CASCADE_ORDER {
            if s != ConfigScope::Local {
                let Some(path) = s.get_config_path() else {
                    continue;
                };
                if !path.exists() {
                    continue;
                }
            }
            if let Ok(scope_entries) = ScopedConfig::get_by_prefix(s, "vault.env.").await {
                for e in scope_entries {
                    let plaintext_warning = if !e.encrypted && is_sensitive_key(&e.key) {
                        " [PLAINTEXT]"
                    } else {
                        ""
                    };
                    entries.push(ConfigListEntry {
                        key: e.key,
                        value: Some(if e.encrypted {
                            "<REDACTED>".to_string()
                        } else {
                            format!("{}{plaintext_warning}", e.value)
                        }),
                        origin: Some(scope_name(s).to_string()),
                        encrypted: Some(e.encrypted),
                    });
                }
            }
        }

        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "list-vault",
                    "entries": entries,
                    "encrypted_count": entries.len(),
                }),
                output,
            )?;
        } else if !output.quiet {
            if entries.is_empty() {
                println!("No vault environment variables configured.");
            } else {
                println!("Vault environment variables (cascade):");
                for e in &entries {
                    let origin = e.origin.as_deref().unwrap_or("?");
                    let val = e.value.as_deref().unwrap_or("");
                    println!("  {:<8} {} = {}  (encrypted)", origin, e.key, val);
                }
                println!("\n{} encrypted entries", entries.len());
                println!("\nNext steps:");
                println!("  - add:     libra config set vault.env.<ENV_VAR_NAME>");
                println!("  - remove:  libra config unset vault.env.<name>");
            }
        }
        return Ok(());
    }

    if show_origin {
        // Show all entries with scope labels
        let mut entries = Vec::new();
        for s in ConfigScope::CASCADE_ORDER {
            if s != ConfigScope::Local {
                let Some(path) = s.get_config_path() else {
                    continue;
                };
                if !path.exists() {
                    continue;
                }
            }
            if let Ok(scope_entries) = ScopedConfig::list_all(s).await {
                for e in scope_entries {
                    let plaintext_warning = if !e.encrypted && is_sensitive_key(&e.key) {
                        " [PLAINTEXT]"
                    } else {
                        ""
                    };
                    entries.push(ConfigListEntry {
                        key: e.key.clone(),
                        value: if name_only {
                            None
                        } else if e.encrypted {
                            Some("<REDACTED>".to_string())
                        } else {
                            Some(format!("{}{plaintext_warning}", e.value))
                        },
                        origin: if show_origin {
                            Some(scope_name(s).to_string())
                        } else {
                            None
                        },
                        encrypted: Some(e.encrypted),
                    });
                }
            }
        }

        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "list",
                    "scope": if show_origin { "all" } else { scope_name(scope) },
                    "cascade": use_cascade,
                    "entries": entries,
                    "count": entries.len(),
                }),
                output,
            )?;
        } else if !output.quiet {
            for e in &entries {
                match (&e.origin, &e.value) {
                    (Some(origin), Some(val)) => println!("  {:<8} {} = {val}", origin, e.key),
                    (Some(origin), None) => println!("  {:<8} {}", origin, e.key),
                    (None, Some(val)) => println!("{}={val}", e.key),
                    (None, None) => println!("{}", e.key),
                }
            }
        }
    } else {
        // Single scope list
        let scope_entries = ScopedConfig::list_all(scope)
            .await
            .map_err(CliError::from_legacy_string)?;

        let entries: Vec<ConfigListEntry> = scope_entries
            .into_iter()
            .map(|e| {
                let plaintext_warning = if !e.encrypted && is_sensitive_key(&e.key) {
                    " [PLAINTEXT]"
                } else {
                    ""
                };
                ConfigListEntry {
                    key: e.key.clone(),
                    value: if name_only {
                        None
                    } else if e.encrypted {
                        Some("<REDACTED>".to_string())
                    } else {
                        Some(format!("{}{plaintext_warning}", e.value))
                    },
                    origin: None,
                    encrypted: Some(e.encrypted),
                }
            })
            .collect();

        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "list",
                    "scope": scope_name(scope),
                    "entries": entries,
                    "count": entries.len(),
                }),
                output,
            )?;
        } else if !output.quiet {
            for e in &entries {
                match &e.value {
                    Some(val) => println!("{}={val}", e.key),
                    None => println!("{}", e.key),
                }
            }
        }
    }
    Ok(())
}

async fn list_ssh_key_entries(scope: ConfigScope) -> CliResult<Vec<ConfigSshKeyEntry>> {
    let mut entries = ScopedConfig::get_by_prefix(scope, "vault.ssh.")
        .await
        .map_err(CliError::from_legacy_string)?
        .into_iter()
        .filter_map(|entry| {
            let remote = entry
                .key
                .strip_prefix("vault.ssh.")?
                .strip_suffix(".pubkey")?;
            let mut parts = entry.value.split_whitespace();
            let key_type = parts.next().unwrap_or("ssh").to_string();
            let _material = parts.next()?;
            let key_id = parts.collect::<Vec<_>>().join(" ");
            Some(ConfigSshKeyEntry {
                remote: remote.to_string(),
                key_type,
                public_key: entry.value,
                key_id: (!key_id.is_empty()).then_some(key_id),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.remote.cmp(&right.remote));
    Ok(entries)
}

async fn list_gpg_key_entries(scope: ConfigScope) -> CliResult<Vec<ConfigGpgKeyEntry>> {
    let mut entries = ScopedConfig::list_all(scope)
        .await
        .map_err(CliError::from_legacy_string)?
        .into_iter()
        .filter_map(|entry| {
            let usage = match entry.key.as_str() {
                "vault.gpg.pubkey" | "vault.gpg_pubkey" => "signing".to_string(),
                key if key.starts_with("vault.gpg.") && key.ends_with(".pubkey") => key
                    .strip_prefix("vault.gpg.")?
                    .strip_suffix(".pubkey")?
                    .to_string(),
                _ => return None,
            };
            Some((usage, entry.key))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries.dedup_by(|left, right| left.0 == right.0);

    let signing_enabled = ScopedConfig::get(scope, "vault.signing")
        .await
        .map_err(CliError::from_legacy_string)?
        .map(|entry| entry.value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    Ok(entries
        .into_iter()
        .map(|(usage, pubkey_config_key)| ConfigGpgKeyEntry {
            signing_enabled: usage == "signing" && signing_enabled,
            usage,
            key_type: "PGP 2048".to_string(),
            pubkey_config_key,
        })
        .collect())
}

async fn handle_unset(
    key: &str,
    all: bool,
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    let count = if all {
        ScopedConfig::unset_all(scope, key)
            .await
            .map_err(CliError::from_legacy_string)?
    } else {
        ScopedConfig::unset(scope, key).await.map_err(|e| {
            let err = CliError::from_legacy_string(&e);
            if e.contains("values exist") {
                err.with_exit_code(5)
            } else {
                err
            }
        })?
    };

    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": if all { "unset-all" } else { "unset" },
                "scope": scope_name(scope),
                "key": key,
                "removed_count": count,
            }),
            output,
        )?;
    } else if !output.quiet {
        if all && count > 1 {
            println!(
                "Unset {}: {} (removed {} values)",
                scope_name(scope),
                key,
                count
            );
        } else {
            println!("Unset {}: {}", scope_name(scope), key);
        }
    }
    Ok(())
}

async fn handle_import(scope: ConfigScope, output: &OutputConfig) -> CliResult<()> {
    let summary = import_git_config(scope)
        .await
        .map_err(CliError::from_legacy_string)?;

    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": "import",
                "source": format!("git-{}", summary.scope),
                "target_scope": summary.scope,
                "imported": summary.imported,
                "skipped_duplicates": summary.skipped_duplicates,
                "auto_encrypted": summary.auto_encrypted,
                "collapsed_multivalue_warnings": summary.collapsed_multivalue_warnings,
                "ignored_invalid": summary.ignored_invalid,
            }),
            output,
        )?;
    } else if !output.quiet {
        print_import_summary(&summary);
    }
    Ok(())
}

async fn handle_path(scope: ConfigScope, output: &OutputConfig) -> CliResult<()> {
    let path = match scope {
        ConfigScope::Local => {
            let storage = try_get_storage_path(None).map_err(|_| {
                CliError::from_legacy_string(
                    "error: not a libra repository (or any parent up to /)\n\nhint: use --global to read/write user-level config without a repository\nhint: use libra init to create a repository here",
                )
            })?;
            storage.join(DATABASE)
        }
        ConfigScope::Global => scope.get_config_path().ok_or_else(|| {
            CliError::from_legacy_string("error: could not determine global config path")
        })?,
    };

    let exists = path.exists();

    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": "path",
                "scope": scope_name(scope),
                "path": path.to_string_lossy(),
                "exists": exists,
            }),
            output,
        )?;
    } else if !output.quiet {
        println!("{}", path.display());
    }
    Ok(())
}

async fn handle_generate_ssh_key(
    remote: &str,
    _scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    // Validate remote name
    if !remote
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || remote.is_empty()
        || remote.len() > 64
    {
        return Err(CliError::from_legacy_string(format!(
            "error: invalid remote name '{remote}': only [a-zA-Z0-9_-] allowed, 1-64 chars"
        )));
    }

    // Verify remote exists
    let remote_exists = ConfigKv::remote_config(remote)
        .await
        .map_err(|e| CliError::from_legacy_string(e.to_string()))?;
    if remote_exists.is_none() {
        return Err(CliError::from_legacy_string(format!(
            "error: remote '{remote}' not found, add it first with libra remote add"
        )));
    }

    // Get vault root dir and unseal key
    let storage = try_get_storage_path(None)
        .map_err(|_| CliError::from_legacy_string("error: not a libra repository"))?;

    let unseal_key = match load_unseal_key_for_scope("local").await {
        Some(key) => key,
        None => {
            let key = lazy_init_vault_for_scope("local").await.map_err(|e| {
                CliError::from_legacy_string(format!(
                    "error: failed to initialize vault for local scope: {e}"
                ))
            })?;
            if !output.quiet {
                println!("Initialized vault for local scope");
            }
            key
        }
    };

    // Get user name for key ID
    let user_name = ConfigKv::get("user.name")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .unwrap_or_else(|| "Libra User".to_string());

    // Generate key pair via vault (returns both pub and priv)
    let (public_key, private_key) = generate_ssh_key_pair(&storage, &unseal_key, &user_name)
        .await
        .map_err(|e| {
            CliError::from_legacy_string(format!("error: SSH key generation failed: {e}"))
        })?;

    // Store public key plaintext in config_kv
    let pubkey_key = format!("vault.ssh.{remote}.pubkey");
    let _ = ConfigKv::set(&pubkey_key, &public_key, false).await;

    // Store private key encrypted in config_kv (vault-backed, no persistent file)
    let privkey_key = format!("vault.ssh.{remote}.privkey");
    let encrypted_privkey = encrypt_token(&unseal_key, private_key.as_bytes()).map_err(|e| {
        CliError::from_legacy_string(format!("error: failed to encrypt SSH private key: {e}"))
    })?;
    let _ = ConfigKv::set(&privkey_key, &hex::encode(encrypted_privkey), true).await;

    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": "generate-ssh-key",
                "remote": remote,
                "type": "RSA",
                "bits": 3072,
                "public_key": public_key,
                "pubkey_config_key": pubkey_key,
                "privkey_config_key": privkey_key,
                "storage": "vault-encrypted",
            }),
            output,
        )?;
    } else if !output.quiet {
        println!("Generated SSH key for remote '{remote}':");
        println!("  Type:       RSA 3072");
        println!("  Public key: {public_key}");
        println!();
        println!("Stored:");
        println!("  public key:  {pubkey_key} (in config)");
        println!("  private key: {privkey_key} (vault-encrypted, temp file on use)");
        println!();
        println!("Next steps:");
        println!("  - add to GitHub:  copy the public key above to your GitHub SSH settings");
        println!("  - push:           libra push {remote} main");
    }
    Ok(())
}

async fn handle_generate_gpg_key(
    name: Option<&str>,
    email: Option<&str>,
    usage: Option<&str>,
    _scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    let usage = match usage.unwrap_or("signing") {
        "signing" => "signing",
        "encrypt" => "encrypt",
        other => {
            return Err(CliError::from_legacy_string(format!(
                "error: invalid value '{other}' for '--usage <USAGE>' (expected 'signing' or 'encrypt')"
            )));
        }
    };
    let is_signing = usage == "signing";

    let storage = try_get_storage_path(None)
        .map_err(|_| CliError::from_legacy_string("error: not a libra repository"))?;

    let unseal_key = match load_unseal_key_for_scope("local").await {
        Some(key) => key,
        None => {
            let key = lazy_init_vault_for_scope("local").await.map_err(|e| {
                CliError::from_legacy_string(format!(
                    "error: failed to initialize vault for local scope: {e}"
                ))
            })?;
            if !output.quiet {
                println!("Initialized vault for local scope");
            }
            key
        }
    };

    let user_name = name
        .map(String::from)
        .unwrap_or_else(|| "Libra User".to_string());

    let user_email = email
        .map(String::from)
        .unwrap_or_else(|| "user@libra.local".to_string());

    let public_key = generate_pgp_key(&storage, &unseal_key, &user_name, &user_email)
        .await
        .map_err(|e| {
            CliError::from_legacy_string(format!("error: GPG key generation failed: {e}"))
        })?;

    // Store pubkey under usage-specific dotted key
    let pubkey_config_key = if is_signing {
        "vault.gpg.pubkey".to_string()
    } else {
        format!("vault.gpg.{usage}.pubkey")
    };
    let _ = ConfigKv::set(&pubkey_config_key, &public_key, false).await;

    // Only enable vault.signing for signing usage
    if is_signing {
        let _ = ConfigKv::set("vault.signing", "true", false).await;
    }

    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": "generate-gpg-key",
                "usage": usage,
                "type": "PGP",
                "bits": 2048,
                "user": format!("{user_name} <{user_email}>"),
                "pubkey_config_key": pubkey_config_key,
                "signing_enabled": is_signing,
            }),
            output,
        )?;
    } else if !output.quiet {
        if is_signing {
            println!("Generated GPG key:");
        } else {
            println!("Generated GPG key (usage: {usage}):");
        }
        println!("  Type:    PGP 2048-bit");
        println!("  User:    {user_name} <{user_email}>");
        println!("  Valid:   10 years");
        println!();
        println!("Stored:");
        println!("  public key: {pubkey_config_key} (in config)");
        if is_signing {
            println!();
            println!("Tip: commit signing is now enabled (vault.signing = true)");
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Import from Git
// ─────────────────────────────────────────────────────────────────────────────

/// Known multi-value keys that should use --add semantics during import.
const KNOWN_MULTI_VALUE_PREFIXES: &[&str] = &[
    "remote.", // remote.*.fetch, remote.*.push, remote.*.pushurl
    "branch.", // branch.*.merge
    "url.",    // url.*.insteadOf, url.*.pushInsteadOf
    "http.",   // http.*.extraHeader
];

const KNOWN_MULTI_VALUE_KEYS: &[&str] = &["credential.helper"];

fn is_known_multi_value_key(key: &str) -> bool {
    if KNOWN_MULTI_VALUE_KEYS.contains(&key) {
        return true;
    }
    for prefix in KNOWN_MULTI_VALUE_PREFIXES {
        if let Some(suffix) = key.strip_prefix(prefix)
            && let Some((_name, leaf)) = suffix.rsplit_once('.')
            && matches!(
                leaf,
                "fetch"
                    | "push"
                    | "pushurl"
                    | "merge"
                    | "insteadOf"
                    | "pushInsteadOf"
                    | "extraHeader"
            )
        {
            return true;
        }
    }
    false
}

async fn import_git_config(scope: ConfigScope) -> Result<ConfigImportSummary, String> {
    let git_flag = match scope {
        ConfigScope::Local => "--local",
        ConfigScope::Global => "--global",
    };

    let mut git_args = vec!["config", git_flag, "--list", "-z"];
    if scope == ConfigScope::Global {
        git_args.push("--no-includes");
    }

    let output = Command::new("git")
        .args(&git_args)
        .output()
        .map_err(|e| format!("failed to run `git config`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let scope_label = scope_name(scope);
        let mut msg = format!("error: failed to import Git {scope_label} config");
        if !stderr.is_empty() {
            let detail = stderr.strip_prefix("fatal: ").unwrap_or(&stderr);
            msg.push_str(&format!("\n  {detail}"));
        }
        if scope == ConfigScope::Local {
            msg.push_str("\n\nhint: Run this command inside a Git repository, or use `--global`.");
        }
        return Err(msg);
    }

    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut ignored_invalid = 0usize;
    let mut auto_encrypted = 0usize;
    let mut collapsed_warnings = 0usize;

    // Track multi-value collapse for non-known keys
    let mut last_value_wins: std::collections::HashMap<String, (String, usize)> =
        std::collections::HashMap::new();

    // First pass: collect all entries
    let mut all_entries: Vec<(String, String)> = Vec::new();
    for entry in output
        .stdout
        .split(|b| *b == 0)
        .filter(|chunk| !chunk.is_empty())
    {
        let raw = String::from_utf8_lossy(entry);
        let (key_raw, value) = match raw.split_once('\n') {
            Some((k, v)) => (k.trim().to_string(), v.to_string()),
            None => {
                // Implicit boolean value
                let trimmed = raw.trim().to_string();
                if trimmed.contains('.') {
                    (trimmed, "true".to_string())
                } else {
                    ignored_invalid += 1;
                    continue;
                }
            }
        };

        // Validate key format
        if !key_raw.contains('.') {
            ignored_invalid += 1;
            continue;
        }
        all_entries.push((key_raw, value));
    }

    // Process entries
    for (key, value) in &all_entries {
        if is_known_multi_value_key(key) {
            // Multi-value: use add semantics, skip exact duplicates
            let existing = ScopedConfig::get_all(scope, key).await?;
            if existing.iter().any(|e| &e.value == value) {
                skipped += 1;
                continue;
            }
            let should_encrypt = is_sensitive_key(key);
            let store_value = if should_encrypt {
                if let Some(unseal_key) = load_unseal_key_for_scope(scope_name(scope)).await {
                    if let Ok(ct) = encrypt_token(&unseal_key, value.as_bytes()) {
                        hex::encode(ct)
                    } else {
                        value.clone()
                    }
                } else {
                    value.clone()
                }
            } else {
                value.clone()
            };
            ScopedConfig::add(scope, key, &store_value, should_encrypt).await?;
            imported += 1;
            if should_encrypt {
                auto_encrypted += 1;
            }
        } else {
            // Single-value: track for last-one-wins
            let count = last_value_wins
                .entry(key.clone())
                .or_insert_with(|| (String::new(), 0));
            count.0 = value.clone();
            count.1 += 1;
        }
    }

    // Apply last-one-wins entries
    for (key, (value, count)) in &last_value_wins {
        if *count > 1 {
            collapsed_warnings += 1;
            emit_warning(format!(
                "key '{key}' has {count} values in Git config, only last value kept (not in known multi-value list)"
            ));
        }

        let existing = ScopedConfig::get(scope, key).await?;
        if existing.as_ref().map(|e| &e.value) == Some(value) {
            skipped += 1;
            continue;
        }
        let should_encrypt = is_sensitive_key(key);
        let store_value = if should_encrypt {
            if let Some(unseal_key) = load_unseal_key_for_scope(scope_name(scope)).await {
                if let Ok(ct) = encrypt_token(&unseal_key, value.as_bytes()) {
                    hex::encode(ct)
                } else {
                    value.clone()
                }
            } else {
                value.clone()
            }
        } else {
            value.clone()
        };
        ScopedConfig::set(scope, key, &store_value, should_encrypt).await?;
        imported += 1;
        if should_encrypt {
            auto_encrypted += 1;
        }
    }

    if ignored_invalid > 0 {
        emit_warning(format!(
            "ignored {ignored_invalid} unsupported Git config entries"
        ));
    }

    Ok(ConfigImportSummary {
        scope: scope_name(scope),
        imported,
        skipped_duplicates: skipped,
        ignored_invalid,
        auto_encrypted,
        collapsed_multivalue_warnings: collapsed_warnings,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Cascade helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn get_cascaded(key: &str) -> Result<Option<(ConfigKvEntry, ConfigScope)>, String> {
    for scope in ConfigScope::CASCADE_ORDER {
        if scope != ConfigScope::Local {
            let Some(path) = scope.get_config_path() else {
                continue;
            };
            if !path.exists() {
                continue;
            }
        }
        match ScopedConfig::get(scope, key).await {
            Ok(Some(v)) => return Ok(Some((v, scope))),
            Ok(None) => continue,
            Err(e) => {
                return Err(format!("failed to read {} config: {e}", scope_name(scope)));
            }
        }
    }
    Ok(None)
}

async fn get_all_cascaded(key: &str) -> Result<Vec<(ConfigKvEntry, ConfigScope)>, String> {
    let mut out = Vec::new();
    for scope in ConfigScope::CASCADE_ORDER {
        if scope != ConfigScope::Local {
            let Some(path) = scope.get_config_path() else {
                continue;
            };
            if !path.exists() {
                continue;
            }
        }
        match ScopedConfig::get_all(scope, key).await {
            Ok(values) => {
                for v in values {
                    out.push((v, scope));
                }
            }
            Err(e) => return Err(format!("failed to read {} config: {e}", scope_name(scope))),
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Output helpers
// ─────────────────────────────────────────────────────────────────────────────

fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => "local",
        ConfigScope::Global => "global",
    }
}

fn get_scope(args: &ConfigArgs) -> ConfigScope {
    if args.global {
        ConfigScope::Global
    } else {
        ConfigScope::Local
    }
}

fn has_explicit_scope(args: &ConfigArgs) -> bool {
    args.local || args.global || args.system
}

fn emit_set_ack(
    action: &str,
    scope: ConfigScope,
    key: &str,
    encrypted: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": action,
                "scope": scope_name(scope),
                "key": key,
                "encrypted": encrypted,
            }),
            output,
        )?;
    } else if !output.quiet {
        let scope_label = scope_name(scope);
        let enc_label = if encrypted { " (encrypted)" } else { "" };
        let action_label = if action == "add" { "Added" } else { "Set" };
        println!("{action_label} {scope_label}{enc_label}: {key}");
    }
    Ok(())
}

fn print_import_summary(summary: &ConfigImportSummary) {
    if summary.imported > 0 {
        println!(
            "Imported {} entries from Git {} config → libra {} config",
            summary.imported, summary.scope, summary.scope
        );
    } else {
        println!(
            "No new entries to import from Git {} config.",
            summary.scope
        );
    }
    let mut details = Vec::new();
    if summary.skipped_duplicates > 0 {
        details.push(format!("{} duplicates", summary.skipped_duplicates));
    }
    if summary.ignored_invalid > 0 {
        details.push(format!("{} invalid keys", summary.ignored_invalid));
    }
    if !details.is_empty() {
        println!("  skipped: {}", details.join(", "));
    }
    if summary.auto_encrypted > 0 {
        println!(
            "  encrypted: {} sensitive key{} auto-encrypted",
            summary.auto_encrypted,
            if summary.auto_encrypted == 1 { "" } else { "s" }
        );
    }
    if summary.collapsed_multivalue_warnings > 0 {
        println!(
            "  warnings: {} multi-value keys collapsed",
            summary.collapsed_multivalue_warnings
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod args_tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn scope_flags_are_mutually_exclusive() {
        let args = ConfigArgs::try_parse_from([
            "config",
            "--global",
            "--local",
            "set",
            "user.name",
            "test",
        ]);
        assert!(args.is_err());
    }

    #[test]
    fn subcommand_set_parses() {
        let args = ConfigArgs::try_parse_from(["config", "set", "user.name", "John"]).unwrap();
        assert!(matches!(args.command, Some(ConfigCommand::Set { .. })));
    }

    #[test]
    fn subcommand_get_parses() {
        let args = ConfigArgs::try_parse_from(["config", "get", "user.name"]).unwrap();
        assert!(matches!(args.command, Some(ConfigCommand::Get { .. })));
    }

    #[test]
    fn subcommand_list_parses() {
        let args = ConfigArgs::try_parse_from(["config", "list"]).unwrap();
        assert!(matches!(args.command, Some(ConfigCommand::List { .. })));
    }

    #[test]
    fn git_compat_list_flag() {
        let args = ConfigArgs::try_parse_from(["config", "-l"]).unwrap();
        assert!(args.list);
    }
}
