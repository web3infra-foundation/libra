use std::io::{self, Write};

use super::rev_list_output::{write_rev_list_count, write_rev_list_output};
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
