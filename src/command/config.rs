//! Config command for reading and writing settings across scopes.
//!
//! Supports subcommand style (`libra config set/get/list/unset/import/path`)
//! and Git-compatible flag style (`--get`, `--list`, etc.).

use std::{io::IsTerminal, path::PathBuf, process::Command};

use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{
    internal::{
        config::{
            ConfigKv, ConfigKvEntry, ValueFilter, is_protected_vault_section, is_sensitive_key,
            is_vault_internal_key, validate_config_regex_pattern, validate_key_syntax,
            validate_section_syntax,
        },
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
    libra config list --show-origin --show-scope       List with file: origin and local/global scope
    libra config list --null                           NUL-delimited records for scripts (-z)
    libra config set --global user.email "j@x.com"     Set global config
    libra config unset user.signingkey                 Remove a key
    libra config --replace-all remote.origin.fetch +refs/heads/*:refs/remotes/origin/*  Replace all values
    libra config --get-all remote.origin.fetch --value '^main$' --fixed-value  Filter multi-values literally
    libra config rename-section remote.origin remote.upstream  Move a config section
    libra config import --global                       Import from Git global config
    libra config set vault.env.GEMINI_API_KEY          Store API key (interactive)
    echo "$SECRET" | libra config set --stdin vault.env.KEY  Set from stdin (CI/CD)
    libra config set --encrypt custom.key "value"      Force-encrypt a value
    libra config list --vault                          List vault env entries
    libra config generate-ssh-key --remote origin      Generate SSH key for remote
    libra config generate-gpg-key                      Generate GPG signing key
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
                        // The global config DB may hold (encrypted) secrets, so
                        // restrict it to the owner on first creation. Windows
                        // has no direct equivalent here; that is a documented
                        // platform difference.
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            std::fs::set_permissions(
                                &config_path,
                                std::fs::Permissions::from_mode(0o600),
                            )
                            .map_err(|e| {
                                format!(
                                    "Failed to restrict permissions on global config database: {e}"
                                )
                            })?;
                        }
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

/// Wave 3–6 Git-compat flags, flattened into [`ConfigArgs`].
///
/// These are deliberately NOT `global = true` (global args propagate to every
/// subcommand and bloat clap's generated builders) and are grouped here so the
/// generated `augment_args` functions stay small enough for the in-process
/// `exec_async` test path's stack. They still parse in flag-mode and before a
/// subcommand, which covers the supported scripting surface.
#[derive(clap::Args, Debug, Default)]
pub struct GitExtraArgs {
    /// Filter values by regex (or a literal string with --fixed-value).
    ///
    /// The clap field is `value_filter` (not `value`) on purpose: a bare `value`
    /// id collides with the `ConfigCommand::Set { value }` positional, and clap's
    /// arg merging would then route the set positional into this filter.
    /// `long = "value"` keeps the Git-compatible `--value` spelling.
    #[clap(long = "value", value_name = "pattern", hide = true)]
    pub value_filter: Option<String>,
    /// Treat --value as a literal string instead of a regex
    #[clap(long("fixed-value"), hide = true)]
    pub fixed_value: bool,
    /// Case-insensitive key/value regex matching
    #[clap(long("ignore-case"), short = 'i', hide = true)]
    pub ignore_case: bool,
    /// Replace all values for a key with a single new value (Git --replace-all)
    #[clap(long("replace-all"), hide = true)]
    pub replace_all: bool,
    /// Rename a config section: takes <old> <new> via the key/value positionals
    #[clap(long("rename-section"), hide = true)]
    pub rename_section: bool,
    /// Remove a config section: takes <section> via the key positional
    #[clap(long("remove-section"), hide = true)]
    pub remove_section: bool,
    /// Canonicalize the value as a type: bool | int | path
    #[clap(long = "type", value_name = "type", hide = true)]
    pub r#type: Option<String>,
    /// Alias for --type=bool
    #[clap(long, hide = true)]
    pub bool: bool,
    /// Alias for --type=int
    #[clap(long, hide = true)]
    pub int: bool,
    /// Alias for --type=path
    #[clap(long, hide = true)]
    pub path: bool,
    /// (rejected) worktree-scoped config
    #[clap(long, hide = true)]
    pub worktree: bool,
    /// (rejected) read/write a plaintext config file
    #[clap(long, short = 'f', value_name = "path", hide = true)]
    pub file: Option<String>,
    /// (rejected) read config from a blob
    #[clap(long, value_name = "blob", hide = true)]
    pub blob: Option<String>,
    /// (rejected) follow include directives
    #[clap(long, hide = true)]
    pub includes: bool,
    /// (rejected) do not follow include directives
    #[clap(long("no-includes"), hide = true)]
    pub no_includes: bool,
    /// (rejected) legacy color helper
    #[clap(long("get-color"), hide = true)]
    pub get_color: bool,
    /// (rejected) legacy colorbool helper
    #[clap(long("get-colorbool"), hide = true)]
    pub get_colorbool: bool,
    /// (rejected) clear a value pattern
    #[clap(long("no-value"), hide = true)]
    pub no_value: bool,
    /// (rejected) show only names for modern get
    #[clap(long("show-names"), hide = true)]
    pub show_names: bool,
    /// (rejected) hide names for modern get
    #[clap(long("no-show-names"), hide = true)]
    pub no_show_names: bool,
    /// (rejected) clear the value type
    #[clap(long("no-type"), hide = true)]
    pub no_type: bool,
    /// (rejected) URL-matched config lookup
    #[clap(long, value_name = "url", hide = true)]
    pub url: Option<String>,
    /// (rejected) URL-matched config lookup
    #[clap(long("get-urlmatch"), hide = true)]
    pub get_urlmatch: bool,
}

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
    /// Show the `file:<path>` SQLite origin for each value
    #[clap(long("show-origin"), global = true, hide = true)]
    pub show_origin: bool,
    /// Show the local/global scope for each value
    #[clap(long("show-scope"), global = true, hide = true)]
    pub show_scope: bool,
    /// NUL-delimit output records (Git -z)
    #[clap(long, short = 'z', global = true, hide = true)]
    pub null: bool,
    /// Show only key names (list / get-regexp)
    #[clap(long("name-only"), global = true, hide = true)]
    pub name_only: bool,
    /// Reveal encrypted values (Git-compat get paths)
    #[clap(long, hide = true)]
    pub reveal: bool,
    /// Filter values by regex (or a literal string with --fixed-value).
    ///
    /// The clap field is `value_filter` (not `value`) on purpose: a bare `value`
    /// id collides with the `ConfigCommand::Set { value }` positional, and clap's
    /// arg merging would then route the set positional into this filter.
    /// `long = "value"` keeps the Git-compatible `--value` spelling.
    ///
    /// These Git-compat flags are intentionally NOT `global = true`: every global
    /// flag is propagated to every subcommand and inflates clap's generated
    /// `augment_subcommands` stack frame, which overflows tokio's small default
    /// test-thread stack for in-process callers. As hidden top-level flags they
    /// still parse in flag-mode (`config --get key --value …`) and before a
    /// subcommand, which is all the supported scripting surface needs.
    /// Wave 3–6 Git-compat flags, flattened into a sub-struct.
    ///
    /// Flattening is deliberate: bundling these ~23 hidden flags here keeps
    /// `ConfigArgs`'s clap-derived `augment_args` function small. A single flat
    /// struct with this many args produces a giant generated builder whose
    /// stack frame (which transitively contains the `augment_subcommands` call)
    /// overflows tokio's small default test-thread stack for in-process
    /// `exec_async` callers. Flatten splits it into a separate, smaller augment
    /// function. Access via `args.extra.<field>`.
    #[command(flatten)]
    pub extra: GitExtraArgs,

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
        /// Append a value without replacing existing ones (Git `set --append`)
        #[clap(long)]
        append: bool,
        /// Replace all existing values with this one (Git `set --all`)
        #[clap(long)]
        all: bool,
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
    /// Rename a configuration section (e.g. remote.origin -> remote.upstream)
    RenameSection {
        /// Existing section name (dotted prefix, e.g. remote.origin)
        old: String,
        /// New section name (dotted prefix, e.g. remote.upstream)
        new: String,
    },
    /// Remove a configuration section and all of its keys
    RemoveSection {
        /// Section name to remove (dotted prefix, e.g. branch.main)
        section: String,
    },
    /// Import configuration from Git
    Import,
    /// Show config database file path
    Path,
    /// Open config in editor (not supported — SQLite storage)
    Edit,
    /// Generate SSH key for a remote
    GenerateSshKey {
        /// Remote name to bind the new SSH key to
        #[clap(long, value_name = "NAME")]
        remote: String,
    },
    /// Generate GPG key for signing
    GenerateGpgKey {
        /// User name for the key (default: from `user.name` config)
        #[clap(long, value_name = "NAME")]
        name: Option<String>,
        /// User email for the key (default: from `user.email` config)
        #[clap(long, value_name = "EMAIL")]
        email: Option<String>,
        /// Key usage: `signing` (default) or `encrypt`
        #[clap(long, value_name = "KIND")]
        usage: Option<String>,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Serializable output types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
struct ConfigListEntry {
    key: String,
    value: Option<String>,
    /// Backward-compatible scope label (`local`/`global`). Do not repurpose.
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    /// Explicit scope label, emitted only when `--show-scope` is requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    /// Git-style origin kind (`"file"`), emitted only with `--show-origin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_type: Option<String>,
    /// Git-style origin path (the backing SQLite DB), emitted only with `--show-origin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_path: Option<String>,
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
    // Reject unsupported scopes and source selectors as early as possible —
    // before opening any database, reading a `--file`/blob path, or consuming
    // stdin. These are SQLite/vault-backed-config incompatibilities, surfaced as
    // actionable CLI usage errors (exit 129 in coarse mode).
    if args.system {
        return Err(CliError::command_usage(
            "--system scope is not supported\n\nhint: use --local or --global",
        ));
    }
    if args.extra.worktree {
        return Err(CliError::command_usage(
            "--worktree scope is not supported by Libra's SQLite/vault-backed config\n\nhint: use --local or --global",
        ));
    }
    if args.extra.file.is_some() {
        return Err(CliError::command_usage(
            "--file is not supported (config is SQLite/vault-backed)\n\nhint: to import a Git config file use `libra config --import`",
        ));
    }
    if args.extra.blob.is_some() {
        return Err(CliError::command_usage(
            "--blob is not supported: Libra config is SQLite/vault-backed, not blob-backed",
        ));
    }
    if args.extra.includes || args.extra.no_includes {
        return Err(CliError::command_usage(
            "include directives are not supported: they are not part of Libra's SQLite-backed config",
        ));
    }
    if args.extra.get_color || args.extra.get_colorbool {
        return Err(CliError::command_usage(
            "--get-color / --get-colorbool are deferred legacy color helpers and not yet supported",
        ));
    }
    if args.extra.no_value || args.extra.show_names || args.extra.no_show_names {
        return Err(CliError::command_usage(
            "--no-value / --show-names / --no-show-names are not supported by Libra config",
        ));
    }
    if args.extra.url.is_some() || args.extra.get_urlmatch {
        return Err(CliError::command_usage(
            "--url / --get-urlmatch are deferred advanced Git compatibility features and not yet supported",
        ));
    }

    // Collapse the typed-value flags up front (rejects --no-type, unknown/
    // deferred types, and multiple selectors with exit 129).
    let config_type = resolve_type(&args)?;

    let scope = get_scope(&args);
    let use_cascade = !has_explicit_scope(&args);

    // Resolve subcommand: either explicit or translated from Git-compat flags
    let cmd = resolve_command(&args)?;

    // `--type` only applies to get/set value canonicalization; reject it with
    // list (Git filters un-canonicalizable entries, which silently drops config)
    // and with section operations.
    if config_type.is_some()
        && matches!(
            cmd,
            ResolvedCommand::List { .. }
                | ResolvedCommand::RenameSection { .. }
                | ResolvedCommand::RemoveSection { .. }
        )
    {
        return Err(CliError::command_usage(
            "--type can only be used with get/get-all/get-regexp or set",
        ));
    }

    // `--null` only controls output delimiters; it must never be repurposed to
    // parse stdin input. Reject the combination before any stdin is consumed.
    if args.null
        && let ResolvedCommand::Set { stdin: true, .. } = &cmd
    {
        return Err(CliError::command_usage(
            "--null controls output delimiters and cannot be used to parse stdin values",
        ));
    }

    match cmd {
        ResolvedCommand::Set {
            key,
            value,
            add,
            replace_all,
            encrypt,
            plaintext,
            stdin,
            filter,
        } => {
            handle_set(
                &key,
                value.as_deref(),
                add,
                replace_all,
                encrypt,
                plaintext,
                stdin,
                filter,
                config_type,
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
            flags,
            filter,
        } => {
            handle_get(
                &key,
                all,
                reveal,
                regexp,
                default.as_deref(),
                flags,
                filter,
                config_type,
                scope,
                use_cascade,
                output,
            )
            .await
        }
        ResolvedCommand::List {
            flags,
            vault,
            ssh_keys,
            gpg_keys,
        } => handle_list(flags, vault, ssh_keys, gpg_keys, scope, use_cascade, output).await,
        ResolvedCommand::Unset { key, all, filter } => {
            handle_unset(&key, all, filter, scope, output).await
        }
        ResolvedCommand::RenameSection { old, new } => {
            handle_rename_section(&old, &new, scope, output).await
        }
        ResolvedCommand::RemoveSection { section } => {
            handle_remove_section(&section, scope, output).await
        }
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

/// Script-safe display flags shared by `get`/`list` output paths.
///
/// These are parsed as global Git-compat flags on [`ConfigArgs`] and bundled
/// here so handler signatures do not balloon as more output options land.
#[derive(Debug, Clone, Copy, Default)]
struct OutputFlags {
    /// NUL-delimit records (`-z`/`--null`): `key\nvalue\0` instead of `key=value\n`.
    null: bool,
    /// Prefix each record with its `local`/`global` scope label.
    show_scope: bool,
    /// Prefix each record with its `file:<path>` SQLite origin.
    show_origin: bool,
    /// Emit key names only (no values).
    name_only: bool,
}

impl OutputFlags {
    fn from_args(args: &ConfigArgs) -> Self {
        Self {
            null: args.null,
            show_scope: args.show_scope,
            show_origin: args.show_origin,
            name_only: args.name_only,
        }
    }

    /// True when a scope/origin prefix must precede each record.
    fn prefixed(&self) -> bool {
        self.show_scope || self.show_origin
    }
}

/// Git-compatible `--value` / `--fixed-value` / `--ignore-case` filter inputs.
///
/// Parsed from the global Git-compat flags on [`ConfigArgs`] and threaded into
/// `get`/`set`/`unset` handlers. [`Self::compile`] validates and compiles the
/// pattern up front (invalid/over-long → exit 6) so it fails before any DB
/// access; `ignore_case` is also consulted by `get-regexp` for the key regex.
#[derive(Debug, Clone, Default)]
struct ValueFilterSpec {
    pattern: Option<String>,
    fixed: bool,
    ignore_case: bool,
}

impl ValueFilterSpec {
    fn from_args(args: &ConfigArgs) -> Self {
        Self {
            pattern: args.extra.value_filter.clone(),
            fixed: args.extra.fixed_value,
            ignore_case: args.extra.ignore_case,
        }
    }

    /// Compile to an internal [`ValueFilter`], or `None` when no `--value` was
    /// given. Invalid or over-long patterns map to exit code 6.
    fn compile(&self) -> CliResult<Option<ValueFilter>> {
        match &self.pattern {
            None => Ok(None),
            Some(p) => ValueFilter::compile(p, self.fixed, self.ignore_case)
                .map(Some)
                .map_err(regex_cli_error),
        }
    }
}

/// Map an invalid/over-long regex error to a stable exit-6 CLI error.
fn regex_cli_error(err: impl std::fmt::Display) -> CliError {
    CliError::from_legacy_string(format!("error: {err}")).with_exit_code(6)
}

/// Map a SQLite write error (including locked/busy/full) to an actionable
/// exit-4 CLI error. The underlying message is a DB error, never a config
/// value, so no secret material is surfaced.
fn config_write_cli_error(context: &str, err: impl std::fmt::Display) -> CliError {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    let hint = if lower.contains("sqlite_full") || lower.contains("disk is full") {
        "insufficient disk space"
    } else if lower.contains("locked") || lower.contains("sqlite_busy") || lower.contains("busy") {
        "database is locked"
    } else {
        "failed to write config"
    };
    CliError::fatal(format!("{context}: {hint}: {message}"))
        .with_stable_code(StableErrorCode::IoWriteFailed)
        .with_exit_code(4)
}

/// Supported `--type` canonicalizations. Table-driven so adding `color` /
/// `bool-or-int` later only extends this enum and [`canonicalize_typed_value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigType {
    Bool,
    Int,
    Path,
}

/// Collapse `--type=<t>` and the `--bool`/`--int`/`--path` aliases into a single
/// optional [`ConfigType`]. Unknown/deferred types, `--no-type`, and multiple
/// selectors are usage errors (exit 129 in coarse mode).
fn resolve_type(args: &ConfigArgs) -> CliResult<Option<ConfigType>> {
    let mut picks: Vec<ConfigType> = Vec::new();
    if args.extra.bool {
        picks.push(ConfigType::Bool);
    }
    if args.extra.int {
        picks.push(ConfigType::Int);
    }
    if args.extra.path {
        picks.push(ConfigType::Path);
    }
    if let Some(t) = args.extra.r#type.as_deref() {
        match t {
            "bool" => picks.push(ConfigType::Bool),
            "int" => picks.push(ConfigType::Int),
            "path" => picks.push(ConfigType::Path),
            "bool-or-int" | "expiry-date" | "color" => {
                return Err(CliError::command_usage(format!(
                    "--type={t} is not yet supported by Libra config"
                )));
            }
            other => {
                return Err(CliError::command_usage(format!(
                    "unknown --type value: {other}"
                )));
            }
        }
    }
    if args.extra.no_type {
        return Err(CliError::command_usage(
            "--no-type is not supported by Libra config",
        ));
    }
    if picks.len() > 1 {
        return Err(CliError::command_usage(
            "multiple type selectors specified (--type / --bool / --int / --path)",
        ));
    }
    Ok(picks.into_iter().next())
}

/// Resolve the `HOME` (or, on Windows, `USERPROFILE`) directory for `~`
/// expansion. Reads the environment directly (no passwd fallback) so an unset or
/// empty value is an error, matching the documented `--type=path` contract.
fn typed_path_home() -> CliResult<PathBuf> {
    let vars: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    for var in vars {
        if let Some(v) = std::env::var_os(var)
            && !v.is_empty()
        {
            return Ok(PathBuf::from(v));
        }
    }
    Err(
        CliError::from_legacy_string("error: cannot expand '~': HOME is not set or empty")
            .with_exit_code(1),
    )
}

/// Expand a leading `~` / `~/child` for `--type=path` reads. `~user` is rejected
/// (exit 1); a value without a leading `~` is returned unchanged.
fn expand_typed_path(value: &str) -> CliResult<String> {
    if value == "~" {
        return Ok(typed_path_home()?.to_string_lossy().into_owned());
    }
    if let Some(child) = value.strip_prefix("~/") {
        return Ok(typed_path_home()?
            .join(child)
            .to_string_lossy()
            .into_owned());
    }
    if value.starts_with('~') {
        return Err(CliError::from_legacy_string(
            "~user path expansion is not supported by Libra config",
        )
        .with_exit_code(1));
    }
    Ok(value.to_string())
}

/// Canonicalize an already-revealed plaintext value for `--type=bool|int|path`.
/// Bool/int parse failures and int overflow map to exit 2; `~user` / unset-HOME
/// path errors map to exit 1. Error messages never echo the value (no secret
/// leak for sensitive keys).
fn canonicalize_typed_value(ty: ConfigType, value: &str) -> CliResult<String> {
    match ty {
        ConfigType::Bool => {
            let b = crate::internal::config::parse_config_bool(value).ok_or_else(|| {
                CliError::from_legacy_string("error: invalid boolean value (expected true/false)")
                    .with_exit_code(2)
            })?;
            Ok(if b { "true" } else { "false" }.to_string())
        }
        ConfigType::Int => {
            let n = crate::internal::config::parse_config_int(value).map_err(|e| {
                CliError::from_legacy_string(format!("error: invalid integer value: {e}"))
                    .with_exit_code(2)
            })?;
            Ok(n.to_string())
        }
        ConfigType::Path => expand_typed_path(value),
    }
}

/// Apply `--type` canonicalization to a rendered get value. Encrypted values
/// that were not revealed (or are vault-internal) stay `<REDACTED>` and are
/// never parsed as a type.
fn typed_output(
    entry: &ConfigKvEntry,
    reveal: bool,
    rendered: &str,
    ty: Option<ConfigType>,
) -> CliResult<String> {
    match ty {
        None => Ok(rendered.to_string()),
        Some(t) => {
            if entry.encrypted && (!reveal || is_vault_internal_key(&entry.key)) {
                Ok(rendered.to_string())
            } else {
                canonicalize_typed_value(t, rendered)
            }
        }
    }
}

#[derive(Debug)]
enum ResolvedCommand {
    Set {
        key: String,
        value: Option<String>,
        add: bool,
        replace_all: bool,
        encrypt: bool,
        plaintext: bool,
        stdin: bool,
        filter: ValueFilterSpec,
    },
    Get {
        key: String,
        all: bool,
        reveal: bool,
        regexp: bool,
        default: Option<String>,
        flags: OutputFlags,
        filter: ValueFilterSpec,
    },
    List {
        flags: OutputFlags,
        vault: bool,
        ssh_keys: bool,
        gpg_keys: bool,
    },
    Unset {
        key: String,
        all: bool,
        filter: ValueFilterSpec,
    },
    RenameSection {
        old: String,
        new: String,
    },
    RemoveSection {
        section: String,
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
                append,
                all,
                encrypt,
                plaintext,
                stdin,
            } => ResolvedCommand::Set {
                key: key.clone(),
                value: value.clone(),
                add: *add || *append,
                replace_all: *all || args.extra.replace_all,
                encrypt: *encrypt,
                plaintext: *plaintext,
                stdin: *stdin,
                filter: ValueFilterSpec::from_args(args),
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
                flags: OutputFlags::from_args(args),
                filter: ValueFilterSpec::from_args(args),
            },
            ConfigCommand::List {
                vault,
                ssh_keys,
                gpg_keys,
            } => ResolvedCommand::List {
                flags: OutputFlags::from_args(args),
                vault: *vault,
                ssh_keys: *ssh_keys,
                gpg_keys: *gpg_keys,
            },
            ConfigCommand::Unset { key, all } => ResolvedCommand::Unset {
                key: key.clone(),
                all: *all,
                filter: ValueFilterSpec::from_args(args),
            },
            ConfigCommand::RenameSection { old, new } => ResolvedCommand::RenameSection {
                old: old.clone(),
                new: new.clone(),
            },
            ConfigCommand::RemoveSection { section } => ResolvedCommand::RemoveSection {
                section: section.clone(),
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

    // Git-compat section flags reuse the key/value positionals as <old> <new>
    // (rename) or <section> (remove). Handle them before the generic key path
    // so the `<old> <new>` arguments are not misread as a set's key/value.
    if args.extra.rename_section {
        let old = args.key.as_deref().ok_or_else(|| {
            CliError::from_legacy_string("error: --rename-section requires <old> <new>")
                .with_exit_code(2)
        })?;
        let new = args.valuepattern.as_deref().ok_or_else(|| {
            CliError::from_legacy_string("error: --rename-section requires <old> <new>")
                .with_exit_code(2)
        })?;
        return Ok(ResolvedCommand::RenameSection {
            old: old.to_string(),
            new: new.to_string(),
        });
    }
    if args.extra.remove_section {
        let section = args.key.as_deref().ok_or_else(|| {
            CliError::from_legacy_string("error: --remove-section requires <section>")
                .with_exit_code(2)
        })?;
        if args.valuepattern.is_some() {
            return Err(CliError::from_legacy_string(
                "error: --remove-section takes exactly one <section>",
            )
            .with_exit_code(2));
        }
        return Ok(ResolvedCommand::RemoveSection {
            section: section.to_string(),
        });
    }

    // Git-compat flag translation
    if args.list {
        return Ok(ResolvedCommand::List {
            flags: OutputFlags::from_args(args),
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

    // Validate key format (must contain at least one dot). For `--get-regexp`
    // the "key" is a regex pattern, not a literal key, so the dot requirement
    // does not apply — its length/syntax are validated when the pattern is
    // compiled (over-long → exit 6).
    if !args.get_regexp && !key.contains('.') {
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
            reveal: args.reveal,
            regexp: true,
            default: args.default.clone(),
            flags: OutputFlags::from_args(args),
            filter: ValueFilterSpec::from_args(args),
        });
    }
    if args.get {
        return Ok(ResolvedCommand::Get {
            key: key.to_string(),
            all: false,
            reveal: args.reveal,
            regexp: false,
            default: args.default.clone(),
            flags: OutputFlags::from_args(args),
            filter: ValueFilterSpec::from_args(args),
        });
    }
    if args.get_all {
        return Ok(ResolvedCommand::Get {
            key: key.to_string(),
            all: true,
            reveal: args.reveal,
            regexp: false,
            default: args.default.clone(),
            flags: OutputFlags::from_args(args),
            filter: ValueFilterSpec::from_args(args),
        });
    }
    if args.unset {
        return Ok(ResolvedCommand::Unset {
            key: key.to_string(),
            all: false,
            filter: ValueFilterSpec::from_args(args),
        });
    }
    if args.unset_all {
        return Ok(ResolvedCommand::Unset {
            key: key.to_string(),
            all: true,
            filter: ValueFilterSpec::from_args(args),
        });
    }
    // Flag-mode append is the legacy `--add`; `--append` is the modern
    // `set --append` subcommand field (see ConfigCommand::Set).
    if args.add {
        let value = args.valuepattern.as_deref().ok_or_else(|| {
            CliError::from_legacy_string("error: missing required argument: <value>")
                .with_exit_code(2)
        })?;
        return Ok(ResolvedCommand::Set {
            key: key.to_string(),
            value: Some(value.to_string()),
            add: true,
            replace_all: false,
            encrypt: false,
            plaintext: false,
            stdin: false,
            filter: ValueFilterSpec::from_args(args),
        });
    }
    // `--replace-all` replaces every (or every matching) value with one new value.
    if args.extra.replace_all {
        let value = args.valuepattern.as_deref().ok_or_else(|| {
            CliError::from_legacy_string("error: missing required argument: <value>")
                .with_exit_code(2)
        })?;
        return Ok(ResolvedCommand::Set {
            key: key.to_string(),
            value: Some(value.to_string()),
            add: false,
            replace_all: true,
            encrypt: false,
            plaintext: false,
            stdin: false,
            filter: ValueFilterSpec::from_args(args),
        });
    }

    // Default: set mode (key + optional value).
    // When value is omitted, handle_set will trigger interactive input for
    // sensitive keys or report a missing-value error for ordinary keys.
    Ok(ResolvedCommand::Set {
        key: key.to_string(),
        value: args.valuepattern.clone(),
        add: false,
        replace_all: false,
        filter: ValueFilterSpec::from_args(args),
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
    replace_all: bool,
    encrypt: bool,
    plaintext: bool,
    stdin: bool,
    filter: ValueFilterSpec,
    config_type: Option<ConfigType>,
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    // Validate the user-typed key syntax (permissive flat-dotted-key rules).
    // Genuine malformations exit 1; legal-but-non-classic keys (underscores,
    // camelCase, many segments) are accepted. NOT applied to `--import`.
    validate_key_syntax(key)
        .map_err(|e| CliError::from_legacy_string(format!("error: {e}")).with_exit_code(1))?;

    // Compile any `--value` filter up front so an invalid/over-long pattern
    // fails (exit 6) before any DB access or mutation.
    let value_filter = filter.compile()?;

    // `--encrypt` and `--plaintext` are mutually exclusive. config.md (line 77)
    // classifies this as a CLI usage error (exit 2 in fine mode, 129 in
    // coarse) — route through `command_usage` so the category matches.
    if encrypt && plaintext {
        return Err(CliError::command_usage(
            "--encrypt and --plaintext are mutually exclusive",
        ));
    }

    // `--plaintext` must not be used with vault internal/secret keys.
    // config.md (line 77) classifies this as a validation reject (exit 1 in
    // fine mode). We use `Failure` (coarse 128) with a stable code so the
    // error class is recoverable rather than silently falling through to
    // `InternalInvariant`.
    if plaintext && (is_vault_internal_key(key) || key.starts_with("vault.env.")) {
        return Err(CliError::failure(
            "--plaintext cannot be used with vault internal/secret keys",
        )
        .with_stable_code(StableErrorCode::RepoStateInvalid));
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
        // `--stdin` and a positional value are mutually exclusive (config.md
        // line 144 — usage error, exit 2 fine / 129 coarse).
        if value.is_some() {
            return Err(CliError::command_usage(
                "cannot use both value argument and --stdin",
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

    // `--type=bool|int` validate and normalize the input before storage (the
    // canonical form is stored). `--type=path` stores the value verbatim and
    // only expands `~` on read. Validation runs on the plaintext before any
    // encryption; error messages never echo the value (no secret leak).
    let resolved_value = match config_type {
        Some(ty @ (ConfigType::Bool | ConfigType::Int)) => {
            canonicalize_typed_value(ty, &resolved_value)?
        }
        Some(ConfigType::Path) | None => resolved_value,
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
        // `--append` / `--add`: append without replacing existing values.
        ScopedConfig::add(scope, key, &store_value, should_encrypt)
            .await
            .map_err(CliError::from_legacy_string)?;
        emit_set_ack("add", scope, key, should_encrypt, output)?;
    } else if replace_all || value_filter.is_some() {
        // `--replace-all` / `set --all`, or a value-filtered set: read matching
        // rows by id, delete them, and insert the new value — all in one
        // transaction so any error path leaves the store unchanged. A default
        // (non-replace-all) set with a `--value` filter must reject an ambiguous
        // (>1) match set with exit 5, mirroring Git.
        let conn = ScopedConfig::get_connection(scope)
            .await
            .map_err(CliError::from_legacy_string)?;
        let txn = conn
            .begin()
            .await
            .map_err(|e| config_write_cli_error("failed to begin config transaction", e))?;
        let enforce_single = !replace_all;
        ConfigKv::replace_matching_with_conn(
            &txn,
            key,
            &store_value,
            should_encrypt,
            value_filter.as_ref(),
            enforce_single,
        )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("values exist") {
                CliError::from_legacy_string(format!("error: {msg}")).with_exit_code(5)
            } else {
                config_write_cli_error("failed to write config", e)
            }
        })?;
        txn.commit()
            .await
            .map_err(|e| config_write_cli_error("failed to commit config transaction", e))?;
        emit_set_ack("set", scope, key, should_encrypt, output)?;
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

/// Resolve the backing SQLite database path for a scope (no `file:` prefix).
///
/// Local uses the repository's `.libra/libra.db`; global uses the resolved
/// global config DB. Returns `None` only when the path cannot be determined
/// (e.g. local scope outside a repository).
fn config_origin_path_string(scope: ConfigScope) -> Option<String> {
    let path = match scope {
        ConfigScope::Local => try_get_storage_path(None).ok().map(|s| s.join(DATABASE)),
        ConfigScope::Global => scope.get_config_path(),
    };
    path.map(|p| p.to_string_lossy().into_owned())
}

/// Git-style `file:<absolute-path>` origin for a scope's backing SQLite DB.
fn format_config_origin(scope: ConfigScope) -> String {
    match config_origin_path_string(scope) {
        Some(p) => format!("file:{p}"),
        None => format!("file:{}", scope_name(scope)),
    }
}

/// Push the scope/origin prefix fields (when requested) onto `out`.
fn push_prefix_fields(out: &mut String, scope: ConfigScope, flags: OutputFlags) {
    let sep = if flags.null { '\0' } else { '\t' };
    if flags.show_scope {
        out.push_str(scope_name(scope));
        out.push(sep);
    }
    if flags.show_origin {
        out.push_str(&format_config_origin(scope));
        out.push(sep);
    }
}

/// Build one key/value text record for `list` / `get-regexp` output.
///
/// `value == None` selects name-only (key only). `kv_sep` joins key and value
/// in non-null text mode (`'='` for `list`, `' '` for `get-regexp`); under
/// `--null` the separator is always `\n` per Git's null record format. The
/// returned string includes its trailing record delimiter (`\n` or `\0`) and
/// any requested scope/origin prefix.
fn format_config_record(
    scope: ConfigScope,
    key: &str,
    value: Option<&str>,
    kv_sep: char,
    flags: OutputFlags,
) -> String {
    let mut out = String::new();
    push_prefix_fields(&mut out, scope, flags);
    match (value, flags.null) {
        // name-only
        (None, false) => {
            out.push_str(key);
            out.push('\n');
        }
        (None, true) => {
            out.push_str(key);
            out.push('\0');
        }
        // key + value
        (Some(v), false) => {
            out.push_str(key);
            out.push(kv_sep);
            out.push_str(v);
            out.push('\n');
        }
        (Some(v), true) => {
            out.push_str(key);
            out.push('\n');
            out.push_str(v);
            out.push('\0');
        }
    }
    out
}

/// Build a single `get` / `get-all` value record (value only, no key),
/// honouring scope/origin prefixes and `--null`.
fn format_get_value_record(scope: ConfigScope, value: &str, flags: OutputFlags) -> String {
    let mut out = String::new();
    push_prefix_fields(&mut out, scope, flags);
    out.push_str(value);
    out.push(if flags.null { '\0' } else { '\n' });
    out
}

/// Build a JSON entry for a `get`/`get-all` value (no `key`), keeping the
/// existing `value`/`origin`/`encrypted` fields and adding `scope`/`origin_type`/
/// `origin_path` only when the corresponding `--show-*` flag is set.
fn get_value_json(
    value: &str,
    scope: ConfigScope,
    encrypted: bool,
    flags: OutputFlags,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("value".into(), serde_json::json!(value));
    obj.insert("origin".into(), serde_json::json!(scope_name(scope)));
    obj.insert("encrypted".into(), serde_json::json!(encrypted));
    if flags.show_scope {
        obj.insert("scope".into(), serde_json::json!(scope_name(scope)));
    }
    if flags.show_origin {
        obj.insert("origin_type".into(), serde_json::json!("file"));
        obj.insert(
            "origin_path".into(),
            serde_json::json!(config_origin_path_string(scope)),
        );
    }
    serde_json::Value::Object(obj)
}

/// Compute the display value for a list entry, honouring redaction, the
/// `[PLAINTEXT]` safety marker (human text mode only), and name-only mode.
fn render_list_value(entry: &ConfigKvEntry, flags: OutputFlags) -> Option<String> {
    if flags.name_only {
        return None;
    }
    if entry.encrypted {
        return Some("<REDACTED>".to_string());
    }
    // Plaintext value. Append a `[PLAINTEXT]` marker for sensitive-looking keys
    // in human text mode only; machine (`--null`) output stays raw.
    if !flags.null && is_sensitive_key(&entry.key) {
        Some(format!("{} [PLAINTEXT]", entry.value))
    } else {
        Some(entry.value.clone())
    }
}

/// Construct a [`ConfigListEntry`] for a prefixed (scope/origin) listing,
/// honouring the JSON backward-compatibility contract: `origin` keeps its
/// scope-label meaning; `scope`/`origin_type`/`origin_path` appear only when
/// the corresponding `--show-*` flag is set.
fn build_prefixed_list_entry(
    key: String,
    value: Option<String>,
    scope: ConfigScope,
    encrypted: bool,
    flags: OutputFlags,
) -> ConfigListEntry {
    ConfigListEntry {
        key,
        value,
        origin: Some(scope_name(scope).to_string()),
        scope: flags.show_scope.then(|| scope_name(scope).to_string()),
        origin_type: flags.show_origin.then(|| "file".to_string()),
        origin_path: if flags.show_origin {
            config_origin_path_string(scope)
        } else {
            None
        },
        encrypted: Some(encrypted),
    }
}

/// Reject `--null` combined with JSON output (JSON is already machine-readable).
fn reject_null_with_json(flags: OutputFlags, output: &OutputConfig) -> CliResult<()> {
    if flags.null && output.is_json() {
        return Err(CliError::command_usage(
            "--null is not compatible with JSON output; JSON is already machine-readable",
        ));
    }
    Ok(())
}

/// Reject `--name-only` combined with JSON output (consumers read the `key` field).
fn reject_name_only_with_json(flags: OutputFlags, output: &OutputConfig) -> CliResult<()> {
    if flags.name_only && output.is_json() {
        return Err(CliError::command_usage(
            "--name-only is not compatible with JSON output; read the `key` field instead",
        ));
    }
    Ok(())
}

/// The string a `--value` filter matches against for one entry.
///
/// Encrypted + revealed (non-vault-internal) keys match on the decrypted text
/// (which is exactly what was rendered for display); every other case matches on
/// the stored value (plaintext, or hex ciphertext for unrevealed secrets), so an
/// unrevealed secret is never decrypted merely to be filtered.
fn value_filter_input<'a>(entry: &'a ConfigKvEntry, reveal: bool, display: &'a str) -> &'a str {
    if entry.encrypted && reveal && !is_vault_internal_key(&entry.key) {
        display
    } else {
        entry.value.as_str()
    }
}

/// Compile a `get-regexp` key pattern, honouring `--ignore-case` and the 4 KiB
/// length cap. Invalid or over-long patterns map to exit code 6.
fn compile_key_regex(pattern: &str, ignore_case: bool) -> CliResult<regex::Regex> {
    validate_config_regex_pattern(pattern).map_err(regex_cli_error)?;
    regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| regex_cli_error(format!("invalid regex pattern '{pattern}': {e}")))
}

/// Read-path error when a `--value` filter matched nothing (Git exit 1, empty stdout).
fn no_value_match_error() -> CliError {
    CliError::failure("no value matched the --value filter")
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_exit_code(1)
}

#[allow(clippy::too_many_arguments)]
async fn handle_get(
    key: &str,
    all: bool,
    reveal: bool,
    regexp: bool,
    default: Option<&str>,
    flags: OutputFlags,
    filter: ValueFilterSpec,
    config_type: Option<ConfigType>,
    scope: ConfigScope,
    use_cascade: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    reject_null_with_json(flags, output)?;
    reject_name_only_with_json(flags, output)?;

    // Compile any `--value` filter up front so an invalid/over-long pattern
    // fails (exit 6) before any DB access.
    let value_filter = filter.compile()?;

    // `--name-only` is only meaningful for list / get-regexp output; a single
    // or multi get of a value has no key column to reduce to.
    if flags.name_only && !regexp {
        return Err(CliError::command_usage(
            "--name-only is only supported for list and --get-regexp; it cannot be combined with a single-key get",
        ));
    }

    // Block --reveal for vault internal keys on exact-key queries
    if reveal && !regexp && !all && is_vault_internal_key(key) {
        return Err(CliError::from_legacy_string(format!(
            "error: key '{}' is a vault internal credential and cannot be revealed",
            key
        )));
    }

    if regexp {
        // Regex search across all keys (honouring --ignore-case and the 4 KiB cap).
        let key_re = compile_key_regex(key, filter.ignore_case)?;
        let scopes: Vec<ConfigScope> = if use_cascade {
            ConfigScope::CASCADE_ORDER.to_vec()
        } else {
            vec![scope]
        };
        let mut entries: Vec<(ConfigKvEntry, ConfigScope)> = Vec::new();
        for s in scopes {
            if s != ConfigScope::Local {
                let Some(path) = s.get_config_path() else {
                    continue;
                };
                if !path.exists() {
                    continue;
                }
            }
            let scope_entries = if use_cascade {
                // Cascade view skips unreadable scopes (mirrors handle_list).
                ScopedConfig::list_all(s).await.map_err(|e| {
                    config_read_cli_error(format!("failed to read {} config: {e}", scope_name(s)))
                })?
            } else {
                ScopedConfig::list_all(s)
                    .await
                    .map_err(CliError::from_legacy_string)?
            };
            for e in scope_entries {
                if key_re.is_match(&e.key) {
                    entries.push((e, s));
                }
            }
        }

        // Build display values + apply the optional value filter (on the raw
        // rendered value) then `--type` canonicalization (on the output value).
        let mut display_entries = Vec::new();
        for (e, s) in &entries {
            let raw = render_get_value(e, reveal, *s, use_cascade).await?;
            if let Some(vf) = &value_filter
                && !vf.matches(value_filter_input(e, reveal, &raw))
            {
                continue;
            }
            let out_val = typed_output(e, reveal, &raw, config_type)?;
            display_entries.push((e, s, out_val));
        }

        // A value-filtered regexp read with no matches exits 1 (empty stdout).
        if value_filter.is_some() && display_entries.is_empty() {
            return Err(no_value_match_error());
        }

        if output.is_json() {
            let json_entries: Vec<ConfigListEntry> = display_entries
                .iter()
                .map(|(e, s, val)| {
                    build_prefixed_list_entry(
                        e.key.clone(),
                        Some(val.clone()),
                        **s,
                        e.encrypted,
                        flags,
                    )
                })
                .collect();
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "get-regexp",
                    "pattern": key,
                    "entries": json_entries,
                }),
                output,
            )?;
        } else if !output.quiet {
            // Git-style `key value` (space-separated); `--name-only` emits the key only.
            for (e, s, val) in &display_entries {
                let value = (!flags.name_only).then_some(val.as_str());
                print!("{}", format_config_record(**s, &e.key, value, ' ', flags));
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
                let bare = OutputFlags {
                    show_scope: false,
                    show_origin: false,
                    ..flags
                };
                print!("{}", format_get_value_record(scope, d, bare));
            }
            return Ok(());
        }

        // Build display values + apply the optional value filter (raw) then
        // `--type` canonicalization (output).
        let mut display_entries = Vec::new();
        for (e, s) in &entries {
            let raw = render_get_value(e, reveal, *s, use_cascade).await?;
            if let Some(vf) = &value_filter
                && !vf.matches(value_filter_input(e, reveal, &raw))
            {
                continue;
            }
            let out_val = typed_output(e, reveal, &raw, config_type)?;
            display_entries.push((e, s, out_val));
        }

        // A value-filtered get-all with no matches exits 1 (empty stdout).
        if value_filter.is_some() && display_entries.is_empty() {
            return Err(no_value_match_error());
        }

        if output.is_json() {
            emit_json_data(
                "config",
                &serde_json::json!({
                    "action": "get-all",
                    "key": key,
                    "entries": display_entries.iter().map(|(e, s, val)| {
                        get_value_json(val, **s, e.encrypted, flags)
                    }).collect::<Vec<_>>(),
                    "default_applied": false,
                }),
                output,
            )?;
        } else if !output.quiet {
            for (_, s, val) in &display_entries {
                print!("{}", format_get_value_record(**s, val, flags));
            }
        }
    } else if let Some(vf) = &value_filter {
        // Single get with `--value`: among all values of the key, return the
        // LAST one that matches the filter (Git's last-one-wins). No match with
        // an existing key exits 1; an entirely absent key may fall back to
        // `--default`.
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

        let mut matched: Option<(ConfigScope, String)> = None;
        for (e, s) in &entries {
            let raw = render_get_value(e, reveal, *s, use_cascade).await?;
            if vf.matches(value_filter_input(e, reveal, &raw)) {
                matched = Some((*s, typed_output(e, reveal, &raw, config_type)?));
            }
        }

        match matched {
            Some((s, val)) => {
                if output.is_json() {
                    let mut obj = serde_json::Map::new();
                    obj.insert("action".into(), serde_json::json!("get"));
                    obj.insert("key".into(), serde_json::json!(key));
                    obj.insert("value".into(), serde_json::json!(val));
                    obj.insert("origin".into(), serde_json::json!(scope_name(s)));
                    obj.insert("default_applied".into(), serde_json::json!(false));
                    if flags.show_scope {
                        obj.insert("scope".into(), serde_json::json!(scope_name(s)));
                    }
                    if flags.show_origin {
                        obj.insert("origin_type".into(), serde_json::json!("file"));
                        obj.insert(
                            "origin_path".into(),
                            serde_json::json!(config_origin_path_string(s)),
                        );
                    }
                    emit_json_data("config", &serde_json::Value::Object(obj), output)?;
                } else if !output.quiet {
                    print!("{}", format_get_value_record(s, &val, flags));
                }
            }
            None if entries.is_empty() && default.is_some() => {
                let d = default.unwrap_or_default();
                if output.is_json() {
                    emit_json_data(
                        "config",
                        &serde_json::json!({
                            "action": "get",
                            "key": key,
                            "value": d,
                            "origin": serde_json::Value::Null,
                            "default_applied": true,
                        }),
                        output,
                    )?;
                } else if !output.quiet {
                    let bare = OutputFlags {
                        show_scope: false,
                        show_origin: false,
                        ..flags
                    };
                    print!("{}", format_get_value_record(scope, d, bare));
                }
            }
            None => return Err(no_value_match_error()),
        }
        return Ok(());
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
                let raw = render_get_value(e, reveal, s, use_cascade).await?;
                let val = typed_output(e, reveal, &raw, config_type)?;
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
            let mut obj = serde_json::Map::new();
            obj.insert("action".into(), serde_json::json!("get"));
            obj.insert("key".into(), serde_json::json!(key));
            obj.insert("value".into(), serde_json::json!(display_value));
            obj.insert(
                "origin".into(),
                serde_json::json!(origin_scope.map(scope_name)),
            );
            obj.insert("default_applied".into(), serde_json::json!(default_applied));
            if let Some(s) = origin_scope {
                if flags.show_scope {
                    obj.insert("scope".into(), serde_json::json!(scope_name(s)));
                }
                if flags.show_origin {
                    obj.insert("origin_type".into(), serde_json::json!("file"));
                    obj.insert(
                        "origin_path".into(),
                        serde_json::json!(config_origin_path_string(s)),
                    );
                }
            }
            emit_json_data("config", &serde_json::Value::Object(obj), output)?;
        } else if !output.quiet {
            match origin_scope {
                Some(s) => print!("{}", format_get_value_record(s, &display_value, flags)),
                None => {
                    // Default applied: no real origin, so drop the scope/origin
                    // prefix but still honour the `--null` record delimiter.
                    let bare = OutputFlags {
                        show_scope: false,
                        show_origin: false,
                        ..flags
                    };
                    print!("{}", format_get_value_record(scope, &display_value, bare));
                }
            }
        }
    }
    Ok(())
}

async fn handle_list(
    flags: OutputFlags,
    vault: bool,
    ssh_keys: bool,
    gpg_keys: bool,
    scope: ConfigScope,
    use_cascade: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    reject_null_with_json(flags, output)?;
    reject_name_only_with_json(flags, output)?;

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
                        ..Default::default()
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

    // General listing. With `--show-origin`/`--show-scope` we surface every
    // scope (cascade) with its prefix; otherwise we list the single resolved
    // scope only (default local), matching the historical behaviour.
    let prefixed = flags.prefixed();
    let cascade_all = prefixed && use_cascade;
    let scopes: Vec<ConfigScope> = if cascade_all {
        ConfigScope::CASCADE_ORDER.to_vec()
    } else {
        vec![scope]
    };

    let mut collected: Vec<(ConfigScope, ConfigKvEntry)> = Vec::new();
    for s in scopes {
        if s != ConfigScope::Local {
            let Some(path) = s.get_config_path() else {
                continue;
            };
            if !path.exists() {
                continue;
            }
        }
        if cascade_all {
            // Cascade view skips unreadable scopes (mirrors prior show-origin loop).
            if let Ok(scope_entries) = ScopedConfig::list_all(s).await {
                for e in scope_entries {
                    collected.push((s, e));
                }
            }
        } else {
            // Single explicit scope: surface read failures as an error.
            let scope_entries = ScopedConfig::list_all(s)
                .await
                .map_err(CliError::from_legacy_string)?;
            for e in scope_entries {
                collected.push((s, e));
            }
        }
    }

    if output.is_json() {
        let entries: Vec<ConfigListEntry> = collected
            .iter()
            .map(|(s, e)| {
                let value = render_list_value(e, flags);
                if prefixed {
                    build_prefixed_list_entry(e.key.clone(), value, *s, e.encrypted, flags)
                } else {
                    ConfigListEntry {
                        key: e.key.clone(),
                        value,
                        encrypted: Some(e.encrypted),
                        ..Default::default()
                    }
                }
            })
            .collect();
        let scope_label = if cascade_all {
            "all"
        } else {
            scope_name(scope)
        };
        let mut payload = serde_json::json!({
            "action": "list",
            "scope": scope_label,
            "entries": entries,
            "count": entries.len(),
        });
        if prefixed {
            payload["cascade"] = serde_json::json!(use_cascade);
        }
        emit_json_data("config", &payload, output)?;
    } else if !output.quiet {
        for (s, e) in &collected {
            let value = render_list_value(e, flags);
            print!(
                "{}",
                format_config_record(*s, &e.key, value.as_deref(), '=', flags)
            );
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
    filter: ValueFilterSpec,
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    // Compile any `--value` filter up front (invalid/over-long → exit 6) before
    // touching the DB.
    let value_filter = filter.compile()?;

    let count = if let Some(vf) = &value_filter {
        // Value-filtered unset: delete matching rows by id inside a transaction.
        // A default (non-`--unset-all`) filtered unset rejects an ambiguous (>1)
        // match set with exit 5; `--unset-all --value` removes every match.
        let conn = ScopedConfig::get_connection(scope)
            .await
            .map_err(CliError::from_legacy_string)?;
        let txn = conn
            .begin()
            .await
            .map_err(|e| config_write_cli_error("failed to begin config transaction", e))?;
        let removed = ConfigKv::unset_matching_with_conn(&txn, key, vf, !all)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("values exist") {
                    CliError::from_legacy_string(format!("error: {msg}")).with_exit_code(5)
                } else {
                    config_write_cli_error("failed to write config", e)
                }
            })?;
        txn.commit()
            .await
            .map_err(|e| config_write_cli_error("failed to commit config transaction", e))?;
        removed
    } else if all {
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

/// Reject generic section operations on protected vault namespaces.
fn reject_protected_vault_section(name: &str) -> CliResult<()> {
    if is_protected_vault_section(name) {
        return Err(CliError::command_usage(
            "vault sections must be managed by dedicated vault/config commands",
        ));
    }
    Ok(())
}

async fn handle_rename_section(
    old: &str,
    new: &str,
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    // Validate both section names (exit 1 on malformation) before any DB work.
    validate_section_syntax(old)
        .map_err(|e| CliError::from_legacy_string(format!("error: {e}")).with_exit_code(1))?;
    validate_section_syntax(new)
        .map_err(|e| CliError::from_legacy_string(format!("error: {e}")).with_exit_code(1))?;
    reject_protected_vault_section(old)?;
    reject_protected_vault_section(new)?;

    let old_prefix = format!("{old}.");
    let new_prefix = format!("{new}.");

    let conn = ScopedConfig::get_connection(scope)
        .await
        .map_err(CliError::from_legacy_string)?;
    let txn = conn
        .begin()
        .await
        .map_err(|e| config_write_cli_error("failed to begin config transaction", e))?;

    // Defence in depth: never move a vault-internal credential row, even if it
    // somehow lives under a non-vault section (e.g. a stray `*.privkey`).
    let source_rows = ConfigKv::get_by_prefix_with_conn(&txn, &old_prefix)
        .await
        .map_err(|e| {
            config_read_cli_error(format!("failed to read {} config: {e}", scope_name(scope)))
        })?;
    if source_rows.iter().any(|r| is_vault_internal_key(&r.key)) {
        return Err(CliError::command_usage(
            "vault sections must be managed by dedicated vault/config commands",
        ));
    }

    let moved = ConfigKv::rename_section_with_conn(&txn, &old_prefix, &new_prefix)
        .await
        .map_err(|e| {
            if e.to_string().contains("already exists") {
                CliError::failure(format!("target section '{new}' already exists"))
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
                    .with_exit_code(5)
            } else {
                config_write_cli_error("failed to rename config section", e)
            }
        })?;
    if moved == 0 {
        return Err(CliError::failure(format!("section '{old}' does not exist"))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_exit_code(5));
    }
    txn.commit()
        .await
        .map_err(|e| config_write_cli_error("failed to commit config transaction", e))?;

    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": "rename-section",
                "scope": scope_name(scope),
                "old": old,
                "new": new,
                "moved": moved,
            }),
            output,
        )?;
    } else if !output.quiet {
        println!(
            "Renamed {} section: {old} -> {new} ({moved} key{})",
            scope_name(scope),
            if moved == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

async fn handle_remove_section(
    section: &str,
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    validate_section_syntax(section)
        .map_err(|e| CliError::from_legacy_string(format!("error: {e}")).with_exit_code(1))?;
    reject_protected_vault_section(section)?;

    let prefix = format!("{section}.");

    let conn = ScopedConfig::get_connection(scope)
        .await
        .map_err(CliError::from_legacy_string)?;
    let txn = conn
        .begin()
        .await
        .map_err(|e| config_write_cli_error("failed to begin config transaction", e))?;

    let source_rows = ConfigKv::get_by_prefix_with_conn(&txn, &prefix)
        .await
        .map_err(|e| {
            config_read_cli_error(format!("failed to read {} config: {e}", scope_name(scope)))
        })?;
    if source_rows.iter().any(|r| is_vault_internal_key(&r.key)) {
        return Err(CliError::command_usage(
            "vault sections must be managed by dedicated vault/config commands",
        ));
    }

    let removed = ConfigKv::remove_section_with_conn(&txn, &prefix)
        .await
        .map_err(|e| config_write_cli_error("failed to remove config section", e))?;
    if removed == 0 {
        return Err(
            CliError::failure(format!("section '{section}' does not exist"))
                .with_stable_code(StableErrorCode::RepoStateInvalid)
                .with_exit_code(5),
        );
    }
    txn.commit()
        .await
        .map_err(|e| config_write_cli_error("failed to commit config transaction", e))?;

    if output.is_json() {
        emit_json_data(
            "config",
            &serde_json::json!({
                "action": "remove-section",
                "scope": scope_name(scope),
                "section": section,
                "removed": removed,
            }),
            output,
        )?;
    } else if !output.quiet {
        println!(
            "Removed {} section: {section} ({removed} key{})",
            scope_name(scope),
            if removed == 1 { "" } else { "s" }
        );
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
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    reject_global_key_generation(scope, "generate-ssh-key")?;

    // Validate remote name. config.md "generate-ssh-key" spec classifies
    // this as a CLI usage error (`error: invalid remote name '<name>': only
    // [a-zA-Z0-9_-] allowed`), so we must surface it via
    // `CliError::command_usage` (which maps to the `Cli` category → exit
    // 129 in coarse mode, 2 in fine mode) rather than the generic
    // `from_legacy_string` path that collapses to `Failure` / exit 128.
    if !remote
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || remote.is_empty()
        || remote.len() > 64
    {
        return Err(CliError::command_usage(format!(
            "invalid remote name '{remote}': only [a-zA-Z0-9_-] allowed, 1-64 chars"
        )));
    }

    // Verify remote exists. Missing remote is a Fatal failure (the user's
    // input is well-formed but the resource does not exist at execution
    // time), classified under the Repo category — exit 128 in coarse mode
    // matches the pre-existing behaviour from the legacy `from_legacy_string`
    // routing this branch used to follow.
    let remote_exists = ConfigKv::remote_config(remote)
        .await
        .map_err(|e| CliError::from_legacy_string(e.to_string()))?;
    if remote_exists.is_none() {
        return Err(CliError::failure(format!(
            "remote '{remote}' not found, add it first with libra remote add"
        ))
        .with_stable_code(StableErrorCode::RepoStateInvalid));
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
    scope: ConfigScope,
    output: &OutputConfig,
) -> CliResult<()> {
    reject_global_key_generation(scope, "generate-gpg-key")?;

    let usage = match usage.unwrap_or("signing") {
        "signing" => "signing",
        "encrypt" => "encrypt",
        other => {
            return Err(CliError::from_legacy_string(format!(
                "error: invalid value '{other}' for '--usage <KIND>' (expected 'signing' or 'encrypt')"
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

fn reject_global_key_generation(scope: ConfigScope, command: &str) -> CliResult<()> {
    if scope == ConfigScope::Local {
        return Ok(());
    }

    Err(CliError::command_usage(format!(
        "{command} only supports local scope; --global key generation is not supported yet"
    ))
    .with_hint("run without --global to generate a repository-local key"))
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
