use std::path::Path;
use std::time::Instant;

use tokio::process::Command;

use super::types::{GateReport, GateResult};
use crate::internal::ai::intentspec::types::{Check, CheckKind};

const DEFAULT_TIMEOUT_SECS: u64 = 900;
const TIMEOUT_EXIT_CODE: i32 = 124;
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Execute a single verification check and return its result.
pub async fn run_check(check: &Check, working_dir: &Path) -> GateResult {
    match check.kind {
        CheckKind::Policy => GateResult {
            check_id: check.id.clone(),
            kind: "policy".into(),
            passed: true,
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            timed_out: false,
        },
        CheckKind::Command | CheckKind::TestSuite => {
            let command = match &check.command {
                Some(cmd) => cmd.clone(),
                None => {
                    return GateResult {
                        check_id: check.id.clone(),
                        kind: format!("{:?}", check.kind).to_lowercase(),
                        passed: false,
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: "no command specified".into(),
                        duration_ms: 0,
                        timed_out: false,
                    };
                }
            };

            let timeout_secs = check.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS);
            let expected_exit = check.expected_exit_code.unwrap_or(0);
            let start = Instant::now();

            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            let mut cmd = Command::new(&shell);
            cmd.arg("-c")
                .arg(&command)
                .current_dir(working_dir)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            let result = match cmd.spawn() {
                Ok(child) => {
                    let timeout_dur =
                        std::time::Duration::from_secs(timeout_secs);
                    tokio::select! {
                        output = child.wait_with_output() => {
                            match output {
                                Ok(out) => {
                                    let stdout = truncate_output(&out.stdout);
                                    let stderr = truncate_output(&out.stderr);
                                    let exit_code = out.status.code().unwrap_or(-1);
                                    (exit_code, stdout, stderr, false)
                                }
                                Err(e) => (-1, String::new(), e.to_string(), false),
                            }
                        }
                        _ = tokio::time::sleep(timeout_dur) => {
                            (TIMEOUT_EXIT_CODE, String::new(), "timed out".into(), true)
                        }
                    }
                }
                Err(e) => (-1, String::new(), e.to_string(), false),
            };

            let (exit_code, stdout, stderr, timed_out) = result;
            let duration_ms = start.elapsed().as_millis() as u64;
            let passed = !timed_out && exit_code == expected_exit;

            GateResult {
                check_id: check.id.clone(),
                kind: format!("{:?}", check.kind).to_lowercase(),
                passed,
                exit_code,
                stdout,
                stderr,
                duration_ms,
                timed_out,
            }
        }
    }
}

/// Execute multiple checks sequentially and aggregate results.
pub async fn run_gates(checks: &[Check], working_dir: &Path) -> GateReport {
    let mut results = Vec::with_capacity(checks.len());
    let mut all_required_passed = true;

    for check in checks {
        let result = run_check(check, working_dir).await;
        if !result.passed && check.required {
            all_required_passed = false;
        }
        results.push(result);
    }

    GateReport {
        results,
        all_required_passed,
    }
}

fn truncate_output(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    if s.len() > MAX_OUTPUT_BYTES {
        let mut truncated = s[..MAX_OUTPUT_BYTES].to_string();
        truncated.push_str("\n[output truncated]");
        truncated
    } else {
        s.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_check(id: &str, kind: CheckKind, command: Option<&str>) -> Check {
        Check {
            id: id.into(),
            kind,
            command: command.map(String::from),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }
    }

    #[tokio::test]
    async fn test_run_check_command_true() {
        let check = make_check("t1", CheckKind::Command, Some("true"));
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_run_check_command_false() {
        let check = make_check("t2", CheckKind::Command, Some("false"));
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(!result.passed);
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_run_check_timeout() {
        let check = Check {
            id: "t3".into(),
            kind: CheckKind::Command,
            command: Some("sleep 60".into()),
            timeout_seconds: Some(1),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(!result.passed);
        assert!(result.timed_out);
        assert_eq!(result.exit_code, TIMEOUT_EXIT_CODE);
    }

    #[tokio::test]
    async fn test_run_check_expected_exit_code() {
        let check = Check {
            id: "t4".into(),
            kind: CheckKind::Command,
            command: Some("exit 42".into()),
            timeout_seconds: Some(10),
            expected_exit_code: Some(42),
            required: true,
            artifacts_produced: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert_eq!(result.exit_code, 42);
    }

    #[tokio::test]
    async fn test_run_check_policy_passthrough() {
        let check = make_check("t5", CheckKind::Policy, None);
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert_eq!(result.kind, "policy");
    }

    #[tokio::test]
    async fn test_run_check_no_command() {
        let check = make_check("t6", CheckKind::Command, None);
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(!result.passed);
        assert_eq!(result.exit_code, -1);
    }

    #[tokio::test]
    async fn test_run_gates_aggregate() {
        let checks = vec![
            make_check("g1", CheckKind::Command, Some("true")),
            make_check("g2", CheckKind::Command, Some("true")),
        ];
        let dir = tempfile::tempdir().unwrap();
        let report = run_gates(&checks, dir.path()).await;
        assert!(report.all_required_passed);
        assert_eq!(report.results.len(), 2);
    }

    #[tokio::test]
    async fn test_run_gates_required_failure() {
        let checks = vec![
            make_check("g3", CheckKind::Command, Some("true")),
            make_check("g4", CheckKind::Command, Some("false")),
        ];
        let dir = tempfile::tempdir().unwrap();
        let report = run_gates(&checks, dir.path()).await;
        assert!(!report.all_required_passed);
    }

    #[tokio::test]
    async fn test_run_gates_optional_failure() {
        let checks = vec![Check {
            id: "g5".into(),
            kind: CheckKind::Command,
            command: Some("false".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: false,
            artifacts_produced: vec![],
        }];
        let dir = tempfile::tempdir().unwrap();
        let report = run_gates(&checks, dir.path()).await;
        assert!(report.all_required_passed);
        assert!(!report.results[0].passed);
    }

    #[tokio::test]
    async fn test_run_check_captures_stdout() {
        let check = make_check("t7", CheckKind::Command, Some("echo hello"));
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert!(result.stdout.contains("hello"));
    }
}
