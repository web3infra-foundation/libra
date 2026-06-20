use super::*;

fn ls_remote_refnames(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            line.split_once('\t')
                .map(|(_, refname)| refname.to_string())
        })
        .collect()
}

#[test]
fn test_ls_remote_get_url_resolves_configured_remote_without_discovery() {
    let local = create_committed_repo_via_cli();
    let missing_remote = local.path().join("missing-remote.git");
    let missing_remote_arg = missing_remote.to_string_lossy().to_string();

    let add_output = run_libra_command(
        &["remote", "add", "origin", &missing_remote_arg],
        local.path(),
    );
    assert_cli_success(&add_output, "failed to add missing remote URL");

    let output = run_libra_command(&["ls-remote", "--get-url", "origin"], local.path());
    assert_cli_success(
        &output,
        "--get-url should resolve configured URLs without contacting the remote",
    );

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        missing_remote_arg
    );
}

#[test]
fn test_ls_remote_exit_code_returns_two_when_no_refs_match() {
    let remote = create_committed_repo_via_cli();
    let outside = tempdir().expect("failed to create outside cwd");
    let remote_path = remote.path().to_string_lossy().to_string();

    let output = run_libra_command(
        &["ls-remote", "--exit-code", &remote_path, "no-match"],
        outside.path(),
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "no-match --exit-code should not render refs: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "no-match --exit-code should be a silent exit signal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_ls_remote_sort_refname_and_reverse_refname() {
    let remote = create_committed_repo_via_cli();
    for branch in ["zeta", "alpha"] {
        let output = run_libra_command(&["branch", branch], remote.path());
        assert_cli_success(&output, "failed to create branch for ls-remote sort");
    }
    let outside = tempdir().expect("failed to create outside cwd");
    let remote_path = remote.path().to_string_lossy().to_string();

    let ascending = run_libra_command(
        &["ls-remote", "--heads", "--sort=refname", &remote_path],
        outside.path(),
    );
    assert_cli_success(&ascending, "ls-remote --sort=refname should succeed");
    assert_eq!(
        ls_remote_refnames(&ascending),
        vec![
            "refs/heads/alpha".to_string(),
            "refs/heads/main".to_string(),
            "refs/heads/zeta".to_string(),
        ]
    );

    let descending = run_libra_command(
        &["ls-remote", "--heads", "--sort=-refname", &remote_path],
        outside.path(),
    );
    assert_cli_success(&descending, "ls-remote --sort=-refname should succeed");
    assert_eq!(
        ls_remote_refnames(&descending),
        vec![
            "refs/heads/zeta".to_string(),
            "refs/heads/main".to_string(),
            "refs/heads/alpha".to_string(),
        ]
    );
}

#[test]
fn test_ls_remote_unknown_sort_key_is_rejected() {
    let remote = create_committed_repo_via_cli();
    let outside = tempdir().expect("failed to create outside cwd");
    let remote_path = remote.path().to_string_lossy().to_string();

    let output = run_libra_command(
        &["ls-remote", "--sort=unknown", &remote_path],
        outside.path(),
    );
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        human.contains("unsupported ls-remote sort key 'unknown'"),
        "unknown sort error should name the unsupported key: {human}"
    );
}
