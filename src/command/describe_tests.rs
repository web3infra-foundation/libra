use super::*;

/// Pin the `Display` format for every variant of [`DescribeError`].
/// These strings are used directly as the CliError message via
/// `describe_cli_error` and surface in human and `--json` envelopes.
#[test]
fn describe_error_display_pins_each_variant() {
    assert_eq!(
        DescribeError::HeadUnborn.to_string(),
        "HEAD does not point to a commit",
    );
    assert_eq!(
        DescribeError::InvalidReference("bad-ref".to_string()).to_string(),
        "bad-ref",
    );
    assert_eq!(
        DescribeError::ReadFailure("db locked".to_string()).to_string(),
        "db locked",
    );
    assert_eq!(
        DescribeError::CorruptReference("bad commit hash".to_string()).to_string(),
        "bad commit hash",
    );
    assert_eq!(
        DescribeError::LoadCommit {
            commit_id: "deadbeef".to_string(),
            detail: "object not found".to_string(),
        }
        .to_string(),
        "failed to load commit 'deadbeef': object not found",
    );
    assert_eq!(
        DescribeError::NoNamesFound.to_string(),
        "no names found, cannot describe anything",
    );
    assert_eq!(
        DescribeError::NoExactMatch {
            commit_id: "deadbeef".to_string(),
        }
        .to_string(),
        "no tag exactly matches 'deadbeef'",
    );
}
