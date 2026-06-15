use super::*;

fn committed_head(repo: &std::path::Path) -> String {
    let output = run_libra_command(&["rev-parse", "HEAD"], repo);
    assert_cli_success(&output, "rev-parse HEAD");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn first_hash(stdout: &str) -> &str {
    stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("")
}

#[test]
fn test_show_ref_abbrev_without_value_uses_default_width() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["show-ref", "--abbrev", "--heads"], repo.path());
    assert_cli_success(&output, "show-ref --abbrev --heads");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = first_hash(&stdout);

    assert_eq!(hash.len(), 7, "unexpected output: {stdout}");
    assert!(stdout.contains(" refs/heads/main"));
}

#[test]
fn test_show_ref_abbrev_explicit_width() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["show-ref", "--abbrev=12", "--heads"], repo.path());
    assert_cli_success(&output, "show-ref --abbrev=12 --heads");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = first_hash(&stdout);

    assert_eq!(hash.len(), 12, "unexpected output: {stdout}");
    assert!(stdout.contains(" refs/heads/main"));
}

#[test]
fn test_show_ref_abbrev_zero_keeps_full_hash() {
    let repo = create_committed_repo_via_cli();
    let head = committed_head(repo.path());

    let output = run_libra_command(&["show-ref", "--abbrev=0", "--heads"], repo.path());
    assert_cli_success(&output, "show-ref --abbrev=0 --heads");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = first_hash(&stdout);

    assert_eq!(hash, head, "unexpected output: {stdout}");
}

#[test]
fn test_show_ref_hash_explicit_width_outputs_only_abbreviated_hash() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["show-ref", "--hash=12", "--heads"], repo.path());
    assert_cli_success(&output, "show-ref --hash=12 --heads");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 1, "unexpected output: {stdout}");
    assert_eq!(lines[0].len(), 12, "unexpected output: {stdout}");
    assert!(!stdout.contains("refs/heads/main"));
}

#[test]
fn test_show_ref_abbrev_json_shortens_hashes() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["--json", "show-ref", "--abbrev=12", "--heads"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --json --abbrev=12 --heads");
    let json = parse_json_stdout(&output);
    let entries = json["data"]["entries"]
        .as_array()
        .expect("entries should be an array");
    let branch = entries
        .iter()
        .find(|entry| entry["refname"] == "refs/heads/main")
        .expect("branch entry should be present");

    assert_eq!(json["data"]["abbrev"], 12);
    assert_eq!(branch["hash"].as_str().unwrap_or("").len(), 12);
}
