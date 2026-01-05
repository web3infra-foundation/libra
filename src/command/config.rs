//! Config command for reading and writing settings across scopes, supporting key/value parsing and remote/branch associations.

use std::path::PathBuf;

use clap::Parser;

use crate::internal::config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Local,
    Global,
    System,
}

impl ConfigScope {
    /// Get the configuration file path for this scope
    pub fn get_config_path(&self) -> Option<PathBuf> {
        match self {
            ConfigScope::Local => {
                // Use the current repository's database (existing behavior)
                None // Will use the default database connection
            }
            ConfigScope::Global => {
                // Use ~/.libra/config.db for global configuration
                if let Some(home_dir) = dirs::home_dir() {
                    let global_config_dir = home_dir.join(".libra");
                    Some(global_config_dir.join("config.db"))
                } else {
                    eprintln!("warning: could not determine home directory for global config");
                    None
                }
            }
            ConfigScope::System => {
                // Use /etc/libra/config.db for system configuration
                Some(PathBuf::from("/etc/libra/config.db"))
            }
        }
    }

    /// Ensure the configuration directory and database exist for this scope
    pub async fn ensure_config_exists(&self) -> Result<(), String> {
        match self {
            ConfigScope::Local => {
                // Local config uses the repository database, which should already exist
                Ok(())
            }
            ConfigScope::Global => {
                if let Some(config_path) = self.get_config_path() {
                    if let Some(parent_dir) = config_path.parent() {
                        if !parent_dir.exists() {
                            std::fs::create_dir_all(parent_dir).map_err(|e| {
                                format!("Failed to create global config directory: {}", e)
                            })?;
                        }
                    }

                    if !config_path.exists() {
                        // Create the global config database
                        crate::internal::db::create_database(config_path.to_str().unwrap())
                            .await
                            .map_err(|e| {
                                format!("Failed to create global config database: {}", e)
                            })?;
                    }
                    Ok(())
                } else {
                    Err("Could not determine global config path".to_string())
                }
            }
            ConfigScope::System => {
                if let Some(config_path) = self.get_config_path() {
                    if let Some(parent_dir) = config_path.parent() {
                        if !parent_dir.exists() {
                            std::fs::create_dir_all(parent_dir).map_err(|e| {
                                format!(
                                    "Failed to create system config directory (may need sudo): {}",
                                    e
                                )
                            })?;
                        }
                    }

                    if !config_path.exists() {
                        // Create the system config database
                        crate::internal::db::create_database(config_path.to_str().unwrap())
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
    /// This is only valid when `get` is set.
    #[clap(long, short = 'd', requires = "get")]
    pub default: Option<String>,
}

impl ConfigArgs {
    pub fn validate(&self) -> Result<(), String> {
        // validate the default value is only present when get is set
        if self.default.is_some() && !(self.get || self.get_all) {
            return Err("default value is only valid when get (get_all) is set".to_string());
        }
        // validate that name_only is only valid when list is set
        if self.name_only && !self.list {
            return Err("--name-only is only valid when --list is set".to_string());
        }

        Ok(())
    }

    /// Get the configuration scope from the command line arguments
    pub fn get_scope(&self) -> ConfigScope {
        if self.global {
            ConfigScope::Global
        } else if self.system {
            ConfigScope::System
        } else {
            ConfigScope::Local // default
        }
    }
}

/// Configuration manager that handles different scopes
pub struct ScopedConfig;

impl ScopedConfig {
    /// Get a database connection for the specified scope
    async fn get_connection(scope: ConfigScope) -> Result<sea_orm::DatabaseConnection, String> {
        match scope {
            ConfigScope::Local => {
                // Use the existing repository database connection
                Ok(crate::internal::db::get_db_conn_instance().await.clone())
            }
            ConfigScope::Global | ConfigScope::System => {
                // Ensure the config exists first
                scope.ensure_config_exists().await?;

                if let Some(config_path) = scope.get_config_path() {
                    crate::internal::db::establish_connection(config_path.to_str().unwrap())
                        .await
                        .map_err(|e| {
                            format!(
                                "Failed to connect to {} config database: {}",
                                match scope {
                                    ConfigScope::Global => "global",
                                    ConfigScope::System => "system",
                                    ConfigScope::Local => "local",
                                },
                                e
                            )
                        })
                } else {
                    Err(format!(
                        "Could not determine config path for {:?} scope",
                        scope
                    ))
                }
            }
        }
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

pub struct Key {
    configuration: String,
    name: Option<String>,
    key: String,
}

pub async fn execute(args: ConfigArgs) {
    if let Err(e) = args.validate() {
        eprintln!("error: {e}");
        return;
    }

    let scope = args.get_scope();

    if args.list {
        list_config(args.name_only, scope).await;
    } else {
        let origin_key = args.key.unwrap();
        let key: Key = parse_key(origin_key).await;
        if args.add {
            add_config(&key, &args.valuepattern.unwrap(), scope).await;
        } else if args.get {
            get_config(
                &key,
                args.default.as_deref(),
                args.valuepattern.as_deref(),
                scope,
            )
            .await;
        } else if args.get_all {
            get_all_config(
                &key,
                args.default.as_deref(),
                args.valuepattern.as_deref(),
                scope,
            )
            .await;
        } else if args.unset {
            unset_config(&key, args.valuepattern.as_deref(), scope).await;
        } else if args.unset_all {
            unset_all_config(&key, args.valuepattern.as_deref(), scope).await;
        } else {
            // If none of the above flags are present, then default to setting a config
            set_config(&key, &args.valuepattern.unwrap(), scope).await;
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
async fn add_config(key: &Key, value: &str, scope: ConfigScope) {
    if let Err(e) = ScopedConfig::insert(
        scope,
        &key.configuration,
        key.name.as_deref(),
        &key.key,
        value,
    )
    .await
    {
        eprintln!("error: {}", e);
    }
}

/// Set a configuration entry by the given key and value (if old one is present, overwrites its value, otherwise create new one)
async fn set_config(key: &Key, value: &str, scope: ConfigScope) {
    // First, check whether given key has multiple values
    match ScopedConfig::get_all(scope, &key.configuration, key.name.as_deref(), &key.key).await {
        Ok(values) => {
            if values.len() >= 2 {
                eprintln!(
                    "warning: {}.{} has multiple values",
                    &key.configuration,
                    match &key.name {
                        Some(str) => str.to_string() + ".",
                        None => "".to_string(),
                    } + &key.key
                );
                eprintln!("error: cannot overwrite multiple values with a single value");
            } else if values.len() == 1 {
                if let Err(e) = ScopedConfig::update(
                    scope,
                    &key.configuration,
                    key.name.as_deref(),
                    &key.key,
                    value,
                )
                .await
                {
                    eprintln!("error: {}", e);
                }
            } else {
                if let Err(e) = ScopedConfig::insert(
                    scope,
                    &key.configuration,
                    key.name.as_deref(),
                    &key.key,
                    value,
                )
                .await
                {
                    eprintln!("error: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("error: {}", e);
        }
    }
}

/// Get the first configuration by the given key and value pattern
async fn get_config(
    key: &Key,
    default: Option<&str>,
    valuepattern: Option<&str>,
    scope: ConfigScope,
) {
    match ScopedConfig::get(scope, &key.configuration, key.name.as_deref(), &key.key).await {
        Ok(value) => {
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
                // if value is not exits just return the default value if it's present
                println!("{default_value}");
            }
        }
        Err(e) => {
            eprintln!("error: {}", e);
        }
    }
}

/// Get all the configurations by the given key and value pattern
async fn get_all_config(
    key: &Key,
    default: Option<&str>,
    valuepattern: Option<&str>,
    scope: ConfigScope,
) {
    match ScopedConfig::get_all(scope, &key.configuration, key.name.as_deref(), &key.key).await {
        Ok(values) => {
            let mut matched_any = false;
            for value in values {
                if let Some(vp) = valuepattern {
                    // for each value, check if it matches the pattern
                    if value.contains(vp) {
                        println!("{value}");
                        matched_any = true;
                    }
                } else {
                    // print all if value pattern is not present
                    matched_any = true;
                    println!("{value}");
                }
            }
            if !matched_any && let Some(default_value) = default {
                // if no value matches the pattern, print the default value if it's present
                println!("{default_value}");
            }
        }
        Err(e) => {
            eprintln!("error: {}", e);
        }
    }
}

/// Remove one configuration by given key and value pattern
async fn unset_config(key: &Key, valuepattern: Option<&str>, scope: ConfigScope) {
    if let Err(e) = ScopedConfig::remove_config(
        scope,
        &key.configuration,
        key.name.as_deref(),
        &key.key,
        valuepattern,
        false,
    )
    .await
    {
        eprintln!("error: {}", e);
    }
}

/// Remove all configurations by given key and value pattern
async fn unset_all_config(key: &Key, valuepattern: Option<&str>, scope: ConfigScope) {
    if let Err(e) = ScopedConfig::remove_config(
        scope,
        &key.configuration,
        key.name.as_deref(),
        &key.key,
        valuepattern,
        true,
    )
    .await
    {
        eprintln!("error: {}", e);
    }
}

/// List all configurations
async fn list_config(name_only: bool, scope: ConfigScope) {
    match ScopedConfig::list_all(scope).await {
        Ok(configurations) => {
            for (key, value) in configurations {
                // If name_only is set, only print the key string
                // Otherwise, print the key=value pair
                if name_only {
                    println!("{key}");
                } else {
                    println!("{key}={value}");
                }
            }
        }
        Err(e) => {
            eprintln!("error: {}", e);
        }
    }
}
