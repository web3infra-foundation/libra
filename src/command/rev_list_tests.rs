use std::io::{self, Write};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::object::{
        commit::Commit,
        signature::{Signature, SignatureType},
    },
};

use super::{
    RevListArgs, RevListEntry, format_rev_list_entry, sort_rev_list_commits, write_rev_list_count,
    write_rev_list_output,
};
use crate::utils::error::StableErrorCode;

struct FailingWriter {
    kind: io::ErrorKind,
}

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(self.kind, "test write failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn test_signature(timestamp: usize) -> Signature {
    Signature {
        signature_type: SignatureType::Committer,
        name: "tester".to_string(),
        email: "tester@example.com".to_string(),
        timestamp,
        timezone: "+0000".to_string(),
    }
}

fn test_hash(byte: u8) -> ObjectHash {
    ObjectHash::from_bytes(&vec![byte; get_hash_kind().size()])
        .expect("test hash bytes should match active hash kind")
}

fn test_commit(id: ObjectHash, timestamp: usize) -> Commit {
    Commit {
        id,
        tree_id: id,
        parent_commit_ids: Vec::new(),
        author: test_signature(timestamp),
        committer: test_signature(timestamp),
        message: "test".to_string(),
    }
}

#[test]
fn test_rev_list_args_default() {
    let args = RevListArgs::try_parse_from(["rev-list"]).unwrap();
    assert!(args.spec.is_none());
    assert!(!args.parents);
    assert!(!args.timestamp);
}

#[test]
fn test_rev_list_args_with_spec() {
    let args = RevListArgs::try_parse_from(["rev-list", "HEAD~1"]).unwrap();
    assert_eq!(args.spec.as_deref(), Some("HEAD~1"));
}

#[test]
fn test_rev_list_args_parse_count_controls() {
    let args =
        RevListArgs::try_parse_from(["rev-list", "-n", "2", "--skip", "1", "--count", "HEAD"])
            .unwrap();
    assert_eq!(args.max_count, Some(2));
    assert_eq!(args.skip, 1);
    assert!(args.count);
    assert_eq!(args.spec.as_deref(), Some("HEAD"));
}

#[test]
fn test_rev_list_args_parse_parent_and_timestamp_output() {
    let args =
        RevListArgs::try_parse_from(["rev-list", "--parents", "--timestamp", "HEAD"]).unwrap();
    assert!(args.parents);
    assert!(args.timestamp);
    assert_eq!(args.spec.as_deref(), Some("HEAD"));
}

#[test]
fn test_format_rev_list_entry_matches_git_field_order() {
    let entry = RevListEntry {
        commit: "abc123".to_string(),
        parents: vec!["def456".to_string(), "789abc".to_string()],
        timestamp: Some(123),
    };

    assert_eq!(
        format_rev_list_entry(&entry, true, true),
        "123 abc123 def456 789abc"
    );
    assert_eq!(
        format_rev_list_entry(&entry, true, false),
        "abc123 def456 789abc"
    );
    assert_eq!(format_rev_list_entry(&entry, false, true), "123 abc123");
}

#[test]
fn test_sort_rev_list_commits_preserves_equal_timestamp_order() {
    let high = test_hash(0xff);
    let low = test_hash(0x01);
    let mut commits = vec![test_commit(high, 1), test_commit(low, 1)];

    sort_rev_list_commits(&mut commits);

    assert_eq!(commits[0].id, high);
    assert_eq!(commits[1].id, low);
}

#[test]
fn test_sort_rev_list_commits_orders_newest_first() {
    let old = test_hash(0x01);
    let new = test_hash(0xff);
    let mut commits = vec![test_commit(old, 1), test_commit(new, 2)];

    sort_rev_list_commits(&mut commits);

    assert_eq!(commits[0].id, new);
    assert_eq!(commits[1].id, old);
}

#[test]
fn test_write_rev_list_output_maps_write_failure_to_write_code() {
    let mut writer = FailingWriter {
        kind: io::ErrorKind::PermissionDenied,
    };

    let error =
        write_rev_list_output(&mut writer, &["abc123".to_string()]).expect_err("write should fail");

    assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
}

#[test]
fn test_write_rev_list_output_ignores_broken_pipe() {
    let mut writer = FailingWriter {
        kind: io::ErrorKind::BrokenPipe,
    };

    write_rev_list_output(&mut writer, &["abc123".to_string()])
        .expect("broken pipe should be ignored");
}

#[test]
fn test_write_rev_list_count_maps_write_failure_to_write_code() {
    let mut writer = FailingWriter {
        kind: io::ErrorKind::PermissionDenied,
    };

    let error = write_rev_list_count(&mut writer, 1).expect_err("write should fail");

    assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
}
