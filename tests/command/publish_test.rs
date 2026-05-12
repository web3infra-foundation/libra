use super::{
    assert_cli_success, create_committed_repo_via_cli, init_repo_via_cli, parse_cli_error_stderr,
    parse_json_stdout, run_libra_command,
};

#[test]
fn publish_reserved_subcommands_return_unsupported_without_clap_json_panic() {
    let repo = tempfile::tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    for args in [
        &["publish", "sync"][..],
        &["--json", "publish", "sync"][..],
        &["publish", "deploy"][..],
        &["publish", "unpublish"][..],
    ] {
        let output = run_libra_command(args, repo.path());
        assert!(
            !output.status.success(),
            "{args:?} should return the publish unsupported error"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("panicked"),
            "{args:?} must not panic on publish reserved subcommands: {stderr}"
        );

        let (_, report) = parse_cli_error_stderr(&output.stderr);
        assert_eq!(report.error_code, "LBR-UNSUPPORTED-001");
        assert!(
            report.message.contains("not ready yet"),
            "{args:?} should explain that publish plumbing is not ready: {stderr}"
        );
    }
}

#[test]
fn publish_status_reports_local_template_state() {
    let repo = tempfile::tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--json", "publish", "status"], repo.path());

    assert!(
        output.status.success(),
        "publish status should inspect the local template state: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"missing\""),
        "status before publish init should be missing: {stdout}"
    );
}

#[test]
fn publish_sync_dry_run_reports_local_ref_plan_without_cloud_writes() {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "v1.0.0"], repo.path());
    assert_cli_success(&tag, "create publish dry-run tag");

    let output = run_libra_command(&["--json", "publish", "sync", "--dry-run"], repo.path());
    assert_cli_success(&output, "publish sync dry-run should produce a local plan");

    let json = parse_json_stdout(&output);
    let data = &json["data"];
    assert_eq!(data["dryRun"], true);
    assert_eq!(data["refsCount"], 2);
    assert_eq!(data["revisionCount"], 1);
    assert_eq!(data["defaultRef"], "refs/heads/main");
    assert!(data["latestRevisionOid"].as_str().is_some());
    assert!(data["fileCount"].as_u64().unwrap_or_default() > 0);
    assert_eq!(data["aiObjectCount"], 0);
    assert_eq!(data["aiBundleCount"], 0);
    assert_eq!(data["updatesFullRefsGeneration"], true);
    assert!(
        !repo.path().join(".libra/publish").exists(),
        "dry-run must not create publish manifests or cloud sync state"
    );
}

#[test]
fn publish_sync_dry_run_ref_filters_to_full_ref() {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "v1.0.0"], repo.path());
    assert_cli_success(&tag, "create publish dry-run tag");

    let output = run_libra_command(
        &[
            "--json",
            "publish",
            "sync",
            "--dry-run",
            "--ref",
            "refs/tags/v1.0.0",
        ],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "publish sync dry-run --ref should produce a local plan",
    );

    let json = parse_json_stdout(&output);
    let data = &json["data"];
    assert_eq!(data["refsCount"], 1);
    assert_eq!(data["selectedRef"], "refs/tags/v1.0.0");
    assert_eq!(data["updatesFullRefsGeneration"], false);
    assert_eq!(data["refs"][0]["refName"], "refs/tags/v1.0.0");
}

#[test]
fn publish_sync_dry_run_ambiguous_short_ref_requires_full_ref() {
    let repo = create_committed_repo_via_cli();
    let branch = run_libra_command(&["branch", "release"], repo.path());
    assert_cli_success(&branch, "create release branch");
    let tag = run_libra_command(&["tag", "release"], repo.path());
    assert_cli_success(&tag, "create release tag");

    let output = run_libra_command(
        &["--json", "publish", "sync", "--dry-run", "--ref", "release"],
        repo.path(),
    );

    assert!(
        !output.status.success(),
        "ambiguous short ref should fail instead of choosing branch or tag"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report.message.contains("ambiguous publish ref"),
        "error should identify the short ref collision: {:?}",
        report.message
    );
}

#[test]
fn publish_sync_dry_run_fail_on_dirty_rejects_dirty_tree() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "dirty\n").expect("dirty tracked file");

    let output = run_libra_command(
        &["--json", "publish", "sync", "--dry-run", "--fail-on-dirty"],
        repo.path(),
    );

    assert!(
        !output.status.success(),
        "publish sync --dry-run --fail-on-dirty should reject a dirty tree"
    );
    let (_, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        report.message.contains("dirty working tree"),
        "error should explain the dirty tree: {:?}",
        report.message
    );
}
