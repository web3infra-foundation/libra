use std::{fs, process::Command};

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

fn claude_settings_file(repo_root: &std::path::Path) -> std::path::PathBuf {
    repo_root.join(".claude").join("settings.json")
}

fn gemini_settings_file(repo_root: &std::path::Path) -> std::path::PathBuf {
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
    assert_eq!(settings["enabledPlugins"]["example"], json!(true));

    let session_start_entries = settings["hooks"]["SessionStart"].as_array().unwrap();
    let startup_count = session_start_entries
        .iter()
        .filter(|item| item["matcher"] == json!("startup"))
        .count();
    let managed_count = session_start_entries
        .iter()
        .filter(|item| {
            item.get("matcher").is_none()
                && item["hooks"][0]["command"] == json!("libra hooks claude session-start")
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
                                "command": "libra hooks claude session-start",
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
            "--command-prefix",
            "go run ./cmd/libra",
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
    assert_eq!(String::from_utf8_lossy(&installed.stdout).trim(), "true");

    let settings_json = fs::read_to_string(&settings_path).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings_json).unwrap();
    let session_start_entries = settings["hooks"]["SessionStart"].as_array().unwrap();
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
            && item["hooks"][0]["command"] == json!("go run ./cmd/libra hooks claude session-start")
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
    assert_eq!(settings["hooksConfig"]["enabled"], json!(true));
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        json!("libra hooks gemini session-start")
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

#[test]
fn test_hooks_reject_unknown_provider() {
    let temp = tempdir().unwrap();

    let output = run_hooks(&temp, "unknown", &["session-start"]);
    assert!(!output.status.success(), "unknown provider should fail");
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported hook provider"));
}
