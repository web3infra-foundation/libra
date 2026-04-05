//! Stash command regression tests.

use std::fs;

use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

#[test]
fn stash_round_trip_preserves_nested_dotfile_paths() {
    let repo = create_committed_repo_via_cli();

    let config_dir = repo.path().join(".config");
    let nested_file = config_dir.join("tool.toml");
    fs::create_dir_all(&config_dir).expect("failed to create nested config dir");
    fs::write(&nested_file, "name = \"base\"\n").expect("failed to write base nested file");

    let output = run_libra_command(&["add", ".config/tool.toml"], repo.path());
    assert_cli_success(&output, "add nested dotfile");

    let output = run_libra_command(
        &["commit", "-m", "track nested dotfile", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&output, "commit nested dotfile");

    fs::write(&nested_file, "name = \"modified\"\n").expect("failed to write modified nested file");

    let output = run_libra_command(&["stash", "push"], repo.path());
    assert_cli_success(&output, "stash push nested dotfile");
    assert_eq!(
        fs::read_to_string(&nested_file).expect("failed to read nested file after stash push"),
        "name = \"base\"\n"
    );

    let output = run_libra_command(&["stash", "pop"], repo.path());
    assert_cli_success(&output, "stash pop nested dotfile");

    assert_eq!(
        fs::read_to_string(&nested_file).expect("failed to read nested file after stash pop"),
        "name = \"modified\"\n"
    );
    assert!(
        !repo.path().join("tool.toml").exists(),
        "stash pop should not flatten nested dotfiles into the repo root"
    );
}
