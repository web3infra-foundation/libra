use std::{fs, path::Path, process::Command};

use libra::utils::test;
use serde_json::json;
use serial_test::serial;
use tempfile::tempdir;

fn run_hooks(temp: &tempfile::TempDir, provider: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp.path())
        .arg("hooks")
        .arg(provider)
        .args(args)
        .output()
        .expect("failed to run hooks command")
}

fn installed_libra_binary() -> String {
    let path = std::fs::canonicalize(env!("CARGO_BIN_EXE_libra"))
        .expect("failed to canonicalize test libra binary");
    quote_command_path(&path)
}

fn installed_libra_binary_for(path: &Path) -> String {
    let path = std::fs::canonicalize(path).expect("failed to canonicalize hook binary");
    quote_command_path(&path)
}

fn quote_command_path(path: &Path) -> String {
    let rendered = path.to_string_lossy();

    #[cfg(windows)]
    {
        if rendered.contains([' ', '\t', '"']) {
            return format!("\"{}\"", rendered.replace('"', "\\\""));
        }
        rendered.into_owned()
    }

    #[cfg(not(windows))]
    {
        if rendered
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
        {
            return rendered.into_owned();
        }
        format!("'{}'", rendered.replace('\'', r#"'\''"#))
    }
}

fn claude_settings_file(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(".claude").join("settings.json")
}

fn gemini_settings_file(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(".gemini").join("settings.json")
}

#[tokio::test]
#[serial]
async fn test_hooks_claude_install_preserves_existing_and_is_idempotent() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let settings_path = claude_settings_file(temp.path());
    fs::create_dir_all(settings_path.parent().expect("parent should exist")).unwrap();
    fs::write(
        &settings_path,
        json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "startup",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "echo keep"
                            }
                        ]
                    }
                ]
            },
            "enabledPlugins": {
                "example": true
            }
        })
        .to_string(),
    )
    .unwrap();

    let first = run_hooks(&temp, "claude", &["install"]);
    assert!(
        first.status.success(),
        "hooks claude install failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = run_hooks(&temp, "claude", &["install"]);
    assert!(
        second.status.success(),
        "second hooks claude install failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let settings_json = fs::read_to_string(&settings_path).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings_json).unwrap();
    let expected_command = format!("{} hooks claude session-start", installed_libra_binary());
    assert_eq!(settings["enabledPlugins"]["example"], json!(true));

    let session_start_entries = settings["hooks"]["SessionStart"].as_array().unwrap();
    let startup_count = session_start_entries
        .iter()
        .filter(|item| item["matcher"] == json!("startup"))
        .count();
    let managed_count = session_start_entries
        .iter()
        .filter(|item| {
            item.get("matcher").is_none() && item["hooks"][0]["command"] == json!(expected_command)
        })
        .count();
    assert_eq!(startup_count, 1);
    assert_eq!(managed_count, 1);
}

#[tokio::test]
#[serial]
async fn test_hooks_claude_install_rewrites_legacy_entries_and_uninstall_roundtrip() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let old_binary = temp.path().join("old").join("libra");
    let new_binary = temp.path().join("new").join("libra");
    fs::create_dir_all(old_binary.parent().unwrap()).unwrap();
    fs::create_dir_all(new_binary.parent().unwrap()).unwrap();
    fs::write(&old_binary, "").unwrap();
    fs::write(&new_binary, "").unwrap();

    let settings_path = claude_settings_file(temp.path());
    fs::create_dir_all(settings_path.parent().expect("parent should exist")).unwrap();
    fs::write(
        &settings_path,
        json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "libra",
                        "hooks": [
                            {
                                "type": "command",
                                "command": format!(
                                    "{} hooks claude session-start",
                                    installed_libra_binary_for(&old_binary)
                                ),
                                "timeout": 10
                            },
                            {
                                "type": "command",
                                "command": "echo keep"
                            }
                        ]
                    }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let install = run_hooks(
        &temp,
        "claude",
        &[
            "install",
            "--binary-path",
            new_binary.to_str().unwrap(),
            "--timeout",
            "15",
        ],
    );
    assert!(
        install.status.success(),
        "hooks claude install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let installed = run_hooks(&temp, "claude", &["is-installed"]);
    assert!(installed.status.success());
    assert_eq!(String::from_utf8_lossy(&installed.stdout).trim(), "false");

    let settings_json = fs::read_to_string(&settings_path).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings_json).unwrap();
    let session_start_entries = settings["hooks"]["SessionStart"].as_array().unwrap();
    let expected_new_command = format!(
        "{} hooks claude session-start",
        installed_libra_binary_for(&new_binary)
    );

    assert_eq!(
        session_start_entries
            .iter()
            .filter(|item| item["matcher"] == json!("libra"))
            .count(),
        1
    );
    assert!(session_start_entries.iter().any(|item| {
        item["matcher"] == json!("libra")
            && item["hooks"]
                .as_array()
                .is_some_and(|hooks| hooks.len() == 1 && hooks[0]["command"] == json!("echo keep"))
    }));
    assert!(session_start_entries.iter().any(|item| {
        item.get("matcher").is_none()
            && item["hooks"][0]["command"] == json!(expected_new_command)
            && item["hooks"][0]["timeout"] == json!(15)
    }));

    let uninstall = run_hooks(&temp, "claude", &["uninstall"]);
    assert!(
        uninstall.status.success(),
        "hooks claude uninstall failed: {}",
        String::from_utf8_lossy(&uninstall.stderr)
    );

    let installed_after = run_hooks(&temp, "claude", &["is-installed"]);
    assert!(installed_after.status.success());
    assert_eq!(
        String::from_utf8_lossy(&installed_after.stdout).trim(),
        "false"
    );
}

#[tokio::test]
#[serial]
async fn test_hooks_gemini_install_is_installed_and_uninstall() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let install = run_hooks(&temp, "gemini", &["install"]);
    assert!(
        install.status.success(),
        "hooks gemini install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let settings_path = gemini_settings_file(temp.path());
    let content = fs::read_to_string(&settings_path).expect("settings file should be created");
    let settings: serde_json::Value = serde_json::from_str(&content).expect("settings json");
    let expected_command = format!("{} hooks gemini session-start", installed_libra_binary());
    assert_eq!(settings["hooksConfig"]["enabled"], json!(true));
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        json!(expected_command)
    );

    let installed = run_hooks(&temp, "gemini", &["is-installed"]);
    assert!(installed.status.success());
    assert_eq!(String::from_utf8_lossy(&installed.stdout).trim(), "true");

    let uninstall = run_hooks(&temp, "gemini", &["uninstall"]);
    assert!(
        uninstall.status.success(),
        "hooks gemini uninstall failed: {}",
        String::from_utf8_lossy(&uninstall.stderr)
    );

    let installed_after = run_hooks(&temp, "gemini", &["is-installed"]);
    assert!(installed_after.status.success());
    assert_eq!(
        String::from_utf8_lossy(&installed_after.stdout).trim(),
        "false"
    );
}

#[tokio::test]
#[serial]
async fn test_hooks_gemini_install_replaces_previous_managed_binary_path() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let old_binary = temp.path().join("old-libra");
    let new_binary = temp.path().join("new-libra");
    fs::write(&old_binary, "").unwrap();
    fs::write(&new_binary, "").unwrap();

    let settings_path = gemini_settings_file(temp.path());
    fs::create_dir_all(settings_path.parent().expect("parent should exist")).unwrap();
    fs::write(
        &settings_path,
        json!({
            "hooksConfig": { "enabled": true },
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "name": "libra-session-start",
                                "type": "command",
                                "command": format!(
                                    "{} hooks gemini session-start",
                                    installed_libra_binary_for(&old_binary)
                                )
                            }
                        ]
                    },
                    {
                        "matcher": "startup",
                        "hooks": [
                            {
                                "name": "user-hook",
                                "type": "command",
                                "command": "echo keep"
                            }
                        ]
                    }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let install = run_hooks(
        &temp,
        "gemini",
        &["install", "--binary-path", new_binary.to_str().unwrap()],
    );
    assert!(
        install.status.success(),
        "hooks gemini install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let content = fs::read_to_string(&settings_path).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&content).unwrap();
    let expected_command = format!(
        "{} hooks gemini session-start",
        installed_libra_binary_for(&new_binary)
    );
    let session_start_entries = settings["hooks"]["SessionStart"].as_array().unwrap();

    assert_eq!(
        session_start_entries
            .iter()
            .filter(|item| {
                item.get("matcher").is_none()
                    && item["hooks"][0]["name"] == json!("libra-session-start")
            })
            .count(),
        1
    );
    assert!(session_start_entries.iter().any(|item| {
        item.get("matcher").is_none() && item["hooks"][0]["command"] == json!(expected_command)
    }));
    assert!(
        session_start_entries
            .iter()
            .any(|item| item["matcher"] == json!("startup"))
    );
}

#[tokio::test]
#[serial]
async fn test_hooks_gemini_is_installed_rejects_disabled_or_stale_command() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let settings_path = gemini_settings_file(temp.path());
    fs::create_dir_all(settings_path.parent().expect("parent should exist")).unwrap();
    fs::write(
        &settings_path,
        json!({
            "hooksConfig": { "enabled": false },
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "name": "libra-session-start",
                                "type": "command",
                                "command": "stale-libra hooks gemini session-start"
                            }
                        ]
                    }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let installed = run_hooks(&temp, "gemini", &["is-installed"]);
    assert!(installed.status.success());
    assert_eq!(String::from_utf8_lossy(&installed.stdout).trim(), "false");
}

#[tokio::test]
#[serial]
async fn test_hooks_gemini_install_rejects_timeout() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;

    let install = run_hooks(&temp, "gemini", &["install", "--timeout", "10"]);
    assert!(
        !install.status.success(),
        "gemini install with timeout should fail"
    );
    assert!(
        String::from_utf8_lossy(&install.stderr).contains("Gemini hooks do not support --timeout")
    );
}

#[tokio::test]
#[serial]
async fn test_hooks_install_commands_require_libra_repo() {
    let temp = tempdir().unwrap();

    let install = run_hooks(&temp, "claude", &["install"]);
    assert!(!install.status.success());
    assert!(String::from_utf8_lossy(&install.stderr).contains("inside a Libra repository"));

    let is_installed = run_hooks(&temp, "gemini", &["is-installed"]);
    assert!(!is_installed.status.success());
    assert!(String::from_utf8_lossy(&is_installed.stderr).contains("inside a Libra repository"));
}

#[test]
fn test_hooks_reject_unknown_provider() {
    let temp = tempdir().unwrap();

    let output = run_hooks(&temp, "unknown", &["session-start"]);
    assert!(!output.status.success(), "unknown provider should fail");
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported hook provider"));
}
