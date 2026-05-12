use super::{init_repo_via_cli, parse_cli_error_stderr, run_libra_command};

#[test]
fn publish_reserved_subcommands_return_unsupported_without_clap_json_panic() {
    let repo = tempfile::tempdir().expect("temp repo");
    init_repo_via_cli(repo.path());

    for args in [
        &["publish", "sync"][..],
        &["--json", "publish", "sync"][..],
        &["publish", "status"][..],
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
