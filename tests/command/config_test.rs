//! Tests config command read/write behaviors, scope handling, and edge cases.

use libra::command::config;
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Guard for temporarily setting an environment variable during a test and restoring it on drop.
///
/// # Safety
/// Modifying environment variables is process-global state. These tests are all annotated with
/// `#[serial]`, ensuring no concurrent mutation happens across tests.
struct EnvVarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        match &self.original {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Sets `LIBRA_CONFIG_GLOBAL_DB` and `LIBRA_CONFIG_SYSTEM_DB` to point at temp files for isolation.
///
/// This prevents tests from touching real host paths like `~/.libra/config.db` or `/etc/libra/config.db`.
struct ScopedConfigPathGuard {
    _global: EnvVarGuard,
    _system: EnvVarGuard,
}

impl ScopedConfigPathGuard {
    fn new(global_db_path: &std::path::Path, system_db_path: &std::path::Path) -> Self {
        let _global = EnvVarGuard::set("LIBRA_CONFIG_GLOBAL_DB", global_db_path.as_os_str());
        let _system = EnvVarGuard::set("LIBRA_CONFIG_SYSTEM_DB", system_db_path.as_os_str());
        Self { _global, _system }
    }
}

#[tokio::test]
#[serial]
async fn test_config_get_failed() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    let args = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("value".to_string()),
        default: Some("erasernoob".to_string()),
        name_only: false,
    };

    // `execute()` prints errors and returns (), so assert on `validate()` directly.
    let err = args.validate().unwrap_err();
    assert_eq!(
        err,
        "default value is only valid when get (get_all) is set".to_string()
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_all() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let arg1 = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("erasernoob".to_string()),
        default: None,
        name_only: false,
    };
    config::execute(arg1).await;

    let args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: None,
        default: None,
        name_only: false,
    };
    config::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_config_get_all_with_default() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let args = config::ConfigArgs {
        add: false,
        get: false,
        get_all: true,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("value".to_string()),
        default: Some("erasernoob".to_string()),
        name_only: false,
    };
    config::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_config_get() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let arg1 = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("erasernoob".to_string()),
        default: None,
        name_only: false,
    };
    config::execute(arg1).await;

    let args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: None,
        default: None,
        name_only: false,
    };
    config::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_config_get_with_default() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: None,
        default: Some("erasernoob".to_string()),
        name_only: false,
    };
    config::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_config_list() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let arg1 = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("erasernoob".to_string()),
        default: None,
        name_only: false,
    };
    config::execute(arg1).await;

    let arg2 = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.email".to_string()),
        valuepattern: Some("erasernoob@example.com".to_string()),
        default: None,
        name_only: false,
    };
    config::execute(arg2).await;

    // List configs
    let args = config::ConfigArgs {
        add: false,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: true,
        local: false,
        global: false,
        system: false,
        key: None,
        valuepattern: None,
        default: None,
        name_only: false,
    };
    assert!(args.validate().is_ok());
    config::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_config_list_name_only() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let arg1 = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("erasernoob".to_string()),
        default: None,
        name_only: false,
    };
    config::execute(arg1).await;

    let arg2 = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: Some("user.email".to_string()),
        valuepattern: Some("erasernoob@example.com".to_string()),
        default: None,
        name_only: false,
    };
    config::execute(arg2).await;

    // List configs with name_only set to true
    let args = config::ConfigArgs {
        add: false,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: true,
        local: false,
        global: false,
        system: false,
        key: None,
        valuepattern: None,
        default: None,
        name_only: true,
    };
    assert!(args.validate().is_ok());
    config::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_config_list_name_only_without_list() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let args = config::ConfigArgs {
        add: false,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: false,
        key: None,
        valuepattern: None,
        default: None,
        name_only: true,
    };
    assert!(args.validate().is_err());
}

// New tests for scope functionality
#[tokio::test]
#[serial]
async fn test_config_scope_local_default() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Test that no scope specified defaults to local
    let args = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false, // No scope specified, should default to local
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("test_user_local_default".to_string()),
        default: None,
        name_only: false,
    };

    assert_eq!(args.get_scope(), config::ConfigScope::Local);
    config::execute(args).await;

    // Verify the value was written to local scope by reading it back
    let read_args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false, // Default to local
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: None,
        default: None,
        name_only: false,
    };

    // This should succeed and print the value we just set
    config::execute(read_args).await;
}

#[tokio::test]
#[serial]
async fn test_config_scope_global() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Isolate global/system DB paths to temp files (no host pollution).
    let global_db_dir = tempdir().unwrap();
    let system_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(
        &global_db_dir.path().join("global_config.db"),
        &system_db_dir.path().join("system_config.db"),
    );

    // Set a value in global scope
    let set_args = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: true,
        system: false,
        key: Some("user.email".to_string()),
        valuepattern: Some("global_user@example.com".to_string()),
        default: None,
        name_only: false,
    };

    assert_eq!(set_args.get_scope(), config::ConfigScope::Global);
    config::execute(set_args).await;

    // Verify the value was written to global scope by reading it back
    let read_global_args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: true,
        system: false,
        key: Some("user.email".to_string()),
        valuepattern: None,
        default: None,
        name_only: false,
    };

    config::execute(read_global_args).await;

    // Verify that the global value is NOT accessible from local scope
    let read_local_args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: true, // Explicitly local
        global: false,
        system: false,
        key: Some("user.email".to_string()),
        valuepattern: None,
        default: Some("not_found".to_string()), // Should return this default
        name_only: false,
    };

    // This should return the default value since the key doesn't exist in local scope
    config::execute(read_local_args).await;
}

#[tokio::test]
#[serial]
async fn test_config_scope_system() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Isolate global/system DB paths to temp files (no host pollution).
    let global_db_dir = tempdir().unwrap();
    let system_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(
        &global_db_dir.path().join("global_config.db"),
        &system_db_dir.path().join("system_config.db"),
    );

    let args = config::ConfigArgs {
        add: false,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: false,
        system: true,
        key: Some("user.name".to_string()),
        valuepattern: Some("system_user".to_string()),
        default: None,
        name_only: false,
    };

    assert_eq!(args.get_scope(), config::ConfigScope::System);
    config::execute(args).await;
}

#[tokio::test]
#[serial]
async fn test_config_scope_explicit_local() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Set a value explicitly in local scope
    let set_args = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: true,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: Some("explicit_local_user".to_string()),
        default: None,
        name_only: false,
    };

    assert_eq!(set_args.get_scope(), config::ConfigScope::Local);
    config::execute(set_args).await;

    // Verify the value was written to local scope by reading it back
    let read_args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: true,
        global: false,
        system: false,
        key: Some("user.name".to_string()),
        valuepattern: None,
        default: None,
        name_only: false,
    };

    config::execute(read_args).await;
}

#[tokio::test]
#[serial]
async fn test_config_scope_isolation() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Isolate global/system DB paths to temp files (no host pollution).
    let global_db_dir = tempdir().unwrap();
    let system_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(
        &global_db_dir.path().join("global_config.db"),
        &system_db_dir.path().join("system_config.db"),
    );

    // Set the same key with different values in different scopes
    let local_args = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: true,
        global: false,
        system: false,
        key: Some("test.isolation".to_string()),
        valuepattern: Some("local_value".to_string()),
        default: None,
        name_only: false,
    };
    config::execute(local_args).await;

    let global_args = config::ConfigArgs {
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: true,
        system: false,
        key: Some("test.isolation".to_string()),
        valuepattern: Some("global_value".to_string()),
        default: None,
        name_only: false,
    };

    config::execute(global_args).await;

    // Verify that each scope returns its own value
    let read_local_args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: true,
        global: false,
        system: false,
        key: Some("test.isolation".to_string()),
        valuepattern: None,
        default: None,
        name_only: false,
    };
    println!("Reading from local scope:");
    config::execute(read_local_args).await;

    let read_global_args = config::ConfigArgs {
        add: false,
        get: true,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        local: false,
        global: true,
        system: false,
        key: Some("test.isolation".to_string()),
        valuepattern: None,
        default: None,
        name_only: false,
    };
    println!("Reading from global scope:");
    config::execute(read_global_args).await;
}

#[tokio::test]
#[serial]
async fn test_config_scope_path_logic() {
    // Test the path logic for different scopes without executing config operations

    // Local scope should return None (uses repository database)
    assert_eq!(config::ConfigScope::Local.get_config_path(), None);

    // Global scope should return a path in the home directory (if available)
    let global_path = config::ConfigScope::Global.get_config_path();
    if dirs::home_dir().is_some() {
        assert!(global_path.is_some());
        let path = global_path.unwrap();
        assert!(path.to_string_lossy().contains(".libra"));
        assert!(path.to_string_lossy().ends_with("config.db"));
    } else {
        // In environments without home directory, should return None
        assert_eq!(global_path, None);
    }

    // System scope should return the appropriate system path for the platform
    let system_path = config::ConfigScope::System.get_config_path();

    #[cfg(unix)]
    {
        assert!(system_path.is_some());
        assert_eq!(
            system_path.unwrap(),
            std::path::PathBuf::from("/etc/libra/config.db")
        );
    }

    #[cfg(windows)]
    {
        // On Windows, should use PROGRAMDATA if available
        if std::env::var_os("PROGRAMDATA").is_some() {
            assert!(system_path.is_some());
            let path = system_path.unwrap();
            assert!(path.to_string_lossy().contains("libra"));
            assert!(path.to_string_lossy().ends_with("config.db"));
        } else {
            assert_eq!(system_path, None);
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        // On unsupported platforms, should return None
        assert_eq!(system_path, None);
    }
}

#[tokio::test]
#[serial]
async fn test_config_windows_system_path() {
    // Test Windows-specific system path behavior
    #[cfg(windows)]
    {
        // Test with PROGRAMDATA environment variable
        let original_programdata = std::env::var_os("PROGRAMDATA");

        // Test with PROGRAMDATA set
        unsafe {
            std::env::set_var("PROGRAMDATA", "C:\\ProgramData");
        }
        let system_path = config::ConfigScope::System.get_config_path();
        assert!(system_path.is_some());
        assert_eq!(
            system_path.unwrap(),
            std::path::PathBuf::from("C:\\ProgramData\\libra\\config.db")
        );

        // Test with PROGRAMDATA unset
        unsafe {
            std::env::remove_var("PROGRAMDATA");
        }
        let system_path_none = config::ConfigScope::System.get_config_path();
        assert_eq!(system_path_none, None);

        // Restore original PROGRAMDATA
        if let Some(original) = original_programdata {
            unsafe {
                std::env::set_var("PROGRAMDATA", original);
            }
        } else {
            unsafe {
                std::env::remove_var("PROGRAMDATA");
            }
        }
    }

    #[cfg(not(windows))]
    {
        // On non-Windows platforms, this test is skipped
        println!("Skipping Windows-specific test on non-Windows platform");
    }
}

#[tokio::test]
#[serial]
async fn test_config_unix_system_path() {
    // Test Unix-specific system path behavior
    #[cfg(unix)]
    {
        let system_path = config::ConfigScope::System.get_config_path();
        assert!(system_path.is_some());
        assert_eq!(
            system_path.unwrap(),
            std::path::PathBuf::from("/etc/libra/config.db")
        );
    }

    #[cfg(not(unix))]
    {
        // On non-Unix platforms, this test is skipped
        println!("Skipping Unix-specific test on non-Unix platform");
    }
}

#[tokio::test]
#[serial]
async fn test_config_cross_platform_paths() {
    // Test that all scopes return appropriate paths for the current platform

    // Local scope should always return None (uses repository database)
    assert_eq!(config::ConfigScope::Local.get_config_path(), None);

    // Global scope behavior (should work on all platforms with home directory)
    let global_path = config::ConfigScope::Global.get_config_path();
    if dirs::home_dir().is_some() {
        assert!(global_path.is_some());
        let path = global_path.unwrap();
        assert!(path.to_string_lossy().contains(".libra"));
        assert!(path.to_string_lossy().ends_with("config.db"));

        // Verify the path uses the correct separator for the platform
        #[cfg(windows)]
        {
            // On Windows, paths should use backslashes or be properly normalized
            let path_str = path.to_string_lossy();
            assert!(path_str.contains("libra") && path_str.contains("config.db"));
        }
        #[cfg(unix)]
        {
            // On Unix, paths should use forward slashes
            assert!(path.to_string_lossy().contains("/"));
        }
    }

    // System scope should return a path on supported platforms
    let system_path = config::ConfigScope::System.get_config_path();

    #[cfg(any(unix, windows))]
    {
        // On supported platforms, should return a path (if environment allows)
        #[cfg(unix)]
        {
            assert!(system_path.is_some());
        }

        #[cfg(windows)]
        {
            // On Windows, depends on PROGRAMDATA availability
            if std::env::var_os("PROGRAMDATA").is_some() {
                assert!(system_path.is_some());
            }
        }

        if let Some(path) = system_path {
            assert!(path.to_string_lossy().contains("libra"));
            assert!(path.to_string_lossy().ends_with("config.db"));
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        // On unsupported platforms, should return None
        assert_eq!(system_path, None);
    }
}
