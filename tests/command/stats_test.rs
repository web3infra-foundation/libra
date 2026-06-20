use std::fs;

use super::{assert_cli_success, parse_json_stdout, run_libra_command};

#[test]
fn test_stats_counts_extensions_in_workdir() {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let root = temp.path();

    fs::write(root.join("main.rs"), "").expect("failed to write main.rs");
    fs::write(root.join("lib.rs"), "").expect("failed to write lib.rs");
    fs::write(root.join("README.md"), "").expect("failed to write README.md");
    fs::write(root.join("LICENSE"), "").expect("failed to write LICENSE");

    fs::create_dir(root.join("target")).expect("failed to create target dir");
    fs::write(root.join("target").join("ignored.rs"), "").expect("failed to write ignored.rs");

    fs::create_dir(root.join(".libra")).expect("failed to create .libra dir");
    fs::write(root.join(".libra").join("ignored.toml"), "").expect("failed to write ignored.toml");

    let output = run_libra_command(&["stats"], root);
    assert_cli_success(&output, "stats command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("File statistics"));
    assert!(stdout.contains("total: 4"));
    assert!(stdout.contains("rs: 2"));
    assert!(stdout.contains("md: 1"));
    assert!(stdout.contains("no_extension: 1"));
    assert!(!stdout.contains("toml: 1"));
}

#[test]
fn test_stats_outputs_json() {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let root = temp.path();

    fs::write(root.join("main.rs"), "").expect("failed to write main.rs");
    fs::write(root.join("README.md"), "").expect("failed to write README.md");
    fs::write(root.join("LICENSE"), "").expect("failed to write LICENSE");

    let output = run_libra_command(&["--json", "stats"], root);
    assert_cli_success(&output, "stats --json should succeed");

    let json = parse_json_stdout(&output);

    assert_eq!(json["total"], 3);
    assert_eq!(json["extensions"]["rs"], 1);
    assert_eq!(json["extensions"]["md"], 1);
    assert_eq!(json["extensions"]["no_extension"], 1);
}
