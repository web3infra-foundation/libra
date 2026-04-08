use std::{path::Path, time::Instant};

use serde_json::json;

use super::{
    policy,
    types::{GateReport, GateResult, TaskSpec},
};
use crate::internal::ai::{
    intentspec::types::{Check, CheckKind, IntentSpec},
    sandbox::{ToolRuntimeContext, run_shell_command},
};

const DEFAULT_TIMEOUT_SECS: u64 = 900;
#[cfg(test)]
const TIMEOUT_EXIT_CODE: i32 = 124;
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Execute a single verification check and return its result.
pub async fn run_check(check: &Check, working_dir: &Path) -> GateResult {
    run_check_with_context(check, working_dir, None, None, None).await
}

pub async fn run_check_with_context(
    check: &Check,
    working_dir: &Path,
    spec: Option<&IntentSpec>,
    task: Option<&TaskSpec>,
    runtime_context: Option<&ToolRuntimeContext>,
) -> GateResult {
    match check.kind {
        CheckKind::Policy | CheckKind::Command | CheckKind::TestSuite => {
            run_command_check(check, working_dir, spec, task, runtime_context).await
        }
    }
}

/// Execute multiple checks sequentially and aggregate results.
pub async fn run_gates(checks: &[Check], working_dir: &Path) -> GateReport {
    run_gates_with_context(checks, working_dir, None, None, None).await
}

pub async fn run_gates_with_context(
    checks: &[Check],
    working_dir: &Path,
    spec: Option<&IntentSpec>,
    task: Option<&TaskSpec>,
    runtime_context: Option<&ToolRuntimeContext>,
) -> GateReport {
    let mut results = Vec::with_capacity(checks.len());
    let mut all_required_passed = true;

    for check in checks {
        let result = run_check_with_context(check, working_dir, spec, task, runtime_context).await;
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

async fn run_command_check(
    check: &Check,
    working_dir: &Path,
    spec: Option<&IntentSpec>,
    task: Option<&TaskSpec>,
    runtime_context: Option<&ToolRuntimeContext>,
) -> GateResult {
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

    if let (Some(spec), Some(task)) = (spec, task) {
        let args = json!({ "command": command });
        if let Err(violation) = policy::evaluate_tool_call(spec, task, "shell", &args, working_dir)
        {
            return GateResult {
                check_id: check.id.clone(),
                kind: format!("{:?}", check.kind).to_lowercase(),
                passed: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("policy preflight rejected check: {}", violation.message),
                duration_ms: 0,
                timed_out: false,
            };
        }
    }

    let timeout_secs = check.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let expected_exit = check.expected_exit_code.unwrap_or(0);
    let start = Instant::now();
    let max_output_bytes = runtime_context
        .and_then(|ctx| ctx.max_output_bytes)
        .unwrap_or(MAX_OUTPUT_BYTES);
    let sandbox = runtime_context.and_then(|ctx| ctx.sandbox.clone());
    let sandbox_runtime = runtime_context.and_then(|ctx| ctx.sandbox_runtime.as_ref());

    let result = run_shell_command(
        &command,
        working_dir,
        Some(timeout_secs.saturating_mul(1000)),
        max_output_bytes,
        sandbox,
        sandbox_runtime,
    )
    .await;

    let (exit_code, stdout, stderr, timed_out) = match result {
        Ok(output) => (
            output.exit_code,
            output.stdout,
            output.stderr,
            output.timed_out,
        ),
        Err(err) => (-1, String::new(), err, false),
    };

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
        let check = make_check("t5", CheckKind::Policy, Some("true"));
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
