use std::io::{self, Write};

use clap::Parser;
use serial_test::serial;
use tempfile::tempdir;

use super::*;
use crate::utils::{
    error::StableErrorCode,
    output::OutputConfig,
    test::{self, ChangeDirGuard},
};

#[test]
fn test_parse_args() {
    let args = ShortlogArgs::parse_from(["shortlog"]);
    assert!(!args.numbered);
    assert!(!args.summary);
    assert!(!args.email);

    let args = ShortlogArgs::parse_from(["shortlog", "-n", "-s", "-e"]);
    assert!(args.numbered);
    assert!(args.summary);
    assert!(args.email);

    let args = ShortlogArgs::parse_from(["shortlog", "--since", "2024-01-01"]);
    assert!(args.since.is_some());
}

#[test]
fn test_parse_new_args() {
    let args = ShortlogArgs::parse_from(["shortlog", "-w"]);
    assert_eq!(args.width, Some(Some("76,6,9".to_string())));

    let args = ShortlogArgs::parse_from(["shortlog", "-w=80", "HEAD"]);
    assert_eq!(args.width, Some(Some("80".to_string())));
    assert_eq!(args.revision, Some("HEAD".to_string()));

    let args = ShortlogArgs::parse_from(["shortlog", "-w", "HEAD"]);
    assert_eq!(args.width, Some(Some("76,6,9".to_string())));
    assert_eq!(args.revision, Some("HEAD".to_string()));

    let args = ShortlogArgs::parse_from(["shortlog", "--format", "%s"]);
    assert_eq!(args.format, Some("%s".to_string()));
}

#[test]
fn test_parse_top_arg() {
    let args = ShortlogArgs::parse_from(["shortlog", "--top", "3"]);
    assert_eq!(args.top, Some(3));
}

#[test]
fn test_parse_min_count_arg() {
    let args = ShortlogArgs::parse_from(["shortlog", "--min-count", "5"]);
    assert_eq!(args.min_count, Some(5));
}

#[test]
fn test_parse_reverse_arg() {
    let args = ShortlogArgs::parse_from(["shortlog", "--reverse"]);
    assert!(args.reverse);
}

#[test]
fn broken_pipe_writer_is_ignored() {
    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut writer = BrokenPipeWriter;
    assert!(
        !render::write_shortlog_line(&mut writer, format_args!("alice")).unwrap(),
        "BrokenPipe should terminate output quietly"
    );
}

#[test]
fn non_broken_pipe_writer_error_is_structured() {
    struct PermissionDeniedWriter;

    impl Write for PermissionDeniedWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut writer = PermissionDeniedWriter;
    let err = render::write_shortlog_line(&mut writer, format_args!("alice")).unwrap_err();
    assert_eq!(err.stable_code(), StableErrorCode::IoWriteFailed);
    assert!(err.message().contains("shortlog output error"));
}

#[tokio::test]
#[serial]
async fn execute_safe_requires_repository() {
    let temp = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp.path());
    let _guard = ChangeDirGuard::new(temp.path());

    let err = execute_safe(
        ShortlogArgs::parse_from(["shortlog"]),
        &OutputConfig::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(err.stable_code(), StableErrorCode::RepoNotFound);
}
