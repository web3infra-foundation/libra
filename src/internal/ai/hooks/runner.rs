//! Hook runner: executes hooks and evaluates results.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::config::{HookConfig, HookDefinition};
use super::event::{HookAction, HookEvent, HookInput, HookOutput};

/// Executes hooks based on configuration.
#[derive(Debug)]
pub struct HookRunner {
    config: HookConfig,
    working_dir: std::path::PathBuf,
}

impl HookRunner {
    /// Create a new hook runner from configuration.
    pub fn new(config: HookConfig, working_dir: std::path::PathBuf) -> Self {
        Self {
            config,
            working_dir,
        }
    }

    /// Load hook configuration and create a runner.
    pub fn load(working_dir: &Path) -> Self {
        let config = super::config::load_hook_config(working_dir);
        Self::new(config, working_dir.to_path_buf())
    }

    /// Returns true if there are any enabled hooks.
    pub fn has_hooks(&self) -> bool {
        self.config.hooks.iter().any(|h| h.enabled)
    }

    /// Run all matching PreToolUse hooks. Returns Block if any hook blocks.
    pub async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
    ) -> HookAction {
        let matching: Vec<&HookDefinition> = self
            .config
            .hooks
            .iter()
            .filter(|h| h.enabled && h.event == HookEvent::PreToolUse && h.matches_tool(tool_name))
            .collect();

        if matching.is_empty() {
            return HookAction::Allow;
        }

        let input = HookInput::pre_tool_use(
            tool_name,
            tool_input,
            &self.working_dir.to_string_lossy(),
        );

        for hook in matching {
            match self.execute_hook(hook, &input).await {
                HookResult::Allow => continue,
                HookResult::Block(reason) => return HookAction::Block(reason),
                HookResult::Error(err) => {
                    tracing::warn!("PreToolUse hook '{}' failed: {}", hook.description, err);
                    // Errors in hooks don't block by default
                    continue;
                }
            }
        }

        HookAction::Allow
    }

    /// Run all matching PostToolUse hooks.
    pub async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
        tool_output: serde_json::Value,
    ) {
        let matching: Vec<&HookDefinition> = self
            .config
            .hooks
            .iter()
            .filter(|h| {
                h.enabled && h.event == HookEvent::PostToolUse && h.matches_tool(tool_name)
            })
            .collect();

        if matching.is_empty() {
            return;
        }

        let input = HookInput::post_tool_use(
            tool_name,
            tool_input,
            tool_output,
            &self.working_dir.to_string_lossy(),
        );

        for hook in matching {
            if let HookResult::Error(err) = self.execute_hook(hook, &input).await {
                tracing::warn!("PostToolUse hook '{}' failed: {}", hook.description, err);
            }
        }
    }

    /// Run all matching session lifecycle hooks.
    pub async fn run_session_event(&self, event: HookEvent) {
        let matching: Vec<&HookDefinition> = self
            .config
            .hooks
            .iter()
            .filter(|h| h.enabled && h.event == event)
            .collect();

        if matching.is_empty() {
            return;
        }

        let input = HookInput::session_event(event, &self.working_dir.to_string_lossy());

        for hook in matching {
            if let HookResult::Error(err) = self.execute_hook(hook, &input).await {
                tracing::warn!("{} hook '{}' failed: {}", event, hook.description, err);
            }
        }
    }

    /// Execute a single hook command.
    async fn execute_hook(&self, hook: &HookDefinition, input: &HookInput) -> HookResult {
        let input_json = match serde_json::to_string(input) {
            Ok(json) => json,
            Err(e) => return HookResult::Error(format!("Failed to serialize hook input: {e}")),
        };

        let timeout = Duration::from_millis(hook.timeout_ms);

        let result = tokio::time::timeout(timeout, async {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(&hook.command)
                .current_dir(&self.working_dir)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(input_json.as_bytes()).await?;
                drop(stdin);
            }

            child.wait_with_output().await
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if !stderr.is_empty() {
                    tracing::debug!("Hook stderr: {}", stderr.trim());
                }

                match exit_code {
                    0 => HookResult::Allow,
                    2 => {
                        // Exit code 2 = block (PreToolUse only)
                        let reason = if let Ok(output) = serde_json::from_str::<HookOutput>(&stdout)
                        {
                            output
                                .message
                                .unwrap_or_else(|| "Blocked by hook".to_string())
                        } else if !stdout.trim().is_empty() {
                            stdout.trim().to_string()
                        } else {
                            "Blocked by hook".to_string()
                        };
                        HookResult::Block(reason)
                    }
                    code => HookResult::Error(format!(
                        "Hook exited with code {code}: {}",
                        stderr.trim()
                    )),
                }
            }
            Ok(Err(e)) => HookResult::Error(format!("Failed to run hook: {e}")),
            Err(_) => HookResult::Error(format!(
                "Hook timed out after {}ms",
                hook.timeout_ms
            )),
        }
    }
}

/// Internal result of executing a hook.
enum HookResult {
    Allow,
    Block(String),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::config::HookConfig;

    fn make_runner(hooks: Vec<HookDefinition>) -> (HookRunner, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let runner = HookRunner::new(
            HookConfig { hooks },
            tmp.path().to_path_buf(),
        );
        (runner, tmp)
    }

    fn make_hook(event: HookEvent, matcher: &str, command: &str) -> HookDefinition {
        HookDefinition {
            event,
            matcher: matcher.to_string(),
            command: command.to_string(),
            description: "test hook".to_string(),
            timeout_ms: 5000,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn test_pre_tool_use_allow() {
        let (runner, _tmp) = make_runner(vec![make_hook(
            HookEvent::PreToolUse,
            "read_file",
            "echo '{}'",
        )]);

        let action = runner
            .run_pre_tool_use("read_file", serde_json::json!({}))
            .await;
        assert_eq!(action, HookAction::Allow);
    }

    #[tokio::test]
    async fn test_pre_tool_use_block() {
        let (runner, _tmp) = make_runner(vec![make_hook(
            HookEvent::PreToolUse,
            "shell",
            r#"echo '{"message":"dangerous command blocked"}' && exit 2"#,
        )]);

        let action = runner
            .run_pre_tool_use("shell", serde_json::json!({"command": "rm -rf /"}))
            .await;
        assert!(action.is_blocked());
        if let HookAction::Block(reason) = action {
            assert!(reason.contains("dangerous command blocked"));
        }
    }

    #[tokio::test]
    async fn test_pre_tool_use_no_matching_hooks() {
        let (runner, _tmp) = make_runner(vec![make_hook(
            HookEvent::PreToolUse,
            "shell",
            "exit 2",
        )]);

        let action = runner
            .run_pre_tool_use("read_file", serde_json::json!({}))
            .await;
        assert_eq!(action, HookAction::Allow);
    }

    #[tokio::test]
    async fn test_pre_tool_use_timeout() {
        let (runner, _tmp) = make_runner(vec![HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "*".to_string(),
            command: "sleep 10".to_string(),
            description: "slow hook".to_string(),
            timeout_ms: 100, // 100ms timeout
            enabled: true,
        }]);

        let action = runner
            .run_pre_tool_use("read_file", serde_json::json!({}))
            .await;
        // Timeout results in error, which doesn't block
        assert_eq!(action, HookAction::Allow);
    }

    #[tokio::test]
    async fn test_disabled_hooks_skipped() {
        let (runner, _tmp) = make_runner(vec![HookDefinition {
            event: HookEvent::PreToolUse,
            matcher: "*".to_string(),
            command: "exit 2".to_string(),
            description: "disabled hook".to_string(),
            timeout_ms: 5000,
            enabled: false,
        }]);

        let action = runner
            .run_pre_tool_use("read_file", serde_json::json!({}))
            .await;
        assert_eq!(action, HookAction::Allow);
    }

    #[tokio::test]
    async fn test_has_hooks() {
        let (runner, _tmp) = make_runner(vec![]);
        assert!(!runner.has_hooks());

        let (runner, _tmp) = make_runner(vec![make_hook(
            HookEvent::PreToolUse,
            "*",
            "echo ok",
        )]);
        assert!(runner.has_hooks());
    }
}
