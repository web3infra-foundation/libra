use std::fs;
use tempfile::TempDir;
use assert_cmd::Command;

#[test]
fn test_stats_counts_extensions_in_workdir() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    fs::write(temp_path.join("file1.txt"), "content").unwrap();
    fs::write(temp_path.join("file2.txt"), "content").unwrap();
    fs::write(temp_path.join("script.rs"), "fn main() {}").unwrap();
    fs::write(temp_path.join("readme"), "no extension").unwrap();

    fs::create_dir(temp_path.join(".libra")).unwrap();
    fs::write(temp_path.join(".libra/config"), "ignored").unwrap();
    fs::create_dir(temp_path.join("target")).unwrap();
    fs::write(temp_path.join("target/output"), "ignored").unwrap();

    std::env::set_current_dir(temp_path).unwrap();

    let mut cmd = Command::cargo_bin("libra").unwrap();
    let output = cmd.arg("stats").output().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("txt: 2"));
    assert!(stdout.contains("rs: 1"));
    assert!(stdout.contains("no_extension: 1"));
    assert!(!stdout.contains(".libra"));
    assert!(!stdout.contains("target"));
}

#[test]
fn test_stats_json_output() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    fs::write(temp_path.join("test.json"), "{}").unwrap();
    fs::write(temp_path.join("data.yaml"), "key: val").unwrap();

    std::env::set_current_dir(temp_path).unwrap();

    let mut cmd = Command::cargo_bin("libra").unwrap();
    let output = cmd.arg("stats").arg("--json").output().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.starts_with("{"));
    assert!(stdout.contains("json"));
    assert!(stdout.contains("yaml"));
}

#[test]
fn test_stats_empty_directory() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    std::env::set_current_dir(temp_path).unwrap();

    let mut cmd = Command::cargo_bin("libra").unwrap();
    let output = cmd.arg("stats").output().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Total files: 0"));
}
