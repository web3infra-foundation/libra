//! Config command for reading and writing settings across scopes, supporting key/value parsing and remote/branch associations.

use std::path::PathBuf;

use clap::Parser;
use once_cell::sync::Lazy;
use sea_orm::DatabaseConnection;
use tokio::sync::Mutex;

use crate::internal::config;

/// Cached database connection for Global scope, paired with the resolved DB path.
///
/// We cache the connection to avoid reconnect overhead, but we must invalidate it if
/// the resolved path changes (e.g., tests override `LIBRA_CONFIG_GLOBAL_DB`).
static GLOBAL_CONFIG_CONN: Lazy<Mutex<Option<(PathBuf, DatabaseConnection)>>> =
    Lazy::new(|| Mutex::new(None));

/// Cached database connection for System scope, paired with the resolved DB path.
///
/// We cache the connection to avoid reconnect overhead, but we must invalidate it if
/// the resolved path changes (e.g., tests override `LIBRA_CONFIG_SYSTEM_DB`).
static SYSTEM_CONFIG_CONN: Lazy<Mutex<Option<(PathBuf, DatabaseConnection)>>> =
    Lazy::new(|| Mutex::new(None));

/// Configuration scope that determines where configuration values are stored and retrieved from.
///
/// This enum defines the three levels of configuration storage, following Git's configuration
/// hierarchy model. Each scope has its own database file and isolation boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    /// Repository-specific configuration stored in the current repository's `.libra` directory.
    ///
    /// This is the default scope when no explicit scope is specified. Configuration values
    /// set at this level only affect the current repository and are stored in:
    /// - `<repository>/.libra/libra.db`
    ///
    /// Local configuration has the highest precedence and overrides global and system settings.
    Local,

    /// User-specific configuration stored in the user's home directory.
    ///
    /// Configuration values set at this level affect all repositories for the current user
    /// and are stored in:
    /// - Unix/Linux/macOS: `~/.libra/config.db`
    /// - Windows: `%USERPROFILE%\.libra\config.db`
    ///
    /// Global configuration has medium precedence and overrides system settings but is
    /// overridden by local settings.
    Global,

    /// System-wide configuration stored in a system directory.
    ///
    /// Configuration values set at this level affect all users and repositories on the system.
    /// Requires administrative privileges to modify and is stored in:
    /// - Unix/Linux/macOS: `/etc/libra/config.db`
    /// - Windows: `%PROGRAMDATA%\libra\config.db`
    ///
    /// System configuration has the lowest precedence and is overridden by both global
    /// and local settings.
    System,
}

impl ConfigScope {
    /// The cascade order for configuration lookup (highest to lowest precedence).
    ///
    /// When no explicit scope is specified for read operations, configuration values
    /// are searched in this order. The first scope that contains the requested key
    /// will provide the value, following Git's configuration precedence model.
    pub const CASCADE_ORDER: [ConfigScope; 3] =
        [ConfigScope::Local, ConfigScope::Global, ConfigScope::System];

    /// Get the configuration file path for this scope.
    ///
    /// Returns the absolute path where the configuration database should be stored
    /// for this scope. The path is platform-specific and follows standard conventions.
    ///
    /// Test/CI can override the location using:
    /// - `LIBRA_CONFIG_GLOBAL_DB` for `Global`
    /// - `LIBRA_CONFIG_SYSTEM_DB` for `System`
    ///
    /// # Returns
    ///
    /// - `Some(PathBuf)` - The path to the configuration database file
    /// - `None` - For `Local` scope (uses repository database), or when a path
    ///   cannot be determined (e.g. missing home directory, unsupported
    ///   platform, or invalid environment configuration)
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use libra::command::config::ConfigScope;
    ///
    /// assert_eq!(ConfigScope::Local.get_config_path(), None);
    /// let _ = ConfigScope::Global.get_config_path();
    /// let _ = ConfigScope::System.get_config_path();
    /// ```
    pub fn get_config_path(&self) -> Option<PathBuf> {
        match self {
            ConfigScope::Local => None,
            ConfigScope::Global => {
                if let Some(p) = std::env::var_os("LIBRA_CONFIG_GLOBAL_DB") {
                    return Some(PathBuf::from(p));
                }

                dirs::home_dir().map(|home_dir| home_dir.join(".libra").join("config.db"))
            }
            ConfigScope::System => {
                if let Some(p) = std::env::var_os("LIBRA_CONFIG_SYSTEM_DB") {
                    return Some(PathBuf::from(p));
                }

                #[cfg(unix)]
                {
                    Some(PathBuf::from("/etc/libra/config.db"))
                }
                #[cfg(windows)]
                {
                    std::env::var_os("PROGRAMDATA").and_then(|path| {
                        let base = PathBuf::from(path);
                        if !base.is_absolute() {
                            // Reject non-absolute PROGRAMDATA values
                            return None;
                        }
                        Some(base.join("libra").join("config.db"))
                    })
                }
                #[cfg(not(any(unix, windows)))]
                {
                    None
                }
            }
        }
    }

    /// Ensure the configuration directory and database exist for this scope.
    ///
    /// Creates the necessary directory structure and initializes the configuration database
    /// if it doesn't already exist. This method handles the setup required before any
    /// configuration operations can be performed.
    ///
    /// # Returns
    ///
    /// - `Ok(())` - Configuration database is ready for use
    /// - `Err(String)` - Failed to create directory or database, with error description
    ///
    /// # Errors
    ///
    /// This method can fail in several scenarios:
    /// - Insufficient permissions to create directories or files
    /// - Disk space issues
    /// - Invalid or inaccessible paths
    /// - Database initialization failures
    ///
    /// For System scope, this typically requires administrative privileges.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use libra::command::config::ConfigScope;
    ///
    /// # async fn example() -> Result<(), String> {
    /// // Ensure global config is ready
    /// ConfigScope::Global.ensure_config_exists().await?;
    ///
    /// // Now we can safely perform config operations
    /// # Ok(())
    /// # }
    /// ```
    pub async fn ensure_config_exists(&self) -> Result<(), String> {
        match self {
            ConfigScope::Local => {
                // Local config uses the repository database, which should already exist
                Ok(())
            }
            ConfigScope::Global => {
                if let Some(config_path) = self.get_config_path() {
                    if let Some(parent_dir) = config_path.parent()
                        && !parent_dir.exists()
                    {
                        std::fs::create_dir_all(parent_dir).map_err(|e| {
                            format!("Failed to create global config directory: {}", e)
                        })?;
                    }

                    if !config_path.exists() {
                        // Create the global config database
                        let config_path_str = config_path.to_string_lossy();
                        crate::internal::db::create_database(&config_path_str)
                            .await
                            .map_err(|e| {
                                format!("Failed to create global config database: {}", e)
                            })?;
                    }
                    Ok(())
                } else {
                    Err(
                        "Could not determine global config path: home directory not available"
                            .to_string(),
                    )
                }
            }
            ConfigScope::System => {
                if let Some(config_path) = self.get_config_path() {
                    if let Some(parent_dir) = config_path.parent()
                        && !parent_dir.exists()
                    {
                        std::fs::create_dir_all(parent_dir).map_err(|e| {
                            format!(
                                "Failed to create system config directory (may need sudo): {}",
                                e
                            )
                        })?;
                    }

                    if !config_path.exists() {
                        // Create the system config database
                        let config_path_str = config_path.to_string_lossy();
                        crate::internal::db::create_database(&config_path_str)
                            .await
                            .map_err(|e| {
                                format!(
                                    "Failed to create system config database (may need sudo): {}",
                                    e
                                )
                            })?;
                    }
                    Ok(())
                } else {
                    Err("Could not determine system config path".to_string())
                }
            }
        }
    }
}

/// Command-line arguments for the config command.
///
/// This structure defines all the possible options and flags that can be used with
/// the config command, following Git's config command interface closely.
///
/// # Scope Selection
///
/// Only one scope flag should be specified at a time:
/// - `--local`: Repository-specific configuration (default)
/// - `--global`: User-specific configuration
/// - `--system`: System-wide configuration
///
/// # Operation Modes
///
/// The command supports several mutually exclusive operation modes:
/// - `--add`: Add a new configuration entry (allows duplicates)
/// - `--get`: Get the first matching configuration value
/// - `--get-all`: Get all matching configuration values
/// - `--unset`: Remove the first matching configuration entry
/// - `--unset-all`: Remove all matching configuration entries
/// - `--list`: List all configuration entries
/// - Default (no mode): Set configuration value (update if exists, create if not)
#[derive(Parser, Debug)]
pub struct ConfigArgs {
    /// Add a configuration entry to database
    #[clap(long, group("mode"), requires("valuepattern"))]
    pub add: bool,
    /// Get a single configuration entry that satisfied key and value pattern from database
    #[clap(long, group("mode"))]
    pub get: bool,
    /// Get all configuration entries that satisfied key and value pattern from database
    #[clap(long("get-all"), group("mode"))]
    pub get_all: bool,
    /// Remove a single configuration entry from database
    #[clap(long, group("mode"))]
    pub unset: bool,
    /// Remove all the configuration entries that satisfied key and valuepattern from database
    #[clap(long("unset-all"), group("mode"))]
    pub unset_all: bool,
    /// List all the configuration entries from database
    #[clap(long, short, group("mode"))]
    pub list: bool,
    /// If set, only print the key string of the configuration entry instead of the key=value.
    /// This is only valid when `list` is set.
    #[clap(long("name-only"), requires = "list")]
    pub name_only: bool,
    /// Use repository config file only (default)
    #[clap(long, group("scope"))]
    pub local: bool,
    /// Use global config file
    #[clap(long, group("scope"))]
    pub global: bool,
    /// Use system config file
    #[clap(long, group("scope"))]
    pub system: bool,
    /// The key string of the configuration entry, should be like configuration.[name].key
    #[clap(value_name("key"), required_unless_present("list"))]
    pub key: Option<String>,
    /// the value or the possible value pattern of the configuration entry
    #[clap(value_name("value_pattern"), required_unless_present("mode"))]
    pub valuepattern: Option<String>,
    /// If the target key is not present, return the given default value.
    /// This is only valid when `get` or `get-all` is set.
    #[clap(long, short = 'd')]
    pub default: Option<String>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn default_works_with_get_all() {
        let args = ConfigArgs::try_parse_from([
            "config",
            "--get-all",
            "-d",
            "fallback",
            "user.name",
        ])
        .unwrap();

        assert!(args.get_all);
        assert_eq!(args.default.as_deref(), Some("fallback"));
    }
}

impl ConfigArgs {
    pub fn validate(&self) -> Result<(), String> {
        // validate the default value is only present when get or get_all is set
        if self.default.is_some() && !(self.get || self.get_all) {
            return Err("default value is only valid when get (get_all) is set".to_string());
        }
        // validate that name_only is only valid when list is set
        if self.name_only && !self.list {
            return Err("--name-only is only valid when --list is set".to_string());
        }

        Ok(())
    }

    /// Get the configuration scope from the command line arguments.
    ///
    /// Determines which configuration scope should be used based on the scope flags.
    /// If no explicit scope is specified, defaults to Local scope.
    ///
    /// # Returns
    ///
    /// - `ConfigScope::Local` - Default when no scope flags are set, or when `--local` is specified
    /// - `ConfigScope::Global` - When `--global` flag is specified
    /// - `ConfigScope::System` - When `--system` flag is specified
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use libra::command::config::{ConfigArgs, ConfigScope};
    ///
    /// let args = ConfigArgs {
    ///     add: false,
    ///     get: false,
    ///     get_all: false,
    ///     unset: false,
    ///     unset_all: false,
    ///     list: false,
    ///     name_only: false,
    ///     local: false,
    ///     global: true,
    ///     system: false,
    ///     key: None,
    ///     valuepattern: None,
    ///     default: None,
    /// };
    ///
    /// assert_eq!(args.get_scope(), ConfigScope::Global);
    /// ```
    pub fn get_scope(&self) -> ConfigScope {
        if self.global {
            ConfigScope::Global
        } else if self.system {
            ConfigScope::System
        } else {
            ConfigScope::Local // default
        }
    }

    /// Returns true if any of `--local`, `--global`, or `--system` was explicitly provided.
    pub fn has_explicit_scope(&self) -> bool {
        self.local || self.global || self.system
    }
}

/// Configuration manager that handles different configuration scopes (Local/Global/System).
///
/// # Architecture overview
///
/// Libra stores configuration entries in SQLite databases. This command supports multiple
/// scopes that map to different database files:
///
/// - **Local**: repository database (existing behavior; repo-specific)
/// - **Global**: user-level database (applies across repos)
/// - **System**: system-level database (applies across users/repos; may require elevated privileges)
///
/// # Read semantics (Git-compatible precedence)
///
/// For read-oriented operations (e.g. `get`, `get-all`, `list`) when **no explicit scope flag**
/// (`--local/--global/--system`) is provided, the command uses a cascading lookup order:
///
/// `Local → Global → System`
///
/// This matches Git’s precedence model: local overrides global, which overrides system.
///
/// When an explicit scope flag is provided, the operation targets that single scope only.
///
/// # Write semantics
///
/// For write-oriented operations (e.g. `add`, default `set`, `unset`, `unset-all`), the command
/// always targets the selected scope. When no scope flag is provided, the default write scope is
/// **Local** (repository database).
///
/// # Connection management
///
/// Global/System operations may require opening a separate database connection. Implementations
/// may cache those connections to reduce reconnect overhead, but must invalidate caches if the
/// resolved database path changes (e.g., via test/CI overrides).
pub struct ScopedConfig;

impl ScopedConfig {
    /// Get a database connection for the specified scope
    async fn get_connection(scope: ConfigScope) -> Result<DatabaseConnection, String> {
        match scope {
            ConfigScope::Local => {
                // Use the existing repository database connection
                Ok(crate::internal::db::get_db_conn_instance().await.clone())
            }
            ConfigScope::Global => {
                Self::get_or_create_cached_connection(&GLOBAL_CONFIG_CONN, scope, "global").await
            }
            ConfigScope::System => {
                Self::get_or_create_cached_connection(&SYSTEM_CONFIG_CONN, scope, "system").await
            }
        }
    }

    /// Get or create a cached database connection for the given scope.
    ///
    /// If the resolved config DB path changes (e.g., due to env overrides in tests),
    /// the cached connection is invalidated and rebuilt.
    async fn get_or_create_cached_connection(
        cache: &Lazy<Mutex<Option<(PathBuf, DatabaseConnection)>>>,
        scope: ConfigScope,
        scope_name: &str,
    ) -> Result<DatabaseConnection, String> {
        // Resolve path first so we can validate/invalidate cache deterministically.
        let Some(config_path) = scope.get_config_path() else {
            return Err(format!(
                "Could not determine config path for {:?} scope",
                scope
            ));
        };

        let mut guard = cache.lock().await;

        // Return cached connection if available and path matches
        if let Some((cached_path, cached_conn)) = guard.as_ref() {
            if cached_path == &config_path {
                return Ok(cached_conn.clone());
            }
            // Path changed: invalidate cached connection.
            *guard = None;
        }

        // Ensure the config exists first
        scope.ensure_config_exists().await?;

        let config_path_str = config_path.to_string_lossy();
        let conn = crate::internal::db::establish_connection(&config_path_str)
            .await
            .map_err(|e| format!("Failed to connect to {} config database: {}", scope_name, e))?;

        // Cache the connection for future use
        *guard = Some((config_path, conn.clone()));
        Ok(conn)
    }

    /// Insert configuration with scope
    pub async fn insert(
        scope: ConfigScope,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let conn = Self::get_connection(scope).await?;
        config::Config::insert_with_conn(&conn, configuration, name, key, value).await;
        Ok(())
    }

    /// Update configuration with scope
    pub async fn update(
        scope: ConfigScope,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let conn = Self::get_connection(scope).await?;
        config::Config::update_with_conn(&conn, configuration, name, key, value).await;
        Ok(())
    }

    /// Get configuration with scope
    pub async fn get(
        scope: ConfigScope,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Result<Option<String>, String> {
        let conn = Self::get_connection(scope).await?;
        Ok(config::Config::get_with_conn(&conn, configuration, name, key).await)
    }

    /// Get all configurations with scope
    pub async fn get_all(
        scope: ConfigScope,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Result<Vec<String>, String> {
        let conn = Self::get_connection(scope).await?;
        Ok(config::Config::get_all_with_conn(&conn, configuration, name, key).await)
    }

    /// List all configurations with scope
    pub async fn list_all(scope: ConfigScope) -> Result<Vec<(String, String)>, String> {
        let conn = Self::get_connection(scope).await?;
        Ok(config::Config::list_all_with_conn(&conn).await)
    }

    /// Remove configuration with scope
    pub async fn remove_config(
        scope: ConfigScope,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        valuepattern: Option<&str>,
        delete_all: bool,
    ) -> Result<(), String> {
        let conn = Self::get_connection(scope).await?;
        config::Config::remove_config_with_conn(
            &conn,
            configuration,
            name,
            key,
            valuepattern,
            delete_all,
        )
        .await;
        Ok(())
    }
}

/// Parsed configuration key broken into `configuration`, optional `name`, and
/// leaf `key` components.
pub struct Key {
    configuration: String,
    name: Option<String>,
    key: String,
}

/// Execute the `config` command using parsed CLI arguments, printing any error
/// to stderr instead of bubbling it up to the caller.
pub async fn execute(args: ConfigArgs) {
    if let Err(e) = execute_impl(args).await {
        eprintln!("error: {e}");
    }
}

/// Internal implementation that returns Result for better error handling
async fn execute_impl(args: ConfigArgs) -> Result<(), String> {
    args.validate()?;

    let scope = args.get_scope();
    let use_cascade = !args.has_explicit_scope();

    if args.list {
        list_config(args.name_only, scope, use_cascade).await
    } else {
        let origin_key = args.key.unwrap();
        let key: Key = parse_key(origin_key).await;
        if args.add {
            add_config(&key, &args.valuepattern.unwrap(), scope).await
        } else if args.get {
            get_config(
                &key,
                args.default.as_deref(),
                args.valuepattern.as_deref(),
                scope,
                use_cascade,
            )
            .await
        } else if args.get_all {
            get_all_config(
                &key,
                args.default.as_deref(),
                args.valuepattern.as_deref(),
                scope,
                use_cascade,
            )
            .await
        } else if args.unset {
            unset_config(&key, args.valuepattern.as_deref(), scope).await
        } else if args.unset_all {
            unset_all_config(&key, args.valuepattern.as_deref(), scope).await
        } else {
            // If none of the above flags are present, then default to setting a config
            set_config(&key, &args.valuepattern.unwrap(), scope).await
        }
    }
}

/// Parse the original key string to three fields: configuration, name and key
/// The parsing strategy for the three parameters configuration, name, and key is as follows:
/// If the original key parameter string does not contain a . symbol, an error is directly raised.
/// If the original key parameter string contains exactly one . symbol, the entire key parameter string is parsed as configuration.key.
/// If the original key parameter string contains more than one . symbol, the entire key parameter string is parsed as configuration.name.key, where the two . symbols correspond to the first . and the last . in the original parameter string.
async fn parse_key(mut origin_key: String) -> Key {
    let configuration: String;
    let name: Option<String>;
    (configuration, origin_key) = match origin_key.split_once(".") {
        Some((first_part, remainer)) => (first_part.to_string(), remainer.to_string()),
        None => {
            panic!("error: key does not contain a section: {origin_key}");
        }
    };
    (name, origin_key) = match origin_key.rsplit_once(".") {
        Some((first_part, remainer)) => (Some(first_part.to_string()), remainer.to_string()),
        None => (None, origin_key),
    };
    let key: String = origin_key;
    Key {
        configuration,
        name,
        key,
    }
}

/// Add a configuration entry by the given key and value (create new one no matter old one is present or not)
async fn add_config(key: &Key, value: &str, scope: ConfigScope) -> Result<(), String> {
    ScopedConfig::insert(
        scope,
        &key.configuration,
        key.name.as_deref(),
        &key.key,
        value,
    )
    .await
}

/// Set a configuration entry by the given key and value (if old one is present, overwrites its value, otherwise create new one)
async fn set_config(key: &Key, value: &str, scope: ConfigScope) -> Result<(), String> {
    // First, check whether given key has multiple values
    let values =
        ScopedConfig::get_all(scope, &key.configuration, key.name.as_deref(), &key.key).await?;

    if values.len() >= 2 {
        Err(format!(
            "warning: {}.{} has multiple values\nerror: cannot overwrite multiple values with a single value",
            &key.configuration,
            match &key.name {
                Some(str) => str.to_string() + ".",
                None => "".to_string(),
            } + &key.key
        ))
    } else if values.len() == 1 {
        ScopedConfig::update(
            scope,
            &key.configuration,
            key.name.as_deref(),
            &key.key,
            value,
        )
        .await
    } else {
        ScopedConfig::insert(
            scope,
            &key.configuration,
            key.name.as_deref(),
            &key.key,
            value,
        )
        .await
    }
}

/// Get the first configuration by the given key and value pattern
async fn get_config(
    key: &Key,
    default: Option<&str>,
    valuepattern: Option<&str>,
    scope: ConfigScope,
    use_cascade: bool,
) -> Result<(), String> {
    let value = if use_cascade {
        get_config_cascaded(&key.configuration, key.name.as_deref(), &key.key).await?
    } else {
        ScopedConfig::get(scope, &key.configuration, key.name.as_deref(), &key.key).await?
    };

    if let Some(v) = value {
        if let Some(vp) = valuepattern {
            // if value pattern is present, check it
            if v.contains(vp) {
                println!("{v}");
            }
        } else {
            // if value pattern is not present, just print it
            println!("{v}");
        }
    } else if let Some(default_value) = default {
        // if value does not exist just return the default value if it's present
        println!("{default_value}");
    }

    Ok(())
}

/// Get all the configurations by the given key and value pattern
async fn get_all_config(
    key: &Key,
    default: Option<&str>,
    valuepattern: Option<&str>,
    scope: ConfigScope,
    use_cascade: bool,
) -> Result<(), String> {
    let values = if use_cascade {
        get_all_config_cascaded(&key.configuration, key.name.as_deref(), &key.key).await?
    } else {
        ScopedConfig::get_all(scope, &key.configuration, key.name.as_deref(), &key.key).await?
    };

    let mut matched_any = false;
    for value in values {
        if let Some(vp) = valuepattern {
            if value.contains(vp) {
                println!("{value}");
                matched_any = true;
            }
        } else {
            matched_any = true;
            println!("{value}");
        }
    }
    if !matched_any && let Some(default_value) = default {
        println!("{default_value}");
    }

    Ok(())
}

/// Get the first matching configuration value using `CASCADE_ORDER`
/// (`Local → Global → System`), skipping scopes whose backing storage is
/// missing or invalid. Errors from individual scopes are ignored so that a
/// later scope can still satisfy the lookup.
async fn get_config_cascaded(
    configuration: &str,
    name: Option<&str>,
    key: &str,
) -> Result<Option<String>, String> {
    for scope in ConfigScope::CASCADE_ORDER {
        if scope != ConfigScope::Local {
            let Some(path) = scope.get_config_path() else {
                continue;
            };
            if !path.exists() {
                continue;
            }
        }

        match ScopedConfig::get(scope, configuration, name, key).await {
            Ok(Some(v)) => return Ok(Some(v)),
            Ok(None) => continue,
            Err(_) => continue,
        }
    }
    Ok(None)
}

/// Get all configuration values for a key across every scope in
/// `CASCADE_ORDER`, skipping scopes whose backing storage is missing or
/// invalid. Values from all scopes are appended in precedence order.
async fn get_all_config_cascaded(
    configuration: &str,
    name: Option<&str>,
    key: &str,
) -> Result<Vec<String>, String> {
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

        if let Ok(mut v) = ScopedConfig::get_all(scope, configuration, name, key).await {
            out.append(&mut v);
        }
    }
    Ok(out)
}

/// Remove one configuration by given key and value pattern
async fn unset_config(
    key: &Key,
    valuepattern: Option<&str>,
    scope: ConfigScope,
) -> Result<(), String> {
    ScopedConfig::remove_config(
        scope,
        &key.configuration,
        key.name.as_deref(),
        &key.key,
        valuepattern,
        false,
    )
    .await
}

/// Remove all configurations by given key and value pattern
async fn unset_all_config(
    key: &Key,
    valuepattern: Option<&str>,
    scope: ConfigScope,
) -> Result<(), String> {
    ScopedConfig::remove_config(
        scope,
        &key.configuration,
        key.name.as_deref(),
        &key.key,
        valuepattern,
        true,
    )
    .await
}

/// List all configurations
async fn list_config(name_only: bool, scope: ConfigScope, use_cascade: bool) -> Result<(), String> {
    let configurations = if use_cascade {
        list_all_config_cascaded().await?
    } else {
        ScopedConfig::list_all(scope).await?
    };

    for (key, value) in configurations {
        if name_only {
            println!("{key}");
        } else {
            println!("{key}={value}");
        }
    }

    Ok(())
}

/// List an effective, precedence-aware view of all configuration entries
/// merged across scopes. Lower precedence entries are loaded first so that
/// higher precedence scopes can overwrite them, then the result is returned
/// sorted by key.
async fn list_all_config_cascaded() -> Result<Vec<(String, String)>, String> {
    use std::collections::HashMap;

    let mut merged: HashMap<String, String> = HashMap::new();

    // Iterate low->high precedence so higher precedence overwrites.
    for scope in ConfigScope::CASCADE_ORDER.iter().rev() {
        if *scope != ConfigScope::Local {
            let Some(path) = scope.get_config_path() else {
                continue;
            };
            if !path.exists() {
                continue;
            }
        }

        if let Ok(entries) = ScopedConfig::list_all(*scope).await {
            for (k, v) in entries {
                merged.insert(k, v);
            }
        }
    }

    let mut out: Vec<(String, String)> = merged.into_iter().collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}
