//! Integration tests for `libra code --stdio` Claude SDK / MCP stdio transport.
//!
//! **Layer:** L1 — deterministic, Unix-only (`#[cfg(unix)]`).

use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::Arc,
};

use git_internal::{
    hash::ObjectHash,
    internal::object::{
        decision::Decision, evidence::Evidence, intent::Intent, patchset::PatchSet, plan::Plan,
        provenance::Provenance, run::Run, run_usage::RunUsage, task::Task,
    },
};
use libra::{
    internal::{
        ai::{
            history::{AI_REF, HistoryManager},
            projection::{ProjectionRebuilder, ThreadProjection},
        },
        model::{
            ai_index_run_event, ai_index_run_patchset, ai_index_task_run, ai_live_context_window,
            ai_scheduler_plan_head, ai_scheduler_state,
            reference::{self, ConfigKind},
        },
    },
    utils::{
        storage::{Storage, local::LocalStorage},
        storage_ext::StorageExt,
        test,
    },
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use serial_test::serial;
use tempfile::tempdir;
use uuid::Uuid;

use super::{assert_cli_success, run_libra_command, run_libra_command_raw};

const PROBE_LIKE_ARTIFACT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_managed_probe_like.json"
));
const SEMANTIC_FULL_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_managed_semantic_full_template.json"
));
const PLAN_TASK_ONLY_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_managed_plan_task_only_template.json"
));
const PLAN_PROMPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/ai/claude_sdk_plan_prompt.txt"
));

const DEFAULT_MANAGED_PROMPT: &str = "Bridge a managed Claude SDK session into Libra artifacts.";

fn parse_stdout_json(output: &Output, context: &str) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("{context}: failed to parse stdout JSON: {err}"))
}

fn parse_stdout_ndjson(output: &Output, context: &str) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap_or_else(|err| panic!("{context}: failed to decode stdout as UTF-8: {err}"))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap_or_else(|err| {
                panic!("{context}: failed to parse NDJSON line: {err}; line={line}")
            })
        })
        .collect()
}

fn find_ndjson_event<'a>(events: &'a [Value], event_name: &str, context: &str) -> &'a Value {
    events
        .iter()
        .find(|event| event["event"] == json!(event_name))
        .unwrap_or_else(|| panic!("{context}: missing NDJSON event '{event_name}'"))
}

fn read_json_file(path: &Path) -> Value {
    let body = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read JSON file '{}': {err}", path.display()));
    serde_json::from_str(&body)
        .unwrap_or_else(|err| panic!("failed to parse JSON file '{}': {err}", path.display()))
}

fn list_json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("failed to read directory '{}': {err}", dir.display()))
        .filter_map(|entry| {
            let entry = entry
                .unwrap_or_else(|err| panic!("failed to read entry in '{}': {err}", dir.display()));
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("json")).then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn only_json_file(dir: &Path, context: &str) -> PathBuf {
    let files = list_json_files(dir);
    assert_eq!(
        files.len(),
        1,
        "{context}: expected exactly one JSON file in '{}', found {:?}",
        dir.display(),
        files
    );
    files[0].clone()
}

async fn read_history_head(repo: &Path, history: &HistoryManager) -> String {
    assert_eq!(history.ref_name(), AI_REF);
    let db_path = repo.join(".libra/libra.db");
    let db_conn = libra::internal::db::establish_connection(
        db_path.to_str().expect("db path should be valid UTF-8"),
    )
    .await
    .expect("failed to connect test database");
    let row = reference::Entity::find()
        .filter(reference::Column::Name.eq(AI_REF))
        .filter(reference::Column::Kind.eq(ConfigKind::Branch))
        .one(&db_conn)
        .await
        .expect("failed to query AI history ref")
        .expect("AI history ref should exist");
    row.commit.expect("AI history ref should point to a commit")
}

fn write_shell_helper(path: &Path, artifact_path: &Path) {
    let artifact_rendered = artifact_path.to_string_lossy().replace('\'', r#"'\''"#);
    let script = format!("#!/bin/sh\ncat '{artifact_rendered}'\n");
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|err| panic!("failed to stat helper script '{}': {err}", path.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap_or_else(|err| {
        panic!(
            "failed to set executable permissions on helper script '{}': {err}",
            path.display()
        )
    });
}

fn write_request_capture_shell_helper(path: &Path, artifact_path: &Path, request_path: &Path) {
    let artifact_rendered = artifact_path.to_string_lossy().replace('\'', r#"'\''"#);
    let request_rendered = request_path.to_string_lossy().replace('\'', r#"'\''"#);
    let script = format!("#!/bin/sh\ncat > '{request_rendered}'\ncat '{artifact_rendered}'\n");
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|err| panic!("failed to stat helper script '{}': {err}", path.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap_or_else(|err| {
        panic!(
            "failed to set executable permissions on helper script '{}': {err}",
            path.display()
        )
    });
}

fn write_json_response_capture_shell_helper(
    path: &Path,
    response_path: &Path,
    request_path: &Path,
) {
    let response_rendered = response_path.to_string_lossy().replace('\'', r#"'\''"#);
    let request_rendered = request_path.to_string_lossy().replace('\'', r#"'\''"#);
    let script = format!("#!/bin/sh\ncat > '{request_rendered}'\ncat '{response_rendered}'\n");
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|err| panic!("failed to stat helper script '{}': {err}", path.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap_or_else(|err| {
        panic!(
            "failed to set executable permissions on helper script '{}': {err}",
            path.display()
        )
    });
}

fn write_json_response_capture_python_helper(
    path: &Path,
    response_path: &Path,
    request_path: &Path,
) {
    let response_literal =
        serde_json::to_string(&response_path.to_string_lossy().to_string()).expect("path json");
    let request_literal =
        serde_json::to_string(&request_path.to_string_lossy().to_string()).expect("path json");
    let script = format!(
        "#!/usr/bin/env python3\nimport json\nimport pathlib\nimport sys\n\nrequest = json.load(sys.stdin)\npathlib.Path({request_literal}).write_text(json.dumps(request), encoding='utf-8')\nsys.stdout.write(pathlib.Path({response_literal}).read_text(encoding='utf-8'))\n"
    );
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper script '{}': {err}", path.display()));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|err| panic!("failed to stat helper script '{}': {err}", path.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap_or_else(|err| {
        panic!(
            "failed to set executable permissions on helper script '{}': {err}",
            path.display()
        )
    });
}

fn embedded_claude_sdk_python_helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("internal")
        .join("ai")
        .join("providers")
        .join("claude_sdk")
        .join("helper.py")
}

fn python3_executable_path() -> String {
    let output = Command::new("python3")
        .arg("-c")
        .arg("import sys; print(sys.executable)")
        .output()
        .expect("failed to resolve python3 executable");
    assert!(
        output.status.success(),
        "python3 executable probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("python3 executable probe should be UTF-8")
        .trim()
        .to_string()
}

fn write_fake_claude_agent_sdk_python_package(root: &Path) {
    let package_dir = root.join("claude_agent_sdk");
    fs::create_dir_all(&package_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create fake python sdk package '{}': {err}",
            package_dir.display()
        )
    });

    let module = r#"
import json
import os
from types import SimpleNamespace

from .types import (
    AssistantMessage,
    ClaudeAgentOptions,
    PermissionResultAllow,
    PermissionResultDeny,
    ResultMessage,
    SystemMessage,
)


def _load_scenario():
    scenario_path = os.environ.get("LIBRA_FAKE_CLAUDE_SDK_SCENARIO_PATH")
    if not scenario_path:
        raise RuntimeError("LIBRA_FAKE_CLAUDE_SDK_SCENARIO_PATH is not set")
    with open(scenario_path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def _default_structured_output():
    return {
        "summary": "Fake Claude managed run",
        "problemStatement": "Exercise the managed Claude SDK helper flow.",
        "changeType": "refactor",
        "objectives": ["Exercise managed Claude SDK helper flow"],
        "successCriteria": ["Managed helper returns a structured result"],
        "riskRationale": "Fake SDK test fixture",
    }


def _default_result_message(session_id):
    return {
        "subtype": "success",
        "is_error": False,
        "session_id": session_id,
        "stop_reason": "end_turn",
        "result": "ok",
        "structured_output": _default_structured_output(),
    }


async def _run_hook_group(registry, hook_name, payload):
    if not registry:
        return
    for matcher in registry.get(hook_name, []):
        hooks = getattr(matcher, "hooks", [])
        if not isinstance(hooks, list):
            continue
        for hook in hooks:
            await hook(payload, payload.get("tool_use_id"), None)


def _assistant_content(step, decision):
    if isinstance(step.get("assistantContent"), list):
        return step["assistantContent"]
    text = step.get("assistantText")
    if not isinstance(text, str):
        suffix = " (interrupt)" if decision.get("interrupt") else ""
        text = f"{step.get('toolName')} => {decision['behavior']}{suffix}"
    return [{"type": "text", "text": text}]


def list_sessions(directory=None, include_worktrees=False):
    scenario = _load_scenario()
    sessions = scenario.get("sessions")
    if not isinstance(sessions, list):
        return []
    return [
        SimpleNamespace(
            session_id=item.get("sessionId", ""),
            summary=item.get("summary", ""),
            last_modified=item.get("lastModified", 0),
            file_size=item.get("fileSize"),
            custom_title=item.get("customTitle"),
            first_prompt=item.get("firstPrompt"),
            git_branch=item.get("gitBranch"),
            cwd=item.get("cwd", directory),
            tag=item.get("tag"),
            created_at=item.get("createdAt"),
        )
        for item in sessions
        if isinstance(item, dict)
    ]


def get_session_messages(provider_session_id, directory=None, limit=None, offset=0):
    scenario = _load_scenario()
    messages = scenario.get("sessionMessages")
    if not isinstance(messages, list):
        return []
    return [
        SimpleNamespace(
            type=item.get("type", ""),
            uuid=item.get("uuid", ""),
            session_id=item.get("session_id", provider_session_id),
            message=item.get("message"),
            parent_tool_use_id=item.get("parent_tool_use_id"),
        )
        for item in messages
        if isinstance(item, dict)
    ]


async def query(*args, **kwargs):
    if False:
        yield None


class ClaudeSDKClient:
    def __init__(self, options: ClaudeAgentOptions):
        self.options = options
        self.scenario = _load_scenario()

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc, tb):
        return False

    async def query(self, prompt):
        self.prompt = prompt

    async def receive_response(self):
        session_id = self.scenario.get("sessionId") or "fake-sdk-session"
        transcript_path = self.scenario.get("transcriptPath") or f"/tmp/{session_id}.jsonl"
        yield SystemMessage(
            subtype="init",
            data={
                "cwd": getattr(self.options, "cwd", None),
                "session_id": session_id,
                "tools": self.scenario.get("tools") or ["Read", "Edit", "AskUserQuestion"],
                "model": getattr(self.options, "model", None) or "claude-haiku-4-5-20251001",
                "permissionMode": getattr(self.options, "permission_mode", None) or "default",
            },
        )

        interrupted = False
        steps = self.scenario.get("steps")
        if isinstance(steps, list):
            for step in steps:
                if not isinstance(step, dict) or step.get("type") != "tool":
                    continue

                tool_input = step.get("input")
                if not isinstance(tool_input, dict):
                    tool_input = {}
                suggestions = step.get("suggestions")
                if not isinstance(suggestions, list):
                    suggestions = []

                hook_payload = {
                    "session_id": session_id,
                    "cwd": getattr(self.options, "cwd", None),
                    "transcript_path": transcript_path,
                    "tool_name": step.get("toolName"),
                    "tool_input": tool_input,
                    "tool_use_id": step.get("toolUseId"),
                    "title": step.get("title"),
                    "display_name": step.get("displayName"),
                    "description": step.get("description"),
                    "blocked_path": step.get("blockedPath"),
                    "decision_reason": step.get("decisionReason"),
                }

                if step.get("emitPermissionRequest", True):
                    await _run_hook_group(
                        getattr(self.options, "hooks", None),
                        "PermissionRequest",
                        {
                            **hook_payload,
                            "hook_event_name": "PermissionRequest",
                            "permission_suggestions": suggestions,
                        },
                    )

                await _run_hook_group(
                    getattr(self.options, "hooks", None),
                    "PreToolUse",
                    {
                        **hook_payload,
                        "hook_event_name": "PreToolUse",
                    },
                )

                decision = {"behavior": "allow", "interrupt": False}
                updated_input = tool_input
                can_use_tool = getattr(self.options, "can_use_tool", None)
                if can_use_tool is not None:
                    context = SimpleNamespace(
                        toolUseID=step.get("toolUseId"),
                        agentID=step.get("agentId"),
                        blockedPath=step.get("blockedPath"),
                        decisionReason=step.get("decisionReason"),
                        suggestions=suggestions,
                        title=step.get("title"),
                        displayName=step.get("displayName"),
                        description=step.get("description"),
                    )
                    result = await can_use_tool(step.get("toolName"), tool_input, context)
                    if isinstance(result, PermissionResultDeny):
                        decision = {
                            "behavior": "deny",
                            "interrupt": bool(getattr(result, "interrupt", False)),
                        }
                    else:
                        decision = {
                            "behavior": "allow",
                            "interrupt": bool(getattr(result, "interrupt", False)),
                        }
                        candidate = getattr(result, "updated_input", None)
                        if isinstance(candidate, dict):
                            updated_input = candidate

                if step.get("emitPostHook", True):
                    post_hook_name = step.get("postHook") or "PostToolUse"
                    await _run_hook_group(
                        getattr(self.options, "hooks", None),
                        post_hook_name,
                        {
                            **hook_payload,
                            "hook_event_name": post_hook_name,
                            "tool_input": updated_input,
                            "tool_response": step.get("toolResponse") or {"ok": True},
                        },
                    )

                yield AssistantMessage(
                    content=_assistant_content(step, decision),
                    model=getattr(self.options, "model", None),
                    parent_tool_use_id=step.get("toolUseId"),
                )

                if decision["interrupt"]:
                    interrupted = True
                    break

        if interrupted:
            yield ResultMessage(
                subtype="error",
                is_error=True,
                session_id=session_id,
                stop_reason="interrupted",
                result="aborted",
            )
            return

        result_message = self.scenario.get("resultMessage") or _default_result_message(session_id)
        yield ResultMessage(
            subtype=result_message.get("subtype"),
            duration_ms=result_message.get("duration_ms"),
            duration_api_ms=result_message.get("duration_api_ms"),
            is_error=result_message.get("is_error", False),
            num_turns=result_message.get("num_turns"),
            session_id=result_message.get("session_id", session_id),
            stop_reason=result_message.get("stop_reason"),
            total_cost_usd=result_message.get("total_cost_usd"),
            usage=result_message.get("usage"),
            model_usage=result_message.get("model_usage") or result_message.get("modelUsage"),
            permission_denials=result_message.get("permission_denials"),
            result=result_message.get("result"),
            structured_output=result_message.get("structured_output"),
            fast_mode_state=result_message.get("fast_mode_state"),
            uuid=result_message.get("uuid"),
        )
"#;
    fs::write(package_dir.join("__init__.py"), module).unwrap_or_else(|err| {
        panic!(
            "failed to write fake python sdk module '{}': {err}",
            package_dir.join("__init__.py").display()
        )
    });

    let types_module = r#"
from dataclasses import dataclass
from typing import Any


@dataclass
class HookMatcher:
    hooks: list[Any]


@dataclass
class PermissionUpdate:
    type: str
    destination: str
    mode: str


@dataclass
class PermissionResultAllow:
    updated_input: dict[str, Any] | None = None
    updated_permissions: list[PermissionUpdate] | None = None
    behavior: str = "allow"
    interrupt: bool = False


@dataclass
class PermissionResultDeny:
    message: str | None = None
    interrupt: bool = False
    behavior: str = "deny"


@dataclass
class ResultMessage:
    subtype: str | None = None
    duration_ms: int | None = None
    duration_api_ms: int | None = None
    is_error: bool | None = None
    num_turns: int | None = None
    session_id: str | None = None
    stop_reason: str | None = None
    total_cost_usd: float | None = None
    usage: Any = None
    model_usage: Any = None
    permission_denials: Any = None
    result: Any = None
    structured_output: Any = None
    fast_mode_state: Any = None
    uuid: str | None = None


@dataclass
class StreamEvent:
    uuid: str | None = None
    session_id: str | None = None
    event: Any = None
    parent_tool_use_id: str | None = None


@dataclass
class SystemMessage:
    subtype: str
    data: dict[str, Any]


@dataclass
class AssistantMessage:
    content: list[Any]
    model: str | None = None
    parent_tool_use_id: str | None = None
    error: Any = None
    usage: Any = None


@dataclass
class TextBlock:
    text: str


@dataclass
class ThinkingBlock:
    thinking: str
    signature: str | None = None


@dataclass
class ToolResultBlock:
    tool_use_id: str
    content: Any
    is_error: bool = False


@dataclass
class ToolUseBlock:
    id: str | None = None
    name: str | None = None
    input: Any = None


class ClaudeAgentOptions:
    def __init__(self, **kwargs):
        self.can_use_tool = None
        for key, value in kwargs.items():
            setattr(self, key, value)
"#;
    fs::write(package_dir.join("types.py"), types_module).unwrap_or_else(|err| {
        panic!(
            "failed to write fake python sdk types '{}': {err}",
            package_dir.join("types.py").display()
        )
    });
}

fn write_real_helper_wrapper(
    path: &Path,
    sdk_root: &Path,
    scenario_path: &Path,
    scripted_responses: Option<&str>,
) {
    let python_rendered = python3_executable_path().replace('\'', r#"'\''"#);
    let helper_rendered = embedded_claude_sdk_python_helper_path()
        .to_string_lossy()
        .replace('\'', r#"'\''"#);
    let sdk_rendered = sdk_root.to_string_lossy().replace('\'', r#"'\''"#);
    let scenario_rendered = scenario_path.to_string_lossy().replace('\'', r#"'\''"#);
    let scripted_export = scripted_responses.map_or_else(String::new, |responses| {
        format!(
            "export LIBRA_CLAUDE_HELPER_SCRIPTED_RESPONSES='{}'\n",
            responses.replace('\'', r#"'\''"#)
        )
    });
    let script = format!(
        "#!/bin/sh\nexport PYTHONPATH='{sdk_rendered}'${{PYTHONPATH:+\":$PYTHONPATH\"}}\nexport LIBRA_FAKE_CLAUDE_SDK_SCENARIO_PATH='{scenario_rendered}'\nexport ANTHROPIC_AUTH_TOKEN='test-token'\n{scripted_export}exec '{python_rendered}' '{helper_rendered}'\n"
    );
    fs::write(path, script)
        .unwrap_or_else(|err| panic!("failed to write helper wrapper '{}': {err}", path.display()));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|err| panic!("failed to stat helper wrapper '{}': {err}", path.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap_or_else(|err| {
        panic!(
            "failed to set executable permissions on helper wrapper '{}': {err}",
            path.display()
        )
    });
}

fn write_fake_interactive_helper(
    repo: &Path,
    scenario: &Value,
    scripted_responses: Option<&Value>,
) -> PathBuf {
    let sdk_root = repo.join("fake-claude-agent-sdk-python");
    write_fake_claude_agent_sdk_python_package(&sdk_root);

    let scenario_path = repo.join("fake-claude-scenario.json");
    fs::write(
        &scenario_path,
        serde_json::to_vec_pretty(scenario).expect("serialize fake sdk scenario"),
    )
    .expect("write fake sdk scenario");

    let helper_path = repo.join("real-helper-wrapper.sh");
    let scripted_responses = scripted_responses
        .map(|value| serde_json::to_string(value).expect("serialize scripted helper responses"));
    write_real_helper_wrapper(
        &helper_path,
        &sdk_root,
        &scenario_path,
        scripted_responses.as_deref(),
    );
    helper_path
}

fn shell_double_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
    )
}

fn build_live_claude_sdk_command(
    repo: &Path,
    prompt_path: &Path,
    permission_mode: &str,
    timeout_seconds: &str,
    tools: &[&str],
    scripted_responses: Option<&str>,
) -> Command {
    let libra_bin = env!("CARGO_BIN_EXE_libra");
    let shell_override = std::env::var("LIBRA_TEST_CLAUDE_LIVE_SHELL")
        .ok()
        .filter(|value| !value.is_empty());
    let sdk_module_override = std::env::var("LIBRA_CLAUDE_AGENT_SDK_MODULE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let default_path = "/tmp/claude-sdk-probe/node_modules/@anthropic-ai/claude-agent-sdk";
            Path::new(default_path)
                .exists()
                .then(|| default_path.to_string())
        });

    let mut command = if let Some(shell) = shell_override.as_deref() {
        let tool_flags = tools
            .iter()
            .map(|tool| format!(" --tool {}", shell_double_quote(tool)))
            .collect::<String>();
        let shell_command = format!(
            "cd {}; {} --json=ndjson claude-sdk run --prompt-file {} --model haiku --permission-mode {} --timeout-seconds {}{} --enable-file-checkpointing true",
            shell_double_quote(repo.to_str().expect("repo path should be valid UTF-8")),
            shell_double_quote(libra_bin),
            shell_double_quote(
                prompt_path
                    .to_str()
                    .expect("prompt path should be valid UTF-8")
            ),
            shell_double_quote(permission_mode),
            shell_double_quote(timeout_seconds),
            tool_flags,
        );
        let mut command = Command::new(shell);
        command.arg("-lc").arg(shell_command);
        command
    } else {
        let mut command = Command::new(libra_bin);
        let mut args = vec![
            "--json=ndjson".to_string(),
            "claude-sdk".to_string(),
            "run".to_string(),
            "--prompt-file".to_string(),
            prompt_path
                .to_str()
                .expect("prompt path should be valid UTF-8")
                .to_string(),
            "--model".to_string(),
            "haiku".to_string(),
            "--permission-mode".to_string(),
            permission_mode.to_string(),
            "--timeout-seconds".to_string(),
            timeout_seconds.to_string(),
        ];
        for tool in tools {
            args.push("--tool".to_string());
            args.push((*tool).to_string());
        }
        args.push("--enable-file-checkpointing".to_string());
        args.push("true".to_string());
        command.current_dir(repo).args(args);
        command
    };

    if let Some(module_path) = sdk_module_override.as_deref() {
        command.env("LIBRA_CLAUDE_AGENT_SDK_MODULE", module_path);
    }
    if let Some(scripted) = scripted_responses {
        command.env("LIBRA_CLAUDE_HELPER_SCRIPTED_RESPONSES", scripted);
    }
    command
}

fn replace_template_slots(node: &mut Value, replacements: &[(&str, Value)]) {
    match node {
        Value::Array(items) => {
            for item in items {
                replace_template_slots(item, replacements);
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                replace_template_slots(value, replacements);
            }
        }
        Value::String(slot) => {
            if let Some((_, replacement)) = replacements.iter().find(|(key, _)| *key == slot) {
                *node = replacement.clone();
            }
        }
        _ => {}
    }
}

fn managed_artifact_from_template(
    template: &str,
    repo: &Path,
    touched_file: &Path,
    prompt: &str,
) -> Value {
    let mut artifact: Value = serde_json::from_str(template)
        .unwrap_or_else(|err| panic!("failed to parse managed artifact template: {err}"));
    let replacements = [
        ("__CWD__", json!(repo.to_string_lossy().to_string())),
        (
            "__TOUCHED_FILE__",
            json!(touched_file.to_string_lossy().to_string()),
        ),
        ("__PROMPT__", json!(prompt)),
    ];
    replace_template_slots(&mut artifact, &replacements);
    artifact
}

fn semantic_full_artifact(repo: &Path, touched_file: &Path) -> Value {
    managed_artifact_from_template(
        SEMANTIC_FULL_TEMPLATE,
        repo,
        touched_file,
        DEFAULT_MANAGED_PROMPT,
    )
}

fn plan_task_only_artifact(repo: &Path, touched_file: &Path) -> Value {
    managed_artifact_from_template(PLAN_TASK_ONLY_TEMPLATE, repo, touched_file, PLAN_PROMPT)
}

fn semantic_edit_artifact(
    repo: &Path,
    touched_file: &Path,
    old_text: &str,
    new_text: &str,
) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let hook_events = artifact["hookEvents"]
        .as_array_mut()
        .expect("semantic full artifact should contain hook events");
    for event in hook_events {
        let is_post_tool_use = event.get("hook") == Some(&json!("PostToolUse"));
        let Some(input) = event.get_mut("input").and_then(Value::as_object_mut) else {
            continue;
        };
        if input.get("tool_name") != Some(&json!("Read")) {
            continue;
        }
        input.insert("tool_name".to_string(), json!("Edit"));
        input.insert(
            "tool_input".to_string(),
            json!({
                "file_path": touched_file.to_string_lossy().to_string(),
                "old_string": old_text,
                "new_string": new_text
            }),
        );
        if is_post_tool_use {
            input.insert(
                "tool_response".to_string(),
                json!({
                    "file": {
                        "filePath": touched_file.to_string_lossy().to_string()
                    }
                }),
            );
        }
    }
    artifact
}

fn real_plan_step_descriptions() -> Vec<String> {
    vec![
        "**Inspect Bridge Architecture** -> Identify initialization sequence, provider contracts, and task event flow between Claude SDK and Libra components".to_string(),
        "**Audit Runtime Behavior** -> Trace provider-native facts (actual type signatures, event payloads, transformation logic) vs semantic candidate interpretations".to_string(),
        "**Extract Structured Intent** -> Map observed runtime evidence to semantic intent model with grounded validation".to_string(),
    ]
}

fn test_change_type_artifact(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let structured_output = artifact["resultMessage"]["structured_output"]
        .as_object_mut()
        .expect("semantic full artifact should contain structured_output");
    structured_output.insert(
        "summary".to_string(),
        json!("Add regression coverage for Claude SDK bridge persistence"),
    );
    structured_output.insert(
        "problemStatement".to_string(),
        json!("The Claude SDK bridge needs explicit regression coverage for persisted artifacts."),
    );
    structured_output.insert("changeType".to_string(), json!("test"));
    structured_output.insert(
        "objectives".to_string(),
        json!(["Add an integration regression test for persisted Claude SDK artifacts"]),
    );
    structured_output.insert(
        "successCriteria".to_string(),
        json!(["Regression test passes and covers persisted artifact behavior"]),
    );
    structured_output.insert(
        "riskRationale".to_string(),
        json!("The change is test-only and should not affect production behavior."),
    );
    artifact
}

fn timed_out_partial_artifact(repo: &Path, touched_file: &Path) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let object = artifact
        .as_object_mut()
        .expect("semantic full artifact should be an object");
    object.insert("helperTimedOut".to_string(), json!(true));
    object.insert(
        "helperError".to_string(),
        json!("Claude SDK helper timed out"),
    );
    object.insert("resultMessage".to_string(), Value::Null);
    artifact
}

fn errored_result_artifact(repo: &Path, touched_file: &Path, detail: &str) -> Value {
    let mut artifact = semantic_full_artifact(repo, touched_file);
    let result = artifact["resultMessage"]
        .as_object_mut()
        .expect("semantic full artifact should contain resultMessage");
    result.insert("subtype".to_string(), json!("error"));
    result.insert("is_error".to_string(), json!(true));
    result.insert("stop_reason".to_string(), json!("error"));
    result.insert("result".to_string(), json!(detail));
    artifact
}

async fn load_intent_history(repo: &Path) -> (Arc<LocalStorage>, HistoryManager) {
    let libra_dir = repo.join(".libra");
    let storage = Arc::new(LocalStorage::new(libra_dir.join("objects")));
    let db_conn = Arc::new(
        libra::internal::db::establish_connection(
            libra_dir
                .join("libra.db")
                .to_str()
                .expect("db path should be valid UTF-8"),
        )
        .await
        .expect("failed to connect test database"),
    );
    let history = HistoryManager::new(storage.clone(), libra_dir, db_conn);
    (storage, history)
}

async fn read_tracked_object<T>(repo: &Path, object_type: &str, object_id: &str) -> T
where
    T: DeserializeOwned + Send + Sync,
{
    let (storage, history) = load_intent_history(repo).await;
    let hash = history
        .get_object_hash(object_type, object_id)
        .await
        .expect("should query object hash")
        .unwrap_or_else(|| panic!("expected {object_type} object '{object_id}' to exist"));
    storage
        .get_json::<T>(&hash)
        .await
        .unwrap_or_else(|err| panic!("failed to load {object_type} '{object_id}': {err}"))
}

async fn list_history_object_ids(repo: &Path, object_type: &str) -> Vec<String> {
    let (_, history) = load_intent_history(repo).await;
    history
        .list_objects(object_type)
        .await
        .unwrap_or_else(|err| panic!("failed to list {object_type} objects: {err}"))
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

fn assert_ai_type_matches(repo: &Path, object_id: &str, expected_type: &str) {
    let selector = format!("{expected_type}:{object_id}");
    let output = run_libra_command(&["cat-file", "--ai-type", &selector], repo);
    assert_cli_success(&output, "cat-file --ai-type should succeed");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        expected_type
    );
}

fn session_messages_fixture(session_id: &str) -> Value {
    json!([
        {
            "type": "system",
            "subtype": "init",
            "session_id": session_id,
            "uuid": "msg-1"
        },
        {
            "type": "user",
            "session_id": session_id,
            "message": {
                "content": [
                    {
                        "type": "text",
                        "text": "Inspect src/lib.rs and summarize the bridge state."
                    }
                ]
            }
        },
        {
            "type": "assistant",
            "session_id": session_id,
            "uuid": "msg-2",
            "message": {
                "content": [
                    {
                        "type": "text",
                        "text": "I will inspect src/lib.rs and then summarize the current bridge shape."
                    },
                    {
                        "type": "tool_use",
                        "name": "Read",
                        "input": {
                            "file_path": "src/lib.rs"
                        }
                    }
                ]
            }
        },
        {
            "type": "system",
            "subtype": "task_progress",
            "session_id": session_id,
            "uuid": "msg-task-1",
            "description": "Reading provider runtime facts"
        },
        {
            "type": "tool_progress",
            "tool_use_id": "tool-1",
            "tool_name": "Read",
            "elapsed_time_seconds": 1,
            "session_id": session_id,
            "uuid": "msg-3"
        },
        {
            "type": "result",
            "subtype": "success",
            "session_id": session_id,
            "uuid": "msg-4",
            "duration_ms": 10,
            "duration_api_ms": 8,
            "is_error": false,
            "num_turns": 1,
            "result": "ok",
            "stop_reason": "end_turn",
            "total_cost_usd": 0.01,
            "permission_denials": [
                {
                    "tool_name": "Edit"
                }
            ],
            "structured_output": {
                "summary": "Separate runtime facts from semantic candidates"
            },
            "usage": {}
        }
    ])
}

fn stage_provider_session_evidence_artifacts(repo: &Path, provider_session_id: &str) {
    let catalog_response_path = repo.join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": provider_session_id,
                "summary": "Claude provider session fixture",
                "lastModified": 1742025600000i64,
                "cwd": repo.to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.join("session-catalog-request.json");
    let catalog_helper_path = repo.join("fake-session-catalog-helper.sh");
    write_json_response_capture_shell_helper(
        &catalog_helper_path,
        &catalog_response_path,
        &catalog_request_path,
    );

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
        ],
        repo,
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should succeed for formal bridge tests",
    );

    let messages_response_path = repo.join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&session_messages_fixture(provider_session_id))
            .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.join("session-messages-request.json");
    let messages_helper_path = repo.join("fake-session-messages-helper.sh");
    write_json_response_capture_shell_helper(
        &messages_helper_path,
        &messages_response_path,
        &messages_request_path,
    );

    let hydrate = run_libra_command(
        &[
            "claude-sdk",
            "hydrate-session",
            "--provider-session-id",
            provider_session_id,
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
        ],
        repo,
    );
    assert_cli_success(
        &hydrate,
        "claude-sdk hydrate-session should succeed for formal bridge tests",
    );

    let build = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            provider_session_id,
        ],
        repo,
    );
    assert_cli_success(
        &build,
        "claude-sdk build-evidence-input should succeed for formal bridge tests",
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_import_persists_bridge_artifacts_and_is_idempotent() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let artifact_path = repo.path().join("probe-like-artifact.json");
    fs::write(&artifact_path, PROBE_LIKE_ARTIFACT).expect("failed to stage managed artifact");

    let first = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            artifact_path.to_str().expect("artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&first, "claude-sdk import should succeed");
    let first_json = parse_stdout_json(&first, "first import");

    assert_eq!(first_json["ok"], json!(true));
    assert_eq!(first_json["mode"], json!("import"));
    assert_eq!(first_json["alreadyPersisted"], json!(false));
    assert!(
        first_json["intentExtractionPath"].is_null(),
        "probe-like fixture should not yield an intent extraction"
    );

    let ai_session_id = first_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");
    let raw_artifact_path = PathBuf::from(
        first_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath"),
    );
    let audit_bundle_path = PathBuf::from(
        first_json["auditBundlePath"]
            .as_str()
            .expect("auditBundlePath"),
    );

    assert!(
        raw_artifact_path.exists(),
        "raw artifact should be materialized"
    );
    assert!(
        audit_bundle_path.exists(),
        "audit bundle should be materialized"
    );

    let audit_bundle = read_json_file(&audit_bundle_path);
    assert_eq!(
        audit_bundle["bridge"]["intentExtraction"]["status"],
        json!("invalid")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runSnapshot"]["id"],
        json!(format!("{ai_session_id}::run"))
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runEvent"]["status"],
        json!("completed")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["provenanceSnapshot"]["provider"],
        json!("claude")
    );
    assert_eq!(
        audit_bundle["bridge"]["aiSession"]["schema"],
        json!("libra.ai_session.v2")
    );

    let ai_type = run_libra_command(&["cat-file", "--ai-type", ai_session_id], repo.path());
    assert_cli_success(&ai_type, "cat-file --ai-type should succeed");
    assert_eq!(
        String::from_utf8_lossy(&ai_type.stdout).trim(),
        "ai_session"
    );

    let ai_pretty = run_libra_command(&["cat-file", "--ai", ai_session_id], repo.path());
    assert_cli_success(&ai_pretty, "cat-file --ai should succeed");
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("schema: libra.ai_session.v2"));
    assert!(ai_pretty_stdout.contains("provider: claude"));

    let second = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            artifact_path.to_str().expect("artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&second, "second claude-sdk import should succeed");
    let second_json = parse_stdout_json(&second, "second import");
    assert_eq!(second_json["alreadyPersisted"], json!(true));
    assert_eq!(second_json["aiSessionId"], json!(ai_session_id));
    assert_eq!(
        second_json["aiSessionObjectHash"],
        first_json["aiSessionObjectHash"]
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_with_custom_helper_persists_intent_extraction() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_json = semantic_full_artifact(repo.path(), &touched_file);

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact_json).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run");

    assert_eq!(run_json["ok"], json!(true));
    assert_eq!(run_json["mode"], json!("run"));
    assert_eq!(run_json["alreadyPersisted"], json!(false));

    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");
    let intent_extraction_path = PathBuf::from(
        run_json["intentExtractionPath"]
            .as_str()
            .expect("intentExtractionPath"),
    );
    let raw_artifact_path = PathBuf::from(
        run_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath"),
    );
    let audit_bundle_path = PathBuf::from(
        run_json["auditBundlePath"]
            .as_str()
            .expect("auditBundlePath"),
    );

    assert!(
        intent_extraction_path.exists(),
        "intent extraction should be persisted"
    );
    assert!(
        raw_artifact_path.exists(),
        "raw artifact should be materialized"
    );
    assert!(
        audit_bundle_path.exists(),
        "audit bundle should be materialized"
    );

    let intent_extraction = read_json_file(&intent_extraction_path);
    assert_eq!(
        intent_extraction["schema"],
        json!("libra.intent_extraction.v2")
    );
    assert_eq!(
        intent_extraction["extraction"]["intent"]["summary"],
        json!("Persist the Claude SDK managed bridge")
    );
    assert_eq!(
        intent_extraction["extraction"]["intent"]["inScope"],
        json!(["src/lib.rs"])
    );
    assert_eq!(
        intent_extraction["extraction"]["acceptance"]["fastChecks"],
        json!([])
    );
    assert!(intent_extraction.get("planningSummary").is_none());

    let audit_bundle = read_json_file(&audit_bundle_path);
    assert_eq!(
        audit_bundle["bridge"]["intentExtraction"]["status"],
        json!("accepted")
    );
    assert_eq!(
        audit_bundle["bridge"]["intentExtractionArtifact"]["schema"],
        json!("libra.intent_extraction.v2")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runSnapshot"]["id"],
        json!(format!("{ai_session_id}::run"))
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runEvent"]["status"],
        json!("completed")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["toolInvocationEvents"][0]["tool"],
        json!("Read")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["providerInitSnapshot"]["apiKeySource"],
        json!("oauth")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["providerInitSnapshot"]["claudeCodeVersion"],
        json!("2.1.76")
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["taskRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(4)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["toolRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["assistantRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["decisionRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(4)
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["contextRuntimeEvents"]
            .as_array()
            .map(Vec::len),
        Some(7)
    );
    assert_eq!(
        audit_bundle["bridge"]["sessionState"]["metadata"]["provider_runtime"]["providerInit"]["apiKeySource"],
        json!("oauth")
    );
    assert!(
        audit_bundle["bridge"]["touchHints"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "src/lib.rs")),
        "touch hints should include the repo-relative file observed from tool evidence"
    );
    assert!(
        audit_bundle["fieldProvenance"]
            .as_array()
            .is_some_and(|entries| entries
                .iter()
                .any(|entry| entry["fieldPath"] == "runtime.assistantEvents")),
        "assistant stream runtime facts should be recorded in field provenance"
    );

    let ai_pretty = run_libra_command(&["cat-file", "--ai", ai_session_id], repo.path());
    assert_cli_success(&ai_pretty, "cat-file --ai should succeed for run path");
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("schema: libra.ai_session.v2"));
    assert!(ai_pretty_stdout.contains("provider: claude"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_can_disable_auto_tool_approval() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["requestContext"] = json!({
        "enableFileCheckpointing": true
    });
    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Read",
            "--auto-approve-tools",
            "false",
            "--include-partial-messages",
            "true",
            "--prompt-suggestions",
            "true",
            "--agent-progress-summaries",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed when auto tool approval is disabled",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["tools"], json!(["Read"]));
    assert_eq!(helper_request["allowedTools"], json!(["Read"]));
    assert_eq!(helper_request["autoApproveTools"], json!(false));
    assert_eq!(helper_request["includePartialMessages"], json!(true));
    assert_eq!(helper_request["promptSuggestions"], json!(true));
    assert_eq!(helper_request["agentProgressSummaries"], json!(true));
    assert_eq!(helper_request["systemPrompt"]["type"], json!("preset"));
    assert_eq!(
        helper_request["systemPrompt"]["preset"],
        json!("claude_code")
    );
    assert!(
        helper_request["systemPrompt"]["append"]
            .as_str()
            .is_some_and(|text| text.contains("concise numbered 3-step plan")),
        "run helper request should carry the built-in Claude system prompt extension"
    );
    assert!(
        helper_request["outputSchema"]["properties"]
            .get("planningSummary")
            .is_some()
    );
    assert!(
        helper_request["outputSchema"]["properties"]
            .get("inScope")
            .is_none()
    );
    assert!(
        helper_request["outputSchema"]["properties"]
            .get("fastChecks")
            .is_none()
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_python_backend_uses_python_binary_and_preserves_request_shape() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn python_backend_request() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("python-helper-request.json");
    let helper_path = repo.path().join("capture-managed-python-helper.py");
    write_json_response_capture_python_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Read",
            "--auto-approve-tools",
            "false",
            "--include-partial-messages",
            "true",
            "--interactive-approvals",
            "true",
            "--enable-file-checkpointing",
            "true",
            "--continue",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
            "--python-binary",
            "python3",
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should use the explicit python backend helper path",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["mode"], json!("query"));
    let helper_cwd = PathBuf::from(
        helper_request["cwd"]
            .as_str()
            .expect("helper request cwd should be present"),
    );
    assert_eq!(
        helper_cwd.canonicalize().expect("canonicalize helper cwd"),
        repo.path().canonicalize().expect("canonicalize repo path")
    );
    assert_eq!(helper_request["tools"], json!(["Read"]));
    assert_eq!(helper_request["allowedTools"], json!(["Read"]));
    assert_eq!(helper_request["autoApproveTools"], json!(false));
    assert_eq!(helper_request["includePartialMessages"], json!(true));
    assert_eq!(helper_request["interactiveApprovals"], json!(true));
    assert_eq!(helper_request["enableFileCheckpointing"], json!(true));
    assert_eq!(helper_request["continue"], json!(true));
    assert!(helper_request.get("resume").is_none());
    assert!(helper_request.get("sessionId").is_none());
    assert!(helper_request.get("resumeSessionAt").is_none());
    assert_eq!(helper_request["systemPrompt"]["type"], json!("preset"));
    assert_eq!(
        helper_request["systemPrompt"]["preset"],
        json!("claude_code")
    );
    assert!(
        helper_request["systemPrompt"]["append"]
            .as_str()
            .is_some_and(|text| text.contains("concise numbered 3-step plan")),
        "python backend should preserve the Claude system prompt extension"
    );
    assert!(
        helper_request["outputSchema"]["properties"]
            .get("summary")
            .is_some(),
        "python backend should keep the managed output schema"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_forwards_interactive_approvals_flag() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn interactive_approvals() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Bash",
            "--interactive-approvals",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed when interactive approvals are enabled",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["interactiveApprovals"], json!(true));
    assert_eq!(
        helper_request["autoApproveTools"],
        json!(false),
        "interactive approvals should override helper-side auto approval"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_streams_ndjson_by_default() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "stream-default-session",
            "resultMessage": {
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "session_id": "stream-default-session",
                "stop_reason": "end_turn",
                "result": "ok",
                "structured_output": {
                    "summary": "Stream the helper output",
                    "problemStatement": "Verify claude-sdk run defaults to NDJSON streaming when requested.",
                    "changeType": "refactor",
                    "objectives": ["Stream NDJSON events"],
                    "successCriteria": ["NDJSON events are emitted"],
                    "riskRationale": "Fake SDK streaming smoke test"
                }
            }
        }),
        None,
    );

    let run = run_libra_command_raw(
        &[
            "--json=ndjson",
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "streaming claude-sdk run should succeed");

    let events = parse_stdout_ndjson(&run, "streaming claude-sdk run output");
    assert!(
        events
            .iter()
            .any(|event| event["event"] == json!("session_init")),
        "streaming run should emit session_init"
    );
    assert!(
        events
            .iter()
            .any(|event| event["event"] == json!("runtime_snapshot")),
        "streaming run should emit runtime_snapshot"
    );
    assert!(
        events
            .iter()
            .any(|event| event["event"] == json!("final_artifact")),
        "streaming run should emit final_artifact"
    );
    let result_event = find_ndjson_event(&events, "libra_result", "streaming run result");
    let raw_artifact_path = PathBuf::from(
        result_event["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath should be present"),
    );
    assert!(
        raw_artifact_path.exists(),
        "streaming run should still persist the managed artifact"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_streaming_persists_managed_inputs_without_manual_subcommands() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "old\n").expect("write source file");

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "stream-managed-inputs-session",
            "steps": [
                {
                    "type": "tool",
                    "toolName": "Edit",
                    "toolUseId": "tool-edit-stream-1",
                    "input": {
                        "file_path": touched_file.to_string_lossy().to_string(),
                        "old_string": "old\n",
                        "new_string": "new\n"
                    }
                }
            ],
            "resultMessage": {
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "session_id": "stream-managed-inputs-session",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 32,
                    "output_tokens": 24,
                    "service_tier": "standard"
                },
                "structured_output": {
                    "summary": "Persist managed inputs during streaming run",
                    "problemStatement": "Verify streaming run writes provider-owned managed input layers automatically.",
                    "changeType": "refactor",
                    "objectives": ["Persist managed inputs automatically"],
                    "successCriteria": ["Managed input history objects exist after run"],
                    "riskRationale": "Fake SDK streaming persistence test"
                }
            }
        }),
        None,
    );

    let run = run_libra_command_raw(
        &[
            "--json=ndjson",
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Edit",
            "--permission-mode",
            "acceptEdits",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "streaming run should succeed");

    let events = parse_stdout_ndjson(&run, "streaming managed inputs output");
    let result_event = find_ndjson_event(&events, "libra_result", "streaming managed inputs");
    let ai_session_id = result_event["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let (_, history) = load_intent_history(repo.path()).await;
    let managed_input_object_id = format!("claude_managed_evidence_input__{ai_session_id}");
    let decision_input_object_id = format!("claude_decision_input__{ai_session_id}");
    let run_binding_path = repo
        .path()
        .join(".libra")
        .join("claude-run-bindings")
        .join(format!("{ai_session_id}.json"));
    let tool_invocation_binding_path = repo
        .path()
        .join(".libra")
        .join("claude-tool-invocation-bindings")
        .join(format!("{ai_session_id}.json"));

    let managed_input_type = run_libra_command(
        &["cat-file", "--ai-type", &managed_input_object_id],
        repo.path(),
    );
    assert_cli_success(
        &managed_input_type,
        "streaming run should persist managed evidence input automatically",
    );
    assert_eq!(
        String::from_utf8_lossy(&managed_input_type.stdout).trim(),
        "claude_managed_evidence_input"
    );

    let decision_input_type = run_libra_command(
        &["cat-file", "--ai-type", &decision_input_object_id],
        repo.path(),
    );
    assert_cli_success(
        &decision_input_type,
        "streaming run should persist decision input automatically",
    );
    assert_eq!(
        String::from_utf8_lossy(&decision_input_type.stdout).trim(),
        "claude_decision_input"
    );

    let managed_inputs = history
        .list_objects("claude_managed_evidence_input")
        .await
        .expect("list managed evidence inputs");
    assert_eq!(managed_inputs.len(), 1);
    let decision_inputs = history
        .list_objects("claude_decision_input")
        .await
        .expect("list decision inputs");
    assert_eq!(decision_inputs.len(), 1);

    assert!(
        run_binding_path.exists(),
        "streaming run should materialize a formal run binding without manual bridge commands"
    );
    let run_binding = read_json_file(&run_binding_path);
    let task_id = run_binding["taskId"]
        .as_str()
        .expect("taskId should be present in run binding")
        .to_string();
    let run_id = run_binding["runId"]
        .as_str()
        .expect("runId should be present in run binding")
        .to_string();

    let tasks = history
        .list_objects("task")
        .await
        .expect("list formal tasks");
    assert_eq!(tasks.len(), 1);
    let runs = history.list_objects("run").await.expect("list formal runs");
    assert_eq!(runs.len(), 1);
    let provenances = history
        .list_objects("provenance")
        .await
        .expect("list formal provenances");
    assert_eq!(provenances.len(), 1);
    let run_usages = history
        .list_objects("run_usage")
        .await
        .expect("list formal run usage objects");
    assert_eq!(run_usages.len(), 1);
    let snapshots = history
        .list_objects("snapshot")
        .await
        .expect("list formal context snapshots");
    assert_eq!(snapshots.len(), 1);
    let context_frames = history
        .list_objects("context_frame")
        .await
        .expect("list formal context frames");
    assert!(
        !context_frames.is_empty(),
        "streaming run should materialize at least one context frame"
    );
    assert_eq!(tasks[0].0, task_id);
    assert_eq!(runs[0].0, run_id);

    let _: Task = read_tracked_object(repo.path(), "task", &task_id).await;
    let formal_run: Run = read_tracked_object(repo.path(), "run", &run_id).await;
    assert_eq!(formal_run.task().to_string(), task_id);
    let provenance: Provenance =
        read_tracked_object(repo.path(), "provenance", &provenances[0].0).await;
    assert_eq!(provenance.run_id().to_string(), run_id);
    assert_eq!(provenance.provider(), "claude");
    let run_usage: RunUsage = read_tracked_object(repo.path(), "run_usage", &run_usages[0].0).await;
    assert_eq!(run_usage.run_id().to_string(), run_id);
    assert!(run_usage.input_tokens() > 0);
    assert!(run_usage.output_tokens() > 0);

    assert!(
        tool_invocation_binding_path.exists(),
        "streaming run should materialize a shared tool invocation binding"
    );
    let tool_invocation_binding = read_json_file(&tool_invocation_binding_path);
    let invocation_entries = tool_invocation_binding["invocations"]
        .as_array()
        .expect("tool invocation binding should contain invocations");
    assert_eq!(invocation_entries.len(), 1);
    let tool_invocation_id = invocation_entries[0]["toolInvocationId"]
        .as_str()
        .expect("toolInvocationId should be present");

    let invocations = history
        .list_objects("invocation")
        .await
        .expect("list shared invocations");
    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].0, tool_invocation_id);

    let tool_invocation_type =
        run_libra_command(&["cat-file", "--ai-type", tool_invocation_id], repo.path());
    assert_cli_success(
        &tool_invocation_type,
        "streaming run should expose shared invocation through cat-file",
    );
    assert_eq!(
        String::from_utf8_lossy(&tool_invocation_type.stdout).trim(),
        "invocation"
    );

    let provenance_type =
        run_libra_command(&["cat-file", "--ai-type", &provenances[0].0], repo.path());
    assert_cli_success(
        &provenance_type,
        "streaming run should expose provenance through cat-file",
    );
    assert_eq!(
        String::from_utf8_lossy(&provenance_type.stdout).trim(),
        "provenance"
    );

    let run_usage_type =
        run_libra_command(&["cat-file", "--ai-type", &run_usages[0].0], repo.path());
    assert_cli_success(
        &run_usage_type,
        "streaming run should expose run usage through cat-file",
    );
    assert_eq!(
        String::from_utf8_lossy(&run_usage_type.stdout).trim(),
        "run_usage"
    );

    let snapshot_type = run_libra_command(&["cat-file", "--ai-type", &snapshots[0].0], repo.path());
    assert_cli_success(
        &snapshot_type,
        "streaming run should expose context snapshot through cat-file",
    );
    assert_eq!(
        String::from_utf8_lossy(&snapshot_type.stdout).trim(),
        "snapshot"
    );

    let context_frame_type = run_libra_command(
        &["cat-file", "--ai-type", &context_frames[0].0],
        repo.path(),
    );
    assert_cli_success(
        &context_frame_type,
        "streaming run should expose context frame through cat-file",
    );
    assert_eq!(
        String::from_utf8_lossy(&context_frame_type.stdout).trim(),
        "context_frame"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_streaming_persists_derived_audit_objects() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;
    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "old\n").expect("write source file");

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "stream-audit-objects-session",
            "steps": [
                {
                    "type": "tool",
                    "toolName": "Edit",
                    "toolUseId": "tool-edit-audit-1",
                    "title": "Claude wants to edit src/lib.rs",
                    "displayName": "Edit file",
                    "description": "Claude needs write access to update the target file.",
                    "suggestions": [
                        {
                            "type": "setMode",
                            "destination": "session",
                            "mode": "acceptEdits"
                        }
                    ],
                    "input": {
                        "file_path": touched_file.to_string_lossy().to_string(),
                        "old_string": "old\n",
                        "new_string": "new\n"
                    },
                    "assistantContent": [
                        {
                            "type": "thinking",
                            "thinking": "I need approval before editing the file.",
                            "signature": "sig_audit_reasoning"
                        },
                        {
                            "type": "text",
                            "text": "I'll edit src/lib.rs once approved."
                        }
                    ]
                }
            ],
            "resultMessage": {
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "session_id": "stream-audit-objects-session",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 40,
                    "output_tokens": 20,
                    "service_tier": "standard"
                },
                "structured_output": {
                    "summary": "Persist derived audit objects",
                    "problemStatement": "Verify Claude-native permission and thinking signals are persisted as derived audit objects.",
                    "changeType": "refactor",
                    "objectives": ["Persist approval_request, reasoning, and tool_invocation_event"],
                    "successCriteria": ["Derived audit objects exist after the run"],
                    "riskRationale": "Fake SDK audit-object regression"
                }
            }
        }),
        Some(&json!([
            {
                "kind": "tool_approval",
                "decision": "approve"
            }
        ])),
    );

    let run = run_libra_command_raw(
        &[
            "--json=ndjson",
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Edit",
            "--interactive-approvals",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "streaming audit-object run should succeed");

    let events = parse_stdout_ndjson(&run, "streaming audit objects output");
    let result_event = find_ndjson_event(&events, "libra_result", "streaming audit objects");
    let ai_session_id = result_event["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let (_, history) = load_intent_history(repo.path()).await;
    let approvals = history
        .list_objects("approval_request")
        .await
        .expect("list approval_request objects");
    assert_eq!(approvals.len(), 1);
    let reasonings = history
        .list_objects("reasoning")
        .await
        .expect("list reasoning objects");
    assert_eq!(reasonings.len(), 1);
    let tool_events = history
        .list_objects("tool_invocation_event")
        .await
        .expect("list tool_invocation_event objects");
    assert_eq!(tool_events.len(), 2);

    let approval: Value =
        read_tracked_object(repo.path(), "approval_request", &approvals[0].0).await;
    assert_eq!(approval["object_type"], json!("approval_request"));
    assert_eq!(
        approval["sourceKind"],
        json!("derived_from_claude_native_signal")
    );
    assert_eq!(approval["aiSessionId"], json!(ai_session_id.clone()));
    assert_eq!(approval["toolUseId"], json!("tool-edit-audit-1"));
    assert_eq!(approval["toolName"], json!("Edit"));
    assert_eq!(approval["status"], json!("approved_once"));
    assert_eq!(approval["decision"], json!(true));
    assert_eq!(approval["title"], json!("Claude wants to edit src/lib.rs"));
    assert_eq!(approval["displayName"], json!("Edit file"));

    let reasoning: Value = read_tracked_object(repo.path(), "reasoning", &reasonings[0].0).await;
    assert_eq!(reasoning["object_type"], json!("reasoning"));
    assert_eq!(
        reasoning["sourceKind"],
        json!("derived_from_claude_native_signal")
    );
    assert_eq!(reasoning["signature"], json!("sig_audit_reasoning"));
    assert_eq!(
        reasoning["text"],
        json!("I need approval before editing the file.")
    );

    let tool_event_values = futures::future::join_all(
        tool_events
            .iter()
            .map(|(id, _)| read_tracked_object::<Value>(repo.path(), "tool_invocation_event", id)),
    )
    .await;
    let statuses = tool_event_values
        .iter()
        .map(|value| {
            value["status"]
                .as_str()
                .expect("tool event status should exist")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        statuses,
        BTreeSet::from(["completed".to_string(), "in_progress".to_string()])
    );

    for (object_id, object_type) in [
        (approvals[0].0.as_str(), "approval_request"),
        (reasonings[0].0.as_str(), "reasoning"),
        (tool_events[0].0.as_str(), "tool_invocation_event"),
    ] {
        let ai_type = run_libra_command(&["cat-file", "--ai-type", object_id], repo.path());
        assert_cli_success(
            &ai_type,
            "cat-file --ai-type should succeed for derived audit objects",
        );
        assert_eq!(String::from_utf8_lossy(&ai_type.stdout).trim(), object_type);
    }
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_forwards_enable_file_checkpointing_flag() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn checkpointing() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["requestContext"] = json!({
        "enableFileCheckpointing": true
    });
    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--enable-file-checkpointing",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should forward file checkpointing");

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["enableFileCheckpointing"], json!(true));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_forwards_continue_flag() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn continue_session() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["requestContext"] = json!({
        "enableFileCheckpointing": true
    });
    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--continue",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should forward --continue");

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["continue"], json!(true));
    assert!(helper_request.get("resume").is_none());
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_forwards_resume_session_controls() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn resume_session() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["requestContext"] = json!({
        "enableFileCheckpointing": true
    });
    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let resume_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let forked_session_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let resume_message_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--resume",
            resume_id,
            "--fork-session",
            "true",
            "--session-id",
            forked_session_id,
            "--resume-session-at",
            resume_message_id,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should forward resume session controls",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["resume"], json!(resume_id));
    assert_eq!(helper_request["forkSession"], json!(true));
    assert_eq!(helper_request["sessionId"], json!(forked_session_id));
    assert_eq!(helper_request["resumeSessionAt"], json!(resume_message_id));
    assert!(helper_request.get("continue").is_none());
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_python_backend_uses_python_helper() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn python_backend() {}\n").expect("write source file");

    let artifact = semantic_full_artifact(repo.path(), &touched_file);
    let artifact_path = repo.path().join("python-managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let request_path = repo.path().join("python-helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.py");
    write_json_response_capture_python_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Read",
            "--python-binary",
            "python3",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should support the opt-in python helper backend",
    );

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["mode"], json!("query"));
    assert_eq!(helper_request["prompt"], json!(DEFAULT_MANAGED_PROMPT));
    assert_eq!(helper_request["tools"], json!(["Read"]));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_rejects_invalid_session_control_combinations() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let cases = [
        (
            vec![
                "claude-sdk",
                "run",
                "--prompt",
                DEFAULT_MANAGED_PROMPT,
                "--continue",
                "true",
                "--resume",
                "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            ],
            "--continue cannot be combined with --resume",
        ),
        (
            vec![
                "claude-sdk",
                "run",
                "--prompt",
                DEFAULT_MANAGED_PROMPT,
                "--resume-session-at",
                "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            ],
            "--resume-session-at requires --resume",
        ),
        (
            vec![
                "claude-sdk",
                "run",
                "--prompt",
                DEFAULT_MANAGED_PROMPT,
                "--fork-session",
                "true",
            ],
            "--fork-session requires --resume",
        ),
        (
            vec![
                "claude-sdk",
                "run",
                "--prompt",
                DEFAULT_MANAGED_PROMPT,
                "--continue",
                "true",
                "--session-id",
                "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            ],
            "--session-id requires --fork-session",
        ),
    ];

    for (args, expected_message) in cases {
        let output = run_libra_command(&args, repo.path());
        assert!(
            !output.status.success(),
            "expected session-control validation to fail for args: {:?}",
            args
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected_message),
            "stderr should mention '{expected_message}', got: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_interactive_tool_approval_uses_canonical_session_cache_key() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "interactive-cache-session",
            "steps": [
                {
                    "type": "tool",
                    "toolName": "Bash",
                    "toolUseId": "tool-bash-1",
                    "input": {
                        "command": "echo first",
                        "description": "cache me",
                        "env": {
                            "A": "1",
                            "B": "2"
                        }
                    }
                },
                {
                    "type": "tool",
                    "toolName": "Bash",
                    "toolUseId": "tool-bash-2",
                    "input": {
                        "env": {
                            "B": "2",
                            "A": "1"
                        },
                        "description": "cache me",
                        "command": "echo first"
                    }
                }
            ]
        }),
        Some(&json!([
            {
                "kind": "tool_approval",
                "decision": "approve_for_session"
            }
        ])),
    );

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Bash",
            "--interactive-approvals",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "interactive approval should succeed and reuse the session-scoped cache",
    );

    let run_json = parse_stdout_json(&run, "interactive approval cache output");
    let raw_artifact_path = PathBuf::from(
        run_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath should be present"),
    );
    let raw_artifact = read_json_file(&raw_artifact_path);
    let approvals = raw_artifact["hookEvents"]
        .as_array()
        .expect("hookEvents should be an array")
        .iter()
        .filter(|event| event["hook"] == json!("CanUseTool"))
        .collect::<Vec<_>>();
    assert_eq!(approvals.len(), 2);
    assert_eq!(approvals[0]["input"]["approval_scope"], json!("session"));
    assert_eq!(approvals[0]["input"]["prompt_source"], json!("scripted"));
    assert_eq!(approvals[0]["input"]["cached"], json!(false));
    assert_eq!(approvals[1]["input"]["approval_scope"], json!("session"));
    assert_eq!(
        approvals[1]["input"]["prompt_source"],
        json!("session_cache")
    );
    assert_eq!(approvals[1]["input"]["cached"], json!(true));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_interactive_tool_approval_can_deny() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "interactive-deny-session",
            "steps": [
                {
                    "type": "tool",
                    "toolName": "Bash",
                    "toolUseId": "tool-bash-deny",
                    "input": {
                        "command": "echo deny"
                    }
                }
            ]
        }),
        Some(&json!([
            {
                "kind": "tool_approval",
                "decision": "deny"
            }
        ])),
    );

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Bash",
            "--interactive-approvals",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "deny should still persist the managed artifact");

    let run_json = parse_stdout_json(&run, "interactive approval deny output");
    let raw_artifact_path = PathBuf::from(
        run_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath should be present"),
    );
    let raw_artifact = read_json_file(&raw_artifact_path);
    let approval = raw_artifact["hookEvents"]
        .as_array()
        .expect("hookEvents should be an array")
        .iter()
        .find(|event| event["hook"] == json!("CanUseTool"))
        .expect("CanUseTool event should exist");
    assert_eq!(approval["input"]["approval_decision"], json!("deny"));
    assert_eq!(
        approval["input"]["interaction_kind"],
        json!("tool_approval")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_interactive_tool_approval_can_abort() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "interactive-abort-session",
            "steps": [
                {
                    "type": "tool",
                    "toolName": "Bash",
                    "toolUseId": "tool-bash-abort",
                    "input": {
                        "command": "echo abort"
                    }
                }
            ]
        }),
        Some(&json!([
            {
                "kind": "tool_approval",
                "decision": "abort"
            }
        ])),
    );

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Bash",
            "--interactive-approvals",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert!(
        !run.status.success(),
        "abort should fail the batch command while still persisting the managed artifact"
    );
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("aborted"),
        "stderr should preserve the abort detail: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).trim().is_empty(),
        "aborted batch runs should not print a top-level success payload"
    );

    let raw_artifact_path = only_json_file(
        &repo.path().join(".libra").join("managed-artifacts"),
        "interactive approval abort raw artifact",
    );
    let raw_artifact = read_json_file(&raw_artifact_path);
    let approval = raw_artifact["hookEvents"]
        .as_array()
        .expect("hookEvents should be an array")
        .iter()
        .find(|event| event["hook"] == json!("CanUseTool"))
        .expect("CanUseTool event should exist");
    assert_eq!(approval["input"]["approval_decision"], json!("abort"));
    assert_eq!(
        raw_artifact["resultMessage"]["stop_reason"],
        json!("interrupted")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_interactive_ask_user_question_collects_answers() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "interactive-ask-user-session",
            "steps": [
                {
                    "type": "tool",
                    "toolName": "AskUserQuestion",
                    "toolUseId": "tool-ask-user-1",
                    "input": {
                        "questions": [
                            {
                                "header": "Stack",
                                "question": "Which stack should we use?",
                                "options": [
                                    {
                                        "label": "Rust",
                                        "description": "Use Rust"
                                    },
                                    {
                                        "label": "TypeScript",
                                        "description": "Use TypeScript"
                                    }
                                ]
                            }
                        ]
                    }
                }
            ]
        }),
        Some(&json!([
            {
                "kind": "ask_user_question",
                "answers": {
                    "Which stack should we use?": "Rust"
                }
            }
        ])),
    );

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "AskUserQuestion",
            "--interactive-approvals",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "AskUserQuestion should succeed in interactive mode");

    let run_json = parse_stdout_json(&run, "interactive ask user question output");
    let raw_artifact_path = PathBuf::from(
        run_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath should be present"),
    );
    let raw_artifact = read_json_file(&raw_artifact_path);
    let approval = raw_artifact["hookEvents"]
        .as_array()
        .expect("hookEvents should be an array")
        .iter()
        .find(|event| event["hook"] == json!("CanUseTool"))
        .expect("CanUseTool event should exist");
    assert_eq!(
        approval["input"]["interaction_kind"],
        json!("ask_user_question")
    );
    assert_eq!(approval["input"]["tool_name"], json!("AskUserQuestion"));
    assert_eq!(approval["input"]["answer_count"], json!(1));

    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");
    let decision_input = run_libra_command(
        &[
            "claude-sdk",
            "build-decision-input",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &decision_input,
        "build-decision-input should absorb AskUserQuestion facts",
    );
    let decision_input_json = parse_stdout_json(&decision_input, "decision input output");
    let decision_input_artifact = read_json_file(Path::new(
        decision_input_json["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    ));
    let signal = decision_input_artifact["signals"]
        .as_array()
        .expect("signals should be an array")
        .iter()
        .find(|signal| signal["interactionKind"] == json!("ask_user_question"))
        .expect("ask user question signal should be present");
    assert_eq!(signal["toolName"], json!("AskUserQuestion"));
    assert_eq!(signal["answerCount"], json!(1));
    assert_eq!(signal["promptSource"], json!("scripted"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_interactive_approvals_require_tty_or_scripted_responses() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let helper_path = write_fake_interactive_helper(
        repo.path(),
        &json!({
            "sessionId": "interactive-no-tty-session",
            "steps": [
                {
                    "type": "tool",
                    "toolName": "Bash",
                    "toolUseId": "tool-bash-no-tty",
                    "input": {
                        "command": "echo no tty"
                    }
                }
            ]
        }),
        None,
    );

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--tool",
            "Bash",
            "--interactive-approvals",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert!(
        !run.status.success(),
        "interactive approvals should fail without a tty or scripted responses"
    );
    assert!(
        String::from_utf8_lossy(&run.stderr)
            .contains("interactive approvals require an interactive terminal"),
        "stderr should explain the missing tty requirement: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_build_managed_evidence_input_persists_history_object() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_evidence_input() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["requestContext"] = json!({
        "enableFileCheckpointing": true
    });
    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--enable-file-checkpointing",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed before managed evidence input",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run managed evidence input");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();
    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    let build = run_libra_command(
        &[
            "claude-sdk",
            "build-managed-evidence-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&build, "build-managed-evidence-input should succeed");
    let build_json = parse_stdout_json(&build, "build-managed-evidence-input output");
    let object_id = build_json["objectId"]
        .as_str()
        .expect("objectId should be present")
        .to_string();
    let artifact_path = PathBuf::from(
        build_json["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    assert!(
        artifact_path.exists(),
        "managed evidence input artifact should exist"
    );

    let artifact = read_json_file(&artifact_path);
    assert_eq!(
        artifact["schema"],
        json!("libra.claude_managed_evidence_input.v1")
    );
    assert_eq!(
        artifact["object_type"],
        json!("claude_managed_evidence_input")
    );
    assert_eq!(artifact["aiSessionId"], json!(ai_session_id.clone()));
    assert_eq!(artifact["providerSessionId"], json!(provider_session_id));
    assert_eq!(
        artifact["patchOverview"]["touchedFiles"],
        json!(["src/lib.rs"])
    );
    assert_eq!(
        artifact["patchOverview"]["checkpointingEnabled"],
        json!(true)
    );
    assert_eq!(artifact["patchOverview"]["rewindSupported"], json!(true));
    assert_eq!(
        artifact["patchOverview"]["filesPersisted"][0]["filename"],
        json!("src/lib.rs")
    );
    assert!(
        !artifact["sourceArtifacts"]["providerEvidenceInputPath"].is_null(),
        "managed evidence input should link the provider evidence_input artifact when present"
    );
    assert_eq!(
        artifact["runtimeOverview"]["decisionRuntimeCount"],
        json!(4)
    );

    let (storage, history) = load_intent_history(repo.path()).await;
    let objects = history
        .list_objects("claude_managed_evidence_input")
        .await
        .expect("should list managed evidence input objects");
    assert_eq!(objects.len(), 1);
    let ai_type = run_libra_command(&["cat-file", "--ai-type", &object_id], repo.path());
    assert_cli_success(
        &ai_type,
        "cat-file --ai-type should succeed for managed evidence input",
    );
    assert_eq!(
        String::from_utf8_lossy(&ai_type.stdout).trim(),
        "claude_managed_evidence_input"
    );
    let ai_pretty = run_libra_command(&["cat-file", "--ai", &object_id], repo.path());
    assert_cli_success(
        &ai_pretty,
        "cat-file --ai should succeed for managed evidence input",
    );
    assert!(String::from_utf8_lossy(&ai_pretty.stdout).contains("claude_managed_evidence_input"));
    drop(storage);
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_build_decision_input_persists_history_object() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn decision_input() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--enable-file-checkpointing",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed before decision input");
    let run_json = parse_stdout_json(&run, "claude-sdk run decision input");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let build_evidence_input = run_libra_command(
        &[
            "claude-sdk",
            "build-managed-evidence-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &build_evidence_input,
        "build-managed-evidence-input should succeed before decision input",
    );

    let build = run_libra_command(
        &[
            "claude-sdk",
            "build-decision-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&build, "build-decision-input should succeed");
    let build_json = parse_stdout_json(&build, "build-decision-input output");
    let object_id = build_json["objectId"]
        .as_str()
        .expect("objectId should be present")
        .to_string();
    let artifact_path = PathBuf::from(
        build_json["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    let artifact = read_json_file(&artifact_path);
    assert_eq!(artifact["schema"], json!("libra.claude_decision_input.v1"));
    assert_eq!(artifact["object_type"], json!("claude_decision_input"));
    assert_eq!(
        artifact["decisionOverview"]["permissionRequestCount"],
        json!(1)
    );
    assert_eq!(artifact["decisionOverview"]["elicitationCount"], json!(1));
    assert_eq!(
        artifact["decisionOverview"]["elicitationResultCount"],
        json!(1)
    );
    assert_eq!(
        artifact["decisionOverview"]["permissionDenialCount"],
        json!(1)
    );
    assert!(!artifact["sourceArtifacts"]["managedEvidenceInputPath"].is_null());

    let ai_type = run_libra_command(&["cat-file", "--ai-type", &object_id], repo.path());
    assert_cli_success(
        &ai_type,
        "cat-file --ai-type should succeed for decision input",
    );
    assert_eq!(
        String::from_utf8_lossy(&ai_type.stdout).trim(),
        "claude_decision_input"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_persists_provider_session_snapshots() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let response_path = repo.path().join("session-catalog.json");
    fs::write(
        &response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "fileSize": 2048,
                "customTitle": "A title",
                "firstPrompt": "Add tests",
                "gitBranch": "main",
                "cwd": repo.path().to_string_lossy().to_string(),
                "tag": "review",
                "createdAt": 1742022000000i64
            },
            {
                "sessionId": "session-b",
                "summary": "Claude session B",
                "lastModified": 1742029200000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");

    let request_path = repo.path().join("session-catalog-request.json");
    let helper_path = repo.path().join("fake-session-helper.sh");
    write_json_response_capture_shell_helper(&helper_path, &response_path, &request_path);

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--limit",
            "10",
            "--offset",
            "2",
            "--include-worktrees",
            "false",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&sync, "claude-sdk sync-sessions should succeed");
    let sync_json = parse_stdout_json(&sync, "claude-sdk sync-sessions");
    assert_eq!(sync_json["ok"], json!(true));
    assert_eq!(sync_json["mode"], json!("sync-sessions"));
    assert_eq!(sync_json["syncedCount"], json!(2));

    let helper_request = read_json_file(&request_path);
    assert_eq!(helper_request["mode"], json!("listSessions"));
    assert_eq!(helper_request["limit"], json!(10));
    assert_eq!(helper_request["offset"], json!(2));
    assert_eq!(helper_request["includeWorktrees"], json!(false));

    let first_artifact = PathBuf::from(
        sync_json["sessions"][0]["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    assert!(
        first_artifact.exists(),
        "provider session artifact should exist"
    );
    let first_snapshot = read_json_file(&first_artifact);
    assert_eq!(first_snapshot["schema"], json!("libra.provider_session.v3"));
    assert_eq!(first_snapshot["provider"], json!("claude"));
    assert_eq!(first_snapshot["providerSessionId"], json!("session-a"));
    assert_eq!(
        first_snapshot["objectId"],
        json!("claude_provider_session__session-a")
    );
    assert_eq!(first_snapshot["summary"], json!("Claude session A"));

    let (_, history) = load_intent_history(repo.path()).await;
    let sessions = history
        .list_objects("provider_session")
        .await
        .expect("should list provider_session objects");
    assert_eq!(
        sessions.len(),
        2,
        "should persist provider session snapshots"
    );

    let ai_pretty = run_libra_command(
        &["cat-file", "--ai", "claude_provider_session__session-a"],
        repo.path(),
    );
    assert_cli_success(
        &ai_pretty,
        "cat-file --ai should succeed for provider_session object",
    );
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("type: provider_session"));
    assert!(ai_pretty_stdout.contains("schema: libra.provider_session.v3"));
    assert!(ai_pretty_stdout.contains("provider: claude"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_preserves_existing_message_sync() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let catalog_response_path = repo.path().join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.path().join("session-catalog-request.json");
    let catalog_helper_path = repo.path().join("fake-session-catalog-helper.sh");
    write_json_response_capture_shell_helper(
        &catalog_helper_path,
        &catalog_response_path,
        &catalog_request_path,
    );

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&sync, "initial claude-sdk sync-sessions should succeed");

    let messages_response_path = repo.path().join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&json!([
            {"type": "user", "session_id": "session-a"},
            {"type": "assistant", "session_id": "session-a"},
            {"type": "result", "subtype": "success", "session_id": "session-a"}
        ]))
        .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.path().join("session-messages-request.json");
    let messages_helper_path = repo.path().join("fake-session-messages-helper.sh");
    write_json_response_capture_shell_helper(
        &messages_helper_path,
        &messages_response_path,
        &messages_request_path,
    );

    let hydrate = run_libra_command(
        &[
            "claude-sdk",
            "hydrate-session",
            "--provider-session-id",
            "session-a",
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&hydrate, "claude-sdk hydrate-session should succeed");

    let resync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&resync, "repeat claude-sdk sync-sessions should succeed");

    let build = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            "session-a",
        ],
        repo.path(),
    );
    assert_cli_success(
        &build,
        "build-evidence-input should still succeed after re-syncing a hydrated session",
    );

    let snapshot_path = repo
        .path()
        .join(".libra/provider-sessions/claude_provider_session__session-a.json");
    let snapshot = read_json_file(&snapshot_path);
    assert_eq!(snapshot["messageSync"]["messageCount"], json!(3));
    assert_eq!(
        snapshot["messageSync"]["lastMessageKind"],
        json!("result:success")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_skips_history_append_when_snapshot_is_unchanged() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let response_path = repo.path().join("session-catalog.json");
    fs::write(
        &response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");

    let request_path = repo.path().join("session-catalog-request.json");
    let helper_path = repo.path().join("fake-session-helper.sh");
    write_json_response_capture_shell_helper(&helper_path, &response_path, &request_path);

    let first = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&first, "initial sync-sessions should succeed");

    let (_, history) = load_intent_history(repo.path()).await;
    let first_head = read_history_head(repo.path(), &history).await;

    let second = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&second, "repeat sync-sessions should succeed");

    let second_head = read_history_head(repo.path(), &history).await;
    assert_eq!(
        second_head, first_head,
        "unchanged sync-sessions runs should not append a new AI history commit"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_keeps_history_in_current_repo_when_cwd_is_overridden() {
    let repo = tempdir().expect("failed to create repo root");
    let external_project = tempdir().expect("failed to create external project root");
    test::setup_with_new_libra_in(repo.path()).await;

    let response_path = repo.path().join("session-catalog.json");
    fs::write(
        &response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": external_project.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");

    let request_path = repo.path().join("session-catalog-request.json");
    let helper_path = repo.path().join("fake-session-helper.sh");
    write_json_response_capture_shell_helper(&helper_path, &response_path, &request_path);

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--cwd",
            external_project
                .path()
                .to_str()
                .expect("external cwd utf-8"),
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should persist into the current repo even with --cwd override",
    );

    let (_, history) = load_intent_history(repo.path()).await;
    let sessions = history
        .list_objects("provider_session")
        .await
        .expect("should list provider_session objects from current repo");
    assert_eq!(sessions.len(), 1);

    assert!(
        !external_project.path().join(".libra/libra.db").exists(),
        "sync-sessions should not create a shadow Libra repo under the overridden cwd"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_hydrate_session_updates_provider_session_with_messages() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let catalog_response_path = repo.path().join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.path().join("session-catalog-request.json");
    let catalog_helper_path = repo.path().join("fake-session-catalog-helper.sh");
    write_json_response_capture_shell_helper(
        &catalog_helper_path,
        &catalog_response_path,
        &catalog_request_path,
    );

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should succeed before hydration",
    );

    let messages_response_path = repo.path().join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "type": "system",
                "subtype": "init",
                "session_id": "session-a",
                "uuid": "msg-1"
            },
            {
                "type": "user",
                "session_id": "session-a"
            },
            {
                "type": "assistant",
                "session_id": "session-a",
                "uuid": "msg-2"
            },
            {
                "type": "tool_progress",
                "tool_use_id": "tool-1",
                "tool_name": "Read",
                "elapsed_time_seconds": 1,
                "session_id": "session-a",
                "uuid": "msg-3"
            },
            {
                "type": "result",
                "subtype": "success",
                "session_id": "session-a",
                "uuid": "msg-4",
                "duration_ms": 10,
                "duration_api_ms": 8,
                "is_error": false,
                "num_turns": 1,
                "result": "ok",
                "stop_reason": "end_turn",
                "total_cost_usd": 0.01,
                "usage": {}
            }
        ]))
        .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.path().join("session-messages-request.json");
    let messages_helper_path = repo.path().join("fake-session-messages-helper.sh");
    write_json_response_capture_shell_helper(
        &messages_helper_path,
        &messages_response_path,
        &messages_request_path,
    );

    let hydrate = run_libra_command(
        &[
            "claude-sdk",
            "hydrate-session",
            "--provider-session-id",
            "session-a",
            "--limit",
            "20",
            "--offset",
            "3",
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&hydrate, "claude-sdk hydrate-session should succeed");
    let hydrate_json = parse_stdout_json(&hydrate, "claude-sdk hydrate-session");
    assert_eq!(hydrate_json["ok"], json!(true));
    assert_eq!(hydrate_json["mode"], json!("hydrate-session"));
    assert_eq!(hydrate_json["providerSessionId"], json!("session-a"));
    assert_eq!(hydrate_json["messageCount"], json!(5));

    let helper_request = read_json_file(&messages_request_path);
    assert_eq!(helper_request["mode"], json!("getSessionMessages"));
    assert_eq!(helper_request["providerSessionId"], json!("session-a"));
    assert_eq!(helper_request["limit"], json!(20));
    assert_eq!(helper_request["offset"], json!(3));

    let artifact_path = PathBuf::from(
        hydrate_json["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    let snapshot = read_json_file(&artifact_path);
    assert_eq!(snapshot["schema"], json!("libra.provider_session.v3"));
    assert_eq!(snapshot["messageSync"]["messageCount"], json!(5));
    assert_eq!(
        snapshot["messageSync"]["kindCounts"]["system:init"],
        json!(1)
    );
    assert_eq!(
        snapshot["messageSync"]["kindCounts"]["result:success"],
        json!(1)
    );
    assert_eq!(
        snapshot["messageSync"]["firstMessageKind"],
        json!("system:init")
    );
    assert_eq!(
        snapshot["messageSync"]["lastMessageKind"],
        json!("result:success")
    );

    let messages_artifact_path = PathBuf::from(
        hydrate_json["messagesArtifactPath"]
            .as_str()
            .expect("messagesArtifactPath should be present"),
    );
    let messages_artifact = read_json_file(&messages_artifact_path);
    assert_eq!(
        messages_artifact["schema"],
        json!("libra.provider_session_messages.v1")
    );
    assert_eq!(messages_artifact["providerSessionId"], json!("session-a"));
    assert_eq!(
        messages_artifact["messages"].as_array().map(Vec::len),
        Some(5)
    );

    let ai_pretty = run_libra_command(
        &["cat-file", "--ai", "claude_provider_session__session-a"],
        repo.path(),
    );
    assert_cli_success(
        &ai_pretty,
        "cat-file --ai should succeed for hydrated provider_session object",
    );
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("message_count: 5"));
    assert!(ai_pretty_stdout.contains("first_message_kind: system:init"));
    assert!(ai_pretty_stdout.contains("last_message_kind: result:success"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_build_evidence_input_from_provider_session_messages() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let catalog_response_path = repo.path().join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.path().join("session-catalog-request.json");
    let catalog_helper_path = repo.path().join("fake-session-catalog-helper.sh");
    write_json_response_capture_shell_helper(
        &catalog_helper_path,
        &catalog_response_path,
        &catalog_request_path,
    );

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &sync,
        "claude-sdk sync-sessions should succeed before evidence-input build",
    );

    let messages_response_path = repo.path().join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "type": "system",
                "subtype": "init",
                "session_id": "session-a",
                "uuid": "msg-1"
            },
            {
                "type": "user",
                "session_id": "session-a",
                "message": {
                    "content": [
                        {
                            "type": "text",
                            "text": "Inspect src/lib.rs and summarize the bridge state."
                        }
                    ]
                }
            },
            {
                "type": "assistant",
                "session_id": "session-a",
                "uuid": "msg-2",
                "message": {
                    "content": [
                        {
                            "type": "text",
                            "text": "I will inspect src/lib.rs and then summarize the current bridge shape."
                        },
                        {
                            "type": "tool_use",
                            "name": "Read",
                            "input": {
                                "file_path": "src/lib.rs"
                            }
                        }
                    ]
                }
            },
            {
                "type": "system",
                "subtype": "task_progress",
                "session_id": "session-a",
                "uuid": "msg-task-1",
                "description": "Reading provider runtime facts"
            },
            {
                "type": "tool_progress",
                "tool_use_id": "tool-1",
                "tool_name": "Read",
                "elapsed_time_seconds": 1,
                "session_id": "session-a",
                "uuid": "msg-3"
            },
            {
                "type": "result",
                "subtype": "success",
                "session_id": "session-a",
                "uuid": "msg-4",
                "duration_ms": 10,
                "duration_api_ms": 8,
                "is_error": false,
                "num_turns": 1,
                "result": "ok",
                "stop_reason": "end_turn",
                "total_cost_usd": 0.01,
                "permission_denials": [
                    {
                        "tool_name": "Edit"
                    }
                ],
                "structured_output": {
                    "summary": "Separate runtime facts from semantic candidates"
                },
                "usage": {}
            }
        ]))
        .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.path().join("session-messages-request.json");
    let messages_helper_path = repo.path().join("fake-session-messages-helper.sh");
    write_json_response_capture_shell_helper(
        &messages_helper_path,
        &messages_response_path,
        &messages_request_path,
    );

    let hydrate = run_libra_command(
        &[
            "claude-sdk",
            "hydrate-session",
            "--provider-session-id",
            "session-a",
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &hydrate,
        "claude-sdk hydrate-session should succeed before evidence-input build",
    );

    let build = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            "session-a",
        ],
        repo.path(),
    );
    assert_cli_success(
        &build,
        "claude-sdk build-evidence-input should succeed for a hydrated provider session",
    );
    let build_json = parse_stdout_json(&build, "claude-sdk build-evidence-input");
    assert_eq!(build_json["ok"], json!(true));
    assert_eq!(build_json["mode"], json!("build-evidence-input"));
    assert_eq!(build_json["providerSessionId"], json!("session-a"));
    assert_eq!(
        build_json["providerSessionObjectId"],
        json!("claude_provider_session__session-a")
    );
    assert_eq!(
        build_json["objectId"],
        json!("claude_evidence_input__session-a")
    );
    assert_eq!(build_json["messageCount"], json!(6));

    let evidence_path = PathBuf::from(
        build_json["artifactPath"]
            .as_str()
            .expect("artifactPath should be present"),
    );
    let evidence = read_json_file(&evidence_path);
    assert_eq!(evidence["schema"], json!("libra.evidence_input.v1"));
    assert_eq!(evidence["object_type"], json!("evidence_input"));
    assert_eq!(evidence["provider"], json!("claude"));
    assert_eq!(evidence["providerSessionId"], json!("session-a"));
    assert_eq!(
        evidence["providerSessionObjectId"],
        json!("claude_provider_session__session-a")
    );
    assert_eq!(evidence["messageOverview"]["messageCount"], json!(6));
    assert_eq!(
        evidence["contentOverview"]["assistantMessageCount"],
        json!(1)
    );
    assert_eq!(evidence["contentOverview"]["userMessageCount"], json!(1));
    assert_eq!(
        evidence["contentOverview"]["observedTools"]["Read"],
        json!(2)
    );
    assert_eq!(
        evidence["contentOverview"]["observedPaths"][0],
        json!("src/lib.rs")
    );
    assert_eq!(evidence["runtimeSignals"]["toolRuntimeCount"], json!(1));
    assert_eq!(evidence["runtimeSignals"]["taskRuntimeCount"], json!(1));
    assert_eq!(evidence["runtimeSignals"]["resultMessageCount"], json!(1));
    assert_eq!(
        evidence["runtimeSignals"]["hasStructuredOutput"],
        json!(true)
    );
    assert_eq!(
        evidence["runtimeSignals"]["hasPermissionDenials"],
        json!(true)
    );
    assert_eq!(evidence["latestResult"]["stopReason"], json!("end_turn"));
    assert_eq!(evidence["latestResult"]["permissionDenialCount"], json!(1));

    let (_, history) = load_intent_history(repo.path()).await;
    let evidence_inputs = history
        .list_objects("evidence_input")
        .await
        .expect("should list evidence_input objects");
    assert_eq!(
        evidence_inputs.len(),
        1,
        "should persist evidence_input objects"
    );

    let ai_pretty = run_libra_command(
        &["cat-file", "--ai", "claude_evidence_input__session-a"],
        repo.path(),
    );
    assert_cli_success(
        &ai_pretty,
        "cat-file --ai should succeed for evidence_input object",
    );
    let ai_pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
    assert!(ai_pretty_stdout.contains("type: evidence_input"));
    assert!(ai_pretty_stdout.contains("schema: libra.evidence_input.v1"));
    assert!(ai_pretty_stdout.contains("message_count: 6"));
    assert!(ai_pretty_stdout.contains("has_structured_output: true"));

    let helper_request = read_json_file(&messages_request_path);
    assert_eq!(helper_request["mode"], json!("getSessionMessages"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_build_evidence_input_skips_history_append_when_artifact_is_unchanged() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let catalog_response_path = repo.path().join("session-catalog.json");
    fs::write(
        &catalog_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");
    let catalog_request_path = repo.path().join("session-catalog-request.json");
    let catalog_helper_path = repo.path().join("fake-session-catalog-helper.sh");
    write_json_response_capture_shell_helper(
        &catalog_helper_path,
        &catalog_response_path,
        &catalog_request_path,
    );

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            catalog_helper_path
                .to_str()
                .expect("catalog helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &sync,
        "sync-sessions should succeed before evidence-input build",
    );

    let messages_response_path = repo.path().join("session-messages.json");
    fs::write(
        &messages_response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "type": "system",
                "subtype": "init",
                "session_id": "session-a",
                "uuid": "msg-1"
            },
            {
                "type": "assistant",
                "session_id": "session-a",
                "uuid": "msg-2",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "Read",
                            "input": {
                                "file_path": "src/lib.rs"
                            }
                        }
                    ]
                }
            },
            {
                "type": "result",
                "subtype": "success",
                "session_id": "session-a",
                "uuid": "msg-3",
                "stop_reason": "end_turn",
                "structured_output": {
                    "summary": "Bridge runtime facts"
                }
            }
        ]))
        .expect("serialize session messages"),
    )
    .expect("write session messages response");
    let messages_request_path = repo.path().join("session-messages-request.json");
    let messages_helper_path = repo.path().join("fake-session-messages-helper.sh");
    write_json_response_capture_shell_helper(
        &messages_helper_path,
        &messages_response_path,
        &messages_request_path,
    );

    let hydrate = run_libra_command(
        &[
            "claude-sdk",
            "hydrate-session",
            "--provider-session-id",
            "session-a",
            "--helper-path",
            messages_helper_path
                .to_str()
                .expect("messages helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &hydrate,
        "hydrate-session should succeed before evidence-input build",
    );

    let first = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            "session-a",
        ],
        repo.path(),
    );
    assert_cli_success(&first, "initial build-evidence-input should succeed");

    let (_, history) = load_intent_history(repo.path()).await;
    let first_head = read_history_head(repo.path(), &history).await;

    let second = run_libra_command(
        &[
            "claude-sdk",
            "build-evidence-input",
            "--provider-session-id",
            "session-a",
        ],
        repo.path(),
    );
    assert_cli_success(&second, "repeat build-evidence-input should succeed");

    let second_head = read_history_head(repo.path(), &history).await;
    assert_eq!(
        second_head, first_head,
        "unchanged evidence-input builds should not append a new AI history commit"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_sync_sessions_rejects_invalid_provider_session_id() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let response_path = repo.path().join("session-catalog.json");
    fs::write(
        &response_path,
        serde_json::to_vec_pretty(&json!([
            {
                "sessionId": "../session-a",
                "summary": "Claude session A",
                "lastModified": 1742025600000i64,
                "cwd": repo.path().to_string_lossy().to_string()
            }
        ]))
        .expect("serialize session catalog"),
    )
    .expect("write session catalog response");

    let request_path = repo.path().join("session-catalog-request.json");
    let helper_path = repo.path().join("fake-session-helper.sh");
    write_json_response_capture_shell_helper(&helper_path, &response_path, &request_path);

    let sync = run_libra_command(
        &[
            "claude-sdk",
            "sync-sessions",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert!(
        !sync.status.success(),
        "sync-sessions should reject invalid provider session ids"
    );
    assert!(
        String::from_utf8_lossy(&sync.stderr).contains("invalid provider session id"),
        "expected invalid provider session id error, got: {}",
        String::from_utf8_lossy(&sync.stderr)
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_batch_error_result_is_not_reported_as_success() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn auth_failure() {}\n").expect("write source file");

    let artifact_path = repo.path().join("errored-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&errored_result_artifact(
            repo.path(),
            &touched_file,
            "authentication_failed: 401 invalid token",
        ))
        .expect("serialize error artifact"),
    )
    .expect("write error artifact");

    let helper_path = repo.path().join("fake-error-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert!(
        !run.status.success(),
        "batch run should fail when the helper artifact reports an error result"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("Claude Code returned an error result"),
        "stderr should report the helper result failure: {stderr}"
    );
    assert!(
        stderr.contains("authentication_failed: 401 invalid token"),
        "stderr should preserve the helper error detail: {stderr}"
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).trim().is_empty(),
        "failed batch runs should not print a top-level success payload"
    );

    let raw_artifact_path = only_json_file(
        &repo.path().join(".libra").join("managed-artifacts"),
        "errored batch run raw artifact",
    );
    let raw_artifact = read_json_file(&raw_artifact_path);
    assert_eq!(raw_artifact["resultMessage"]["subtype"], json!("error"));
    assert_eq!(raw_artifact["resultMessage"]["is_error"], json!(true));

    let audit_bundle_path = only_json_file(
        &repo.path().join(".libra").join("audit-bundles"),
        "errored batch run audit bundle",
    );
    let audit_bundle = read_json_file(&audit_bundle_path);
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runEvent"]["error"],
        json!("authentication_failed: 401 invalid token")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_batch_timeout_is_not_reported_as_success() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("timed-out-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&timed_out_partial_artifact(repo.path(), &touched_file))
            .expect("serialize timeout artifact"),
    )
    .expect("write timeout artifact");

    let helper_path = repo.path().join("fake-timeout-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--batch",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert!(
        !run.status.success(),
        "batch run should fail when the helper times out even if artifacts were persisted"
    );
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("Claude SDK helper timed out"),
        "timeout failure should mention the helper timeout"
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).trim().is_empty(),
        "timed out batch runs should not print a top-level success payload"
    );

    let raw_artifact_path = only_json_file(
        &repo.path().join(".libra").join("managed-artifacts"),
        "timed out batch run raw artifact",
    );
    let raw_artifact = read_json_file(&raw_artifact_path);
    assert_eq!(raw_artifact["helperTimedOut"], json!(true));

    let audit_bundle_path = only_json_file(
        &repo.path().join(".libra").join("audit-bundles"),
        "timed out batch run audit bundle",
    );
    let audit_bundle = read_json_file(&audit_bundle_path);
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["runEvent"]["status"],
        json!("timed_out")
    );
    assert_eq!(
        audit_bundle["bridge"]["sessionState"]["metadata"]["managed_helper_timed_out"],
        json!(true)
    );
    assert_eq!(
        audit_bundle["bridge"]["sessionState"]["metadata"]["managed_helper_error"],
        json!("Claude SDK helper timed out")
    );
    assert!(
        audit_bundle["bridge"]["objectCandidates"]["decisionRuntimeEvents"]
            .as_array()
            .is_some_and(|events| !events.is_empty()),
        "partial timeout artifact should still preserve decision runtime facts"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_run_plan_prompt_fixture_preserves_real_plan_text_shape() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("samples").join("managed.rs");
    fs::create_dir_all(touched_file.parent().expect("sample file parent")).expect("mkdir samples");
    fs::write(&touched_file, "pub fn provider_runtime() {}\n").expect("write sample file");

    let artifact_path = repo.path().join("managed-plan-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&plan_task_only_artifact(repo.path(), &touched_file))
            .expect("serialize plan scenario artifact"),
    )
    .expect("write plan scenario artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let prompt_path = repo.path().join("plan-prompt.txt");
    fs::write(&prompt_path, PLAN_PROMPT).expect("write plan prompt fixture");

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt-file",
            prompt_path.to_str().expect("prompt path utf-8"),
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed for plan prompt fixture",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run with plan prompt fixture");

    let audit_bundle_path = PathBuf::from(
        run_json["auditBundlePath"]
            .as_str()
            .expect("auditBundlePath should be present"),
    );
    let intent_extraction_path = PathBuf::from(
        run_json["intentExtractionPath"]
            .as_str()
            .expect("intentExtractionPath should be present"),
    );

    let audit_bundle = read_json_file(&audit_bundle_path);
    let task_runtime_events = audit_bundle["bridge"]["objectCandidates"]["taskRuntimeEvents"]
        .as_array()
        .expect("taskRuntimeEvents should be an array");

    assert_eq!(
        audit_bundle["rawArtifact"]["prompt"],
        json!(PLAN_PROMPT),
        "raw artifact should preserve the persisted prompt fixture"
    );
    assert_eq!(
        audit_bundle["bridge"]["intentExtraction"]["status"],
        json!("accepted")
    );
    assert!(
        task_runtime_events.is_empty(),
        "real-artifact-shaped plan fixture should not invent task runtime events"
    );
    assert_eq!(
        audit_bundle["bridge"]["objectCandidates"]["providerInitSnapshot"]["agents"],
        json!(["general-purpose", "statusline-setup", "Explore", "Plan"])
    );
    assert!(
        audit_bundle["rawArtifact"]["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| {
                message["message"]["content"]
                    .as_array()
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item["text"]
                                .as_str()
                                .is_some_and(|text| text.contains("3-Step Audit Plan"))
                        })
                    })
            })),
        "plan scenario should preserve the assistant plan text in raw messages"
    );
    assert_eq!(
        audit_bundle["rawArtifact"]["resultMessage"]["structured_output"]["changeType"],
        json!("refactor")
    );

    let intent_extraction = read_json_file(&intent_extraction_path);
    assert_eq!(
        intent_extraction["extraction"]["intent"]["summary"],
        json!("Audit the Claude SDK to Libra bridge using real managed artifacts")
    );
    assert_eq!(
        intent_extraction["planningSummary"],
        json!(
            "Use the opening numbered plan as runtime guidance only; formal Plan objects still come from raw assistant plan text."
        )
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_resolve_extraction_materializes_intentspec_preview() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed before resolve-extraction",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run before resolve-extraction");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&resolve, "claude-sdk resolve-extraction should succeed");
    let resolve_json = parse_stdout_json(&resolve, "claude-sdk resolve-extraction");
    let expected_extraction_path = run_json["intentExtractionPath"]
        .as_str()
        .expect("run should emit intentExtractionPath");

    assert_eq!(resolve_json["ok"], json!(true));
    assert_eq!(resolve_json["mode"], json!("resolve-extraction"));
    assert_eq!(resolve_json["aiSessionId"], json!(ai_session_id));
    assert_eq!(
        resolve_json["extractionPath"],
        json!(expected_extraction_path)
    );
    assert_eq!(resolve_json["riskLevel"], json!("medium"));
    assert!(
        resolve_json["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("IntentSpec generated.")),
        "resolve-extraction summary should be derived from the IntentSpec preview"
    );

    let resolved_spec_path = PathBuf::from(
        resolve_json["resolvedSpecPath"]
            .as_str()
            .expect("resolvedSpecPath should be present"),
    );
    assert!(
        resolved_spec_path.exists(),
        "resolved IntentSpec artifact should be materialized"
    );

    let resolved_artifact = read_json_file(&resolved_spec_path);
    assert_eq!(
        resolved_artifact["schema"],
        json!("libra.intent_resolution.v1")
    );
    assert_eq!(resolved_artifact["aiSessionId"], json!(ai_session_id));
    assert_eq!(resolved_artifact["riskLevel"], json!("medium"));
    assert_eq!(
        resolved_artifact["extractionSource"],
        json!("claude_agent_sdk_managed.structured_output")
    );
    assert_eq!(resolved_artifact["intentspec"]["kind"], json!("IntentSpec"));
    assert_eq!(
        resolved_artifact["intentspec"]["apiVersion"],
        json!("intentspec.io/v1alpha1")
    );
    assert_eq!(
        resolved_artifact["intentspec"]["intent"]["summary"],
        json!("Persist the Claude SDK managed bridge")
    );
    assert_eq!(
        resolved_artifact["intentspec"]["risk"]["level"],
        json!("medium")
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_resolve_extraction_accepts_v1_artifact_schema() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn legacy_extraction_schema() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed before v1 compatibility check",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run before v1 compatibility check");
    let extraction_path = PathBuf::from(
        run_json["intentExtractionPath"]
            .as_str()
            .expect("intentExtractionPath should be present"),
    );

    let mut extraction = read_json_file(&extraction_path);
    extraction["schema"] = json!("libra.intent_extraction.v1");
    extraction
        .as_object_mut()
        .expect("extraction artifact should be an object")
        .remove("planningSummary");
    fs::write(
        &extraction_path,
        serde_json::to_vec_pretty(&extraction).expect("serialize compatibility artifact"),
    )
    .expect("write compatibility artifact");

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--extraction",
            extraction_path.to_str().expect("extraction path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &resolve,
        "resolve-extraction should accept legacy v1 extraction schema",
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_intent_writes_formal_intent_and_binding_artifact() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed before persist-intent");
    let run_json = parse_stdout_json(&run, "claude-sdk run before persist-intent");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &resolve,
        "claude-sdk resolve-extraction should succeed before persist-intent",
    );

    let persist = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&persist, "claude-sdk persist-intent should succeed");
    let persist_json = parse_stdout_json(&persist, "claude-sdk persist-intent");
    let expected_extraction_path = run_json["intentExtractionPath"]
        .as_str()
        .expect("run should emit intentExtractionPath");

    assert_eq!(persist_json["ok"], json!(true));
    assert_eq!(persist_json["mode"], json!("persist-intent"));
    assert_eq!(persist_json["aiSessionId"], json!(ai_session_id));

    let intent_id = persist_json["intentId"]
        .as_str()
        .expect("intentId should be present")
        .to_string();
    let binding_path = PathBuf::from(
        persist_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    assert!(
        binding_path.exists(),
        "persist-intent should materialize a binding artifact"
    );

    let binding_artifact = read_json_file(&binding_path);
    assert_eq!(
        binding_artifact["schema"],
        json!("libra.intent_input_binding.v1")
    );
    assert_eq!(
        binding_artifact["extractionPath"],
        json!(expected_extraction_path)
    );
    assert_eq!(binding_artifact["aiSessionId"], json!(ai_session_id));
    assert_eq!(binding_artifact["intentId"], json!(intent_id));
    assert!(
        binding_artifact["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("IntentSpec generated.")),
        "binding artifact should retain the resolved IntentSpec summary"
    );

    let (storage, history) = load_intent_history(repo.path()).await;
    let intents = history
        .list_objects("intent")
        .await
        .expect("should list intent objects");
    assert_eq!(intents.len(), 1, "should persist exactly one formal intent");
    assert_eq!(
        intents[0].0, intent_id,
        "history should contain the persisted intent ID"
    );

    let stored_intent: Intent = storage
        .get_json(&intents[0].1)
        .await
        .expect("should load persisted intent object");
    assert_eq!(
        stored_intent.prompt(),
        "Persist the Claude SDK managed bridge"
    );
    assert!(
        stored_intent.spec().is_some(),
        "persisted formal intent should retain the canonical IntentSpec"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_intent_accepts_test_change_type() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_bridge() {}\n").expect("write source file");

    let artifact_path = repo
        .path()
        .join("managed-run-artifact-test-change-type.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&test_change_type_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");

    let helper_path = repo.path().join("fake-managed-helper-test-change-type.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed for changeType=test");
    let run_json = parse_stdout_json(&run, "claude-sdk run changeType=test");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &resolve,
        "claude-sdk resolve-extraction should accept changeType=test",
    );
    let resolve_json = parse_stdout_json(&resolve, "claude-sdk resolve-extraction changeType=test");
    let resolved_spec_path = PathBuf::from(
        resolve_json["resolvedSpecPath"]
            .as_str()
            .expect("resolvedSpecPath should be present"),
    );
    let resolved_artifact = read_json_file(&resolved_spec_path);
    assert_eq!(
        resolved_artifact["intentspec"]["intent"]["changeType"],
        json!("test")
    );

    let persist = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &persist,
        "claude-sdk persist-intent should accept changeType=test",
    );
    let persist_json = parse_stdout_json(&persist, "claude-sdk persist-intent changeType=test");
    let intent_id = persist_json["intentId"]
        .as_str()
        .expect("intentId should be present")
        .to_string();

    let (storage, history) = load_intent_history(repo.path()).await;
    let intents = history
        .list_objects("intent")
        .await
        .expect("should list intent objects");
    assert_eq!(intents.len(), 1, "should persist exactly one formal intent");
    assert_eq!(intents[0].0, intent_id);

    let stored_intent: Intent = storage
        .get_json(&intents[0].1)
        .await
        .expect("should load persisted intent object");
    let stored_spec = stored_intent
        .spec()
        .expect("persisted formal intent should retain the canonical IntentSpec");
    assert_eq!(stored_spec.0["intent"]["changeType"], json!("test"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_persists_task_run_evidence_and_decision() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn formal_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed for formal bridge test");
    let run_json = parse_stdout_json(&run, "claude-sdk run formal bridge");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();

    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&resolve, "resolve-extraction should succeed");

    let persist_intent = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&persist_intent, "persist-intent should succeed");
    let persist_intent_json = parse_stdout_json(&persist_intent, "persist-intent output");
    let intent_id = persist_intent_json["intentId"]
        .as_str()
        .expect("intentId should be present")
        .to_string();
    let intent_binding_path = PathBuf::from(
        persist_intent_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    assert!(
        intent_binding_path.exists(),
        "persist-intent should materialize its binding artifact"
    );

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    let task_id = bridge_json["taskId"]
        .as_str()
        .expect("taskId should be present")
        .to_string();
    let run_id = bridge_json["runId"]
        .as_str()
        .expect("runId should be present")
        .to_string();
    assert_eq!(bridge_json["intentId"], json!(intent_id.clone()));
    assert!(
        bridge_json.get("planId").is_none(),
        "bridge-run should not synthesize a formal plan when raw assistant messages contain no numbered plan"
    );
    let bridge_binding_path = PathBuf::from(
        bridge_json["bindingPath"]
            .as_str()
            .expect("bridge-run bindingPath should be present"),
    );
    assert!(
        bridge_binding_path.exists(),
        "bridge-run should materialize a formal run binding artifact"
    );
    let bridge_binding = read_json_file(&bridge_binding_path);
    assert_eq!(
        bridge_binding["schema"],
        json!("libra.claude_formal_run_binding.v1")
    );
    assert_eq!(bridge_binding["aiSessionId"], json!(ai_session_id.clone()));
    assert_eq!(
        bridge_binding["providerSessionId"],
        json!(provider_session_id.clone())
    );
    assert_eq!(bridge_binding["taskId"], json!(task_id.clone()));
    assert_eq!(bridge_binding["runId"], json!(run_id.clone()));
    assert_eq!(bridge_binding["intentId"], json!(intent_id.clone()));
    assert!(
        bridge_binding.get("planId").is_none(),
        "formal run binding should omit planId when no numbered assistant plan exists"
    );
    assert_eq!(
        bridge_binding["intentBindingPath"],
        json!(intent_binding_path.to_string_lossy().to_string())
    );
    let run_audit_bundle_path = run_json["auditBundlePath"]
        .as_str()
        .expect("run output should include auditBundlePath");
    let bridge_audit_bundle_path = bridge_binding["auditBundlePath"]
        .as_str()
        .expect("formal run binding should include auditBundlePath");
    assert_eq!(
        bridge_audit_bundle_path, run_audit_bundle_path,
        "formal run binding should point back to the managed audit bundle"
    );

    let bridge_repeat = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge_repeat, "bridge-run should be idempotent");
    let bridge_repeat_json = parse_stdout_json(&bridge_repeat, "bridge-run repeat output");
    assert_eq!(bridge_repeat_json["taskId"], json!(task_id.clone()));
    assert_eq!(bridge_repeat_json["runId"], json!(run_id.clone()));

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_ids = evidence_json["evidenceIds"]
        .as_array()
        .expect("evidenceIds should be an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("evidence id should be a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        evidence_ids.len(),
        9,
        "should persist the expected native/runtime Evidence records"
    );
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("persist-evidence bindingPath should be present"),
    );
    assert!(
        evidence_binding_path.exists(),
        "persist-evidence should materialize an evidence binding artifact"
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    assert_eq!(
        evidence_binding["schema"],
        json!("libra.claude_evidence_binding.v1")
    );
    assert_eq!(
        evidence_binding["aiSessionId"],
        json!(ai_session_id.clone())
    );
    assert_eq!(
        evidence_binding["providerSessionId"],
        json!(provider_session_id.clone())
    );
    assert_eq!(evidence_binding["runId"], json!(run_id.clone()));
    assert_eq!(
        evidence_binding["runBindingPath"],
        json!(bridge_binding_path.to_string_lossy().to_string())
    );
    assert_eq!(evidence_binding["evidenceIds"], json!(evidence_ids.clone()));
    assert_eq!(
        evidence_binding["evidences"]
            .as_array()
            .expect("evidences should be an array")
            .len(),
        evidence_ids.len(),
        "binding should retain one entry per persisted Evidence object"
    );
    let evidence_entries = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array");
    let bound_evidence_ids = evidence_entries
        .iter()
        .map(|entry| {
            entry["evidenceId"]
                .as_str()
                .expect("evidence entry should include evidenceId")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    let bound_evidence_kinds = evidence_entries
        .iter()
        .map(|entry| {
            assert!(
                entry["sourcePath"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty()),
                "evidence entry should include a non-empty sourcePath"
            );
            entry["kind"]
                .as_str()
                .expect("evidence entry should include kind")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        bound_evidence_ids,
        evidence_ids.iter().cloned().collect::<BTreeSet<_>>(),
        "binding entries should reference exactly the persisted Evidence ids"
    );
    assert_eq!(
        bound_evidence_kinds,
        BTreeSet::from([
            "evidence_input_summary".to_string(),
            "intent_extraction_result".to_string(),
            "managed_context_runtime_summary".to_string(),
            "managed_decision_runtime_summary".to_string(),
            "managed_provenance_summary".to_string(),
            "managed_task_runtime_summary".to_string(),
            "managed_tool_runtime_summary".to_string(),
            "managed_usage_summary".to_string(),
            "provider_session_snapshot".to_string(),
        ]),
        "binding should preserve the expected evidence kinds"
    );
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("persist-evidence bindingPath should be present"),
    );
    assert!(
        evidence_binding_path.exists(),
        "persist-evidence should materialize an evidence binding artifact"
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    assert_eq!(
        evidence_binding["schema"],
        json!("libra.claude_evidence_binding.v1")
    );
    assert_eq!(
        evidence_binding["aiSessionId"],
        json!(ai_session_id.clone())
    );
    assert_eq!(
        evidence_binding["providerSessionId"],
        json!(provider_session_id.clone())
    );
    assert_eq!(evidence_binding["runId"], json!(run_id.clone()));
    assert_eq!(
        evidence_binding["runBindingPath"],
        json!(bridge_binding_path.to_string_lossy().to_string())
    );
    assert_eq!(evidence_binding["evidenceIds"], json!(evidence_ids.clone()));
    assert_eq!(
        evidence_binding["evidences"]
            .as_array()
            .expect("evidences should be an array")
            .len(),
        evidence_ids.len(),
        "binding should retain one entry per persisted Evidence object"
    );
    let evidence_entries = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array");
    let bound_evidence_ids = evidence_entries
        .iter()
        .map(|entry| {
            entry["evidenceId"]
                .as_str()
                .expect("evidence entry should include evidenceId")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    let bound_evidence_kinds = evidence_entries
        .iter()
        .map(|entry| {
            assert!(
                entry["sourcePath"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty()),
                "evidence entry should include a non-empty sourcePath"
            );
            entry["kind"]
                .as_str()
                .expect("evidence entry should include kind")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        bound_evidence_ids,
        evidence_ids.iter().cloned().collect::<BTreeSet<_>>(),
        "binding entries should reference exactly the persisted Evidence ids"
    );
    assert_eq!(
        bound_evidence_kinds,
        BTreeSet::from([
            "evidence_input_summary".to_string(),
            "intent_extraction_result".to_string(),
            "managed_context_runtime_summary".to_string(),
            "managed_decision_runtime_summary".to_string(),
            "managed_provenance_summary".to_string(),
            "managed_task_runtime_summary".to_string(),
            "managed_tool_runtime_summary".to_string(),
            "managed_usage_summary".to_string(),
            "provider_session_snapshot".to_string(),
        ]),
        "binding should preserve the expected evidence kinds"
    );

    let evidence_repeat = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence_repeat, "persist-evidence should be idempotent");
    let evidence_repeat_json =
        parse_stdout_json(&evidence_repeat, "persist-evidence repeat output");
    assert_eq!(
        evidence_repeat_json["evidenceIds"],
        json!(evidence_ids.clone())
    );

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision, "persist-decision should succeed");
    let decision_json = parse_stdout_json(&decision, "persist-decision output");
    let decision_id = decision_json["decisionId"]
        .as_str()
        .expect("decisionId should be present")
        .to_string();
    assert_eq!(decision_json["decisionType"], json!("checkpoint"));

    let decision_repeat = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision_repeat, "persist-decision should be idempotent");
    let decision_repeat_json =
        parse_stdout_json(&decision_repeat, "persist-decision repeat output");
    assert_eq!(
        decision_repeat_json["decisionId"],
        json!(decision_id.clone())
    );

    let task: Task = read_tracked_object(repo.path(), "task", &task_id).await;
    assert_eq!(task.intent().map(|id| id.to_string()), Some(intent_id));

    let formal_run: Run = read_tracked_object(repo.path(), "run", &run_id).await;
    assert_eq!(formal_run.task().to_string(), task_id);
    assert!(
        formal_run.plan().is_none(),
        "formal run should remain unplanned when the managed artifact lacks numbered assistant plan text"
    );

    let evidence_objects = futures::future::join_all(
        evidence_ids
            .iter()
            .map(|id| read_tracked_object::<Evidence>(repo.path(), "evidence", id)),
    )
    .await;
    let evidence_kinds = evidence_objects
        .iter()
        .map(|evidence| evidence.kind().to_string())
        .collect::<Vec<_>>();
    assert!(evidence_kinds.contains(&"provider_session_snapshot".to_string()));
    assert!(evidence_kinds.contains(&"evidence_input_summary".to_string()));
    assert!(evidence_kinds.contains(&"intent_extraction_result".to_string()));
    assert!(evidence_kinds.contains(&"managed_provenance_summary".to_string()));
    assert!(evidence_kinds.contains(&"managed_usage_summary".to_string()));
    assert!(evidence_kinds.contains(&"managed_tool_runtime_summary".to_string()));
    assert!(evidence_kinds.contains(&"managed_task_runtime_summary".to_string()));
    assert!(evidence_kinds.contains(&"managed_decision_runtime_summary".to_string()));
    assert!(evidence_kinds.contains(&"managed_context_runtime_summary".to_string()));
    assert!(
        evidence_objects
            .iter()
            .all(|evidence| evidence.run_id().to_string() == run_id),
        "all Evidence records should point at the bridged run"
    );

    let stored_decision: Decision =
        read_tracked_object(repo.path(), "decision", &decision_id).await;
    assert_eq!(stored_decision.run_id().to_string(), run_id);
    assert_eq!(stored_decision.decision_type().to_string(), "checkpoint");
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_links_patchset_into_evidence_and_decision() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn patchset_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run patchset bridge");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();
    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    let run_id = bridge_json["runId"]
        .as_str()
        .expect("runId should be present")
        .to_string();

    let managed_evidence_input = run_libra_command(
        &[
            "claude-sdk",
            "build-managed-evidence-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &managed_evidence_input,
        "build-managed-evidence-input should succeed",
    );
    let managed_evidence_input_json = parse_stdout_json(
        &managed_evidence_input,
        "build-managed-evidence-input output",
    );
    let managed_evidence_input_path = managed_evidence_input_json["artifactPath"]
        .as_str()
        .expect("artifactPath should be present")
        .to_string();

    let patchset = run_libra_command(
        &[
            "claude-sdk",
            "persist-patchset",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&patchset, "persist-patchset should succeed");
    let patchset_json = parse_stdout_json(&patchset, "persist-patchset output");
    let patchset_id = patchset_json["patchsetId"]
        .as_str()
        .expect("patchsetId should be present")
        .to_string();
    let patchset_binding_path = PathBuf::from(
        patchset_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let patchset_binding = read_json_file(&patchset_binding_path);
    assert_eq!(
        patchset_binding["schema"],
        json!("libra.claude_patchset_binding.v1")
    );
    assert_eq!(
        patchset_binding["aiSessionId"],
        json!(ai_session_id.clone())
    );
    assert_eq!(patchset_binding["runId"], json!(run_id.clone()));
    assert_eq!(patchset_binding["patchsetId"], json!(patchset_id.clone()));
    assert_eq!(
        patchset_binding["managedEvidenceInputPath"],
        json!(managed_evidence_input_path)
    );

    let patchset_repeat = run_libra_command(
        &[
            "claude-sdk",
            "persist-patchset",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&patchset_repeat, "persist-patchset should be idempotent");
    let patchset_repeat_json =
        parse_stdout_json(&patchset_repeat, "persist-patchset repeat output");
    assert_eq!(
        patchset_repeat_json["patchsetId"],
        json!(patchset_id.clone())
    );

    let patchset_ai = run_libra_command(
        &["cat-file", "--ai", &format!("patchset:{patchset_id}")],
        repo.path(),
    );
    assert_cli_success(&patchset_ai, "cat-file --ai should succeed for patchset");
    let patchset_stdout = String::from_utf8_lossy(&patchset_ai.stdout);
    assert!(
        patchset_stdout.contains("src/lib.rs"),
        "cat-file should surface touched patch paths for the formal patchset"
    );

    let stored_patchset: PatchSet =
        read_tracked_object(repo.path(), "patchset", &patchset_id).await;
    assert_eq!(stored_patchset.run().to_string(), run_id);
    assert_eq!(stored_patchset.touched().len(), 1);
    assert_eq!(stored_patchset.touched()[0].path, "src/lib.rs");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_ids = evidence_json["evidenceIds"]
        .as_array()
        .expect("evidenceIds should be an array")
        .iter()
        .map(|value| value.as_str().expect("evidence id").to_string())
        .collect::<Vec<_>>();
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    assert_eq!(
        evidence_binding["patchsetBindingPath"],
        json!(patchset_binding_path.to_string_lossy().to_string())
    );
    assert_eq!(evidence_binding["patchsetId"], json!(patchset_id.clone()));

    let patchset_uuid = Uuid::parse_str(&patchset_id).expect("patchsetId should be a UUID");
    let evidence_objects = futures::future::join_all(
        evidence_ids
            .iter()
            .map(|id| read_tracked_object::<Evidence>(repo.path(), "evidence", id)),
    )
    .await;
    assert!(
        evidence_objects
            .iter()
            .all(|evidence| evidence.patchset_id() == Some(patchset_uuid)),
        "all Claude evidence objects should point at the persisted formal patchset"
    );

    let decision_input = run_libra_command(
        &[
            "claude-sdk",
            "build-decision-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision_input, "build-decision-input should succeed");

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision, "persist-decision should succeed");
    let decision_json = parse_stdout_json(&decision, "persist-decision output");
    let decision_id = decision_json["decisionId"]
        .as_str()
        .expect("decisionId should be present")
        .to_string();
    let decision_binding_path = PathBuf::from(
        decision_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let decision_binding = read_json_file(&decision_binding_path);
    assert_eq!(
        decision_binding["patchsetBindingPath"],
        json!(patchset_binding_path.to_string_lossy().to_string())
    );
    assert_eq!(decision_binding["patchsetId"], json!(patchset_id.clone()));

    let stored_decision: Decision =
        read_tracked_object(repo.path(), "decision", &decision_id).await;
    assert_eq!(
        stored_decision.chosen_patchset_id(),
        Some(patchset_uuid),
        "the formal decision should retain the chosen patchset link"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_patchset_stores_diff_artifact_for_edit_payloads() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    let old_text = "pub fn patchset_diff() {}\n";
    let new_text = "pub fn patchset_diff() {}\n// diff artifact\n";
    fs::write(&touched_file, old_text).expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_edit_artifact(
            repo.path(),
            &touched_file,
            old_text,
            new_text,
        ))
        .expect("serialize edit artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();
    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let managed_evidence_input = run_libra_command(
        &[
            "claude-sdk",
            "build-managed-evidence-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &managed_evidence_input,
        "build-managed-evidence-input should succeed",
    );

    let patchset = run_libra_command(
        &[
            "claude-sdk",
            "persist-patchset",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&patchset, "persist-patchset should succeed");
    let patchset_json = parse_stdout_json(&patchset, "persist-patchset output");
    let patchset_id = patchset_json["patchsetId"]
        .as_str()
        .expect("patchsetId should be present")
        .to_string();
    let patchset_binding_path = PathBuf::from(
        patchset_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let patchset_binding = read_json_file(&patchset_binding_path);
    assert_eq!(patchset_binding["diffArtifactStore"], json!("libra"));
    let diff_artifact_key = patchset_binding["diffArtifactKey"]
        .as_str()
        .expect("diffArtifactKey should be present");

    let stored_patchset: PatchSet =
        read_tracked_object(repo.path(), "patchset", &patchset_id).await;
    let artifact_ref = stored_patchset
        .artifact()
        .expect("patchset should include a diff artifact ref");
    assert_eq!(artifact_ref.store(), "libra");
    assert_eq!(artifact_ref.key(), diff_artifact_key);
    assert_eq!(stored_patchset.touched().len(), 1);
    assert_eq!(stored_patchset.touched()[0].lines_added, 2);
    assert_eq!(stored_patchset.touched()[0].lines_deleted, 1);

    let storage = LocalStorage::new(repo.path().join(".libra").join("objects"));
    let diff_hash: ObjectHash = artifact_ref
        .key()
        .parse()
        .expect("diff artifact key should parse as object hash");
    let (bytes, _) = storage
        .get(&diff_hash)
        .await
        .expect("diff artifact should exist in object storage");
    let diff_text = String::from_utf8(bytes).expect("diff artifact should be UTF-8");
    assert!(diff_text.contains("diff --git a/src/lib.rs b/src/lib.rs"));
    assert!(diff_text.contains("-pub fn patchset_diff() {}"));
    assert!(diff_text.contains("+// diff artifact"));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_live_e2e_persists_full_object_flow() {
    if std::env::var("LIBRA_TEST_CLAUDE_SDK_LIVE").map_or(true, |value| value.is_empty()) {
        eprintln!("skipped (LIBRA_TEST_CLAUDE_SDK_LIVE not set)");
        return;
    }

    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn live_claude_sdk_e2e() {}\n").expect("write source file");

    let prompt_path = repo.path().join("live-claude-sdk-prompt.txt");
    fs::write(
        &prompt_path,
        "Use the Edit or Write tool to modify only src/lib.rs. Append exactly one new line at the end of the file: // live claude sdk e2e . Do not only describe the change. Actually write the file change, do not modify any other file, and stop immediately after the edit.",
    )
    .expect("write live prompt");

    let mut command = build_live_claude_sdk_command(
        repo.path(),
        &prompt_path,
        "acceptEdits",
        "60",
        &["Read", "Edit", "Write", "Glob", "Grep"],
        None,
    );
    let output = command
        .output()
        .expect("failed to execute live Claude SDK command");
    assert!(
        output.status.success(),
        "live claude-sdk run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run_events = parse_stdout_ndjson(&output, "live claude-sdk run output");
    let run_json = find_ndjson_event(&run_events, "libra_result", "live claude-sdk run output");
    let auto_finalize = run_json
        .get("autoFinalize")
        .expect("libra_result should include autoFinalize summary");
    assert!(
        auto_finalize
            .get("warnings")
            .is_none_or(|warnings| warnings.as_array().is_some_and(|items| items.is_empty())),
        "live auto-finalize should complete without warnings: {auto_finalize}"
    );
    let patchset_id = auto_finalize["patchsetId"]
        .as_str()
        .expect("autoFinalize.patchsetId should be present")
        .to_string();
    let decision_id = auto_finalize["decisionId"]
        .as_str()
        .expect("autoFinalize.decisionId should be present")
        .to_string();

    let (_, history) = load_intent_history(repo.path()).await;
    let evidence_objects = history
        .list_objects("evidence")
        .await
        .expect("list live evidence objects");
    let first_evidence_id = evidence_objects
        .first()
        .map(|(id, _)| id.clone())
        .expect("auto-finalize should persist at least one evidence object");
    let provenance_objects = history
        .list_objects("provenance")
        .await
        .expect("list live provenance objects");
    let run_snapshot_objects = history
        .list_objects("run_snapshot")
        .await
        .expect("list live run snapshot objects");
    let patchset_snapshot_objects = history
        .list_objects("patchset_snapshot")
        .await
        .expect("list live patchset snapshot objects");
    let provenance_snapshot_objects = history
        .list_objects("provenance_snapshot")
        .await
        .expect("list live provenance snapshot objects");
    let run_usage_objects = history
        .list_objects("run_usage")
        .await
        .expect("list live run usage objects");
    let snapshot_objects = history
        .list_objects("snapshot")
        .await
        .expect("list live context snapshots");
    let context_frame_objects = history
        .list_objects("context_frame")
        .await
        .expect("list live context frames");
    assert_eq!(provenance_objects.len(), 1);
    assert_eq!(run_snapshot_objects.len(), 1);
    assert_eq!(patchset_snapshot_objects.len(), 1);
    assert_eq!(provenance_snapshot_objects.len(), 1);
    assert_eq!(run_usage_objects.len(), 1);
    assert_eq!(snapshot_objects.len(), 1);
    assert!(
        !context_frame_objects.is_empty(),
        "live auto-finalize should persist at least one context frame"
    );

    for (object_id, object_type) in [
        (patchset_id.as_str(), "patchset"),
        (first_evidence_id.as_str(), "evidence"),
        (decision_id.as_str(), "decision"),
        (provenance_objects[0].0.as_str(), "provenance"),
        (run_snapshot_objects[0].0.as_str(), "run_snapshot"),
        (patchset_snapshot_objects[0].0.as_str(), "patchset_snapshot"),
        (
            provenance_snapshot_objects[0].0.as_str(),
            "provenance_snapshot",
        ),
        (run_usage_objects[0].0.as_str(), "run_usage"),
        (snapshot_objects[0].0.as_str(), "snapshot"),
        (context_frame_objects[0].0.as_str(), "context_frame"),
    ] {
        let selector = format!("{object_type}:{object_id}");
        let ai_type = run_libra_command(&["cat-file", "--ai-type", &selector], repo.path());
        assert_cli_success(
            &ai_type,
            "cat-file --ai-type should succeed for live objects",
        );
        assert_eq!(String::from_utf8_lossy(&ai_type.stdout).trim(), object_type);
    }
    assert!(
        fs::read_to_string(&touched_file)
            .expect("read live-edited file")
            .contains("// live claude sdk e2e"),
        "live Claude SDK test should verify the requested file edit actually landed"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_live_e2e_persists_plan_family_snapshot_events() {
    if std::env::var("LIBRA_TEST_CLAUDE_SDK_LIVE").map_or(true, |value| value.is_empty()) {
        eprintln!("skipped (LIBRA_TEST_CLAUDE_SDK_LIVE not set)");
        return;
    }

    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn live_claude_sdk_plan_family() {}\n")
        .expect("write source file");

    let prompt_path = repo.path().join("live-claude-sdk-plan-prompt.txt");
    fs::write(
        &prompt_path,
        "Before using tools, output exactly one concise numbered 3-step plan in assistant text. The plan must use lines starting with 1., 2., and 3. Then inspect src/lib.rs with read-only tools if needed, produce the required structured output, and stop without editing files.",
    )
    .expect("write live plan prompt");

    let libra_bin = env!("CARGO_BIN_EXE_libra");
    let shell_override = std::env::var("LIBRA_TEST_CLAUDE_LIVE_SHELL")
        .ok()
        .filter(|value| !value.is_empty());
    let sdk_module_override = std::env::var("LIBRA_CLAUDE_AGENT_SDK_MODULE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let default_path = "/tmp/claude-sdk-probe/node_modules/@anthropic-ai/claude-agent-sdk";
            Path::new(default_path)
                .exists()
                .then(|| default_path.to_string())
        });

    let mut command = if let Some(shell) = shell_override.as_deref() {
        let shell_command = format!(
            "cd {}; {} --json=ndjson claude-sdk run --prompt-file {} --model haiku --permission-mode acceptEdits --timeout-seconds 60 --tool Read --tool Glob --tool Grep",
            shell_double_quote(
                repo.path()
                    .to_str()
                    .expect("repo path should be valid UTF-8")
            ),
            shell_double_quote(libra_bin),
            shell_double_quote(
                prompt_path
                    .to_str()
                    .expect("prompt path should be valid UTF-8")
            ),
        );
        let mut command = std::process::Command::new(shell);
        command.arg("-lc").arg(shell_command);
        command
    } else {
        let mut command = std::process::Command::new(libra_bin);
        command.current_dir(repo.path()).args([
            "--json=ndjson",
            "claude-sdk",
            "run",
            "--prompt-file",
            prompt_path
                .to_str()
                .expect("prompt path should be valid UTF-8"),
            "--model",
            "haiku",
            "--permission-mode",
            "acceptEdits",
            "--timeout-seconds",
            "60",
            "--tool",
            "Read",
            "--tool",
            "Glob",
            "--tool",
            "Grep",
        ]);
        command
    };
    if let Some(module_path) = sdk_module_override.as_deref() {
        command.env("LIBRA_CLAUDE_AGENT_SDK_MODULE", module_path);
    }

    let output = command
        .output()
        .expect("failed to execute live Claude SDK plan command");
    assert!(
        output.status.success(),
        "live claude-sdk plan run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_events = parse_stdout_ndjson(&output, "live claude-sdk plan output");
    let run_json = find_ndjson_event(
        &stdout_events,
        "libra_result",
        "live claude-sdk plan output",
    );
    let raw_artifact_path = PathBuf::from(
        run_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath should be present"),
    );
    let raw_artifact = read_json_file(&raw_artifact_path);

    let intent_snapshots = list_history_object_ids(repo.path(), "intent_snapshot").await;
    let intent_event_ids = list_history_object_ids(repo.path(), "intent_event").await;
    let plan_snapshots = list_history_object_ids(repo.path(), "plan_snapshot").await;
    let plan_step_snapshots = list_history_object_ids(repo.path(), "plan_step_snapshot").await;
    let task_snapshots = list_history_object_ids(repo.path(), "task_snapshot").await;
    let plan_step_event_ids = list_history_object_ids(repo.path(), "plan_step_event").await;
    let run_snapshots = list_history_object_ids(repo.path(), "run_snapshot").await;
    let run_event_ids = list_history_object_ids(repo.path(), "run_event").await;
    let provenance_snapshots = list_history_object_ids(repo.path(), "provenance_snapshot").await;

    if plan_snapshots.is_empty() {
        panic!(
            "prompt-contract broken: no plan_snapshot objects were produced. raw_artifact_path={} raw_stdout={}",
            raw_artifact_path.display(),
            String::from_utf8_lossy(&output.stdout)
        );
    }

    assert_eq!(intent_snapshots.len(), 1);
    assert!(
        !intent_event_ids.is_empty(),
        "live plan path should emit formal intent lifecycle events"
    );
    assert_eq!(plan_snapshots.len(), 1);
    assert_eq!(plan_step_snapshots.len(), 3);
    assert_eq!(task_snapshots.len(), 3);
    assert_eq!(plan_step_event_ids.len(), 3);
    assert_eq!(run_snapshots.len(), 1);
    assert!(
        !run_event_ids.is_empty(),
        "live plan path should emit formal run lifecycle events"
    );
    assert_eq!(provenance_snapshots.len(), 1);

    let intent_event_values = futures::future::join_all(
        intent_event_ids
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "intent_event", id)),
    )
    .await;
    let intent_statuses = intent_event_values
        .iter()
        .filter_map(|value| value["status"].as_str().or_else(|| value["kind"].as_str()))
        .collect::<BTreeSet<_>>();
    assert!(
        intent_statuses.contains("created")
            || intent_statuses.contains("analyzed")
            || intent_statuses.contains("completed"),
        "live plan path should expose meaningful intent lifecycle semantics"
    );

    let run_event_values = futures::future::join_all(
        run_event_ids
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "run_event", id)),
    )
    .await;
    let run_statuses = run_event_values
        .iter()
        .filter_map(|value| value["status"].as_str().or_else(|| value["kind"].as_str()))
        .collect::<BTreeSet<_>>();
    assert!(
        run_statuses.contains("created")
            || run_statuses.contains("completed")
            || run_statuses.contains("failed"),
        "live plan path should expose meaningful run lifecycle semantics"
    );

    let plan_snapshot: Value =
        read_tracked_object(repo.path(), "plan_snapshot", &plan_snapshots[0]).await;
    assert!(
        plan_snapshot["step_text"]
            .as_str()
            .is_some_and(|text| text.contains("1.") && text.contains("2.") && text.contains("3.")),
        "live plan snapshot should preserve numbered plan text: {plan_snapshot}"
    );
    assert!(
        raw_artifact["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| {
                message.get("type").and_then(Value::as_str) == Some("assistant")
                    && message
                        .get("message")
                        .and_then(|inner| inner.get("content"))
                        .and_then(Value::as_array)
                        .is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block.get("type").and_then(Value::as_str) == Some("text")
                                    && block.get("text").and_then(Value::as_str).is_some_and(
                                        |text| {
                                            text.contains("1.")
                                                && text.contains("2.")
                                                && text.contains("3.")
                                        },
                                    )
                            })
                        })
            })),
        "raw managed artifact should retain numbered assistant plan text"
    );

    let plan_step_snapshot_ids = plan_step_snapshots.iter().cloned().collect::<BTreeSet<_>>();
    let live_plan_thread_id = plan_snapshot["thread_id"].clone();
    let live_plan_id = Value::String(plan_snapshots[0].clone());
    let task_snapshot_values = futures::future::join_all(
        task_snapshots
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "task_snapshot", id)),
    )
    .await;
    assert!(
        task_snapshot_values.iter().all(|value| {
            value["origin_step_id"]
                .as_str()
                .is_some_and(|id| plan_step_snapshot_ids.contains(id))
                && value["thread_id"] == live_plan_thread_id
                && value["plan_id"] == live_plan_id
                && value["intent_id"].is_string()
        }),
        "live task snapshots should stay projection-only views over the derived plan-step ids"
    );

    let plan_step_event_values = futures::future::join_all(
        plan_step_event_ids
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "plan_step_event", id)),
    )
    .await;
    assert!(
        plan_step_event_values.iter().all(|value| {
            value["status"] == json!("pending")
                && value["step_id"]
                    .as_str()
                    .is_some_and(|id| plan_step_snapshot_ids.contains(id))
                && value["run_id"].is_string()
        }),
        "live plan path should use formal pending plan_step_event semantics for derived steps"
    );

    for (object_id, object_type) in [
        (intent_snapshots[0].as_str(), "intent_snapshot"),
        (plan_snapshots[0].as_str(), "plan_snapshot"),
        (plan_step_snapshots[0].as_str(), "plan_step_snapshot"),
        (task_snapshots[0].as_str(), "task_snapshot"),
        (plan_step_event_ids[0].as_str(), "plan_step_event"),
        (run_snapshots[0].as_str(), "run_snapshot"),
        (provenance_snapshots[0].as_str(), "provenance_snapshot"),
    ] {
        assert_ai_type_matches(repo.path(), object_id, object_type);
    }
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_live_e2e_persists_derived_audit_objects() {
    if std::env::var("LIBRA_TEST_CLAUDE_SDK_LIVE").map_or(true, |value| value.is_empty()) {
        eprintln!("skipped (LIBRA_TEST_CLAUDE_SDK_LIVE not set)");
        return;
    }

    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn live_claude_sdk_audit_objects() {}\n")
        .expect("write source file");

    let prompt_path = repo.path().join("live-claude-sdk-audit-prompt.txt");
    fs::write(
        &prompt_path,
        "Inspect src/lib.rs, then use Edit or Write to append exactly one new line at the end of src/lib.rs: // live claude sdk audit objects . Do not modify any other file, and stop immediately after the edit.",
    )
    .expect("write live prompt");

    let scripted_responses = serde_json::to_string(&json!([
        { "kind": "tool_approval", "decision": "approve" },
        { "kind": "tool_approval", "decision": "approve" },
        { "kind": "tool_approval", "decision": "approve" }
    ]))
    .expect("serialize scripted responses");

    let mut command = build_live_claude_sdk_command(
        repo.path(),
        &prompt_path,
        "default",
        "60",
        &["Read", "Edit", "Write", "Glob", "Grep"],
        Some(&scripted_responses),
    );

    let output = command
        .output()
        .expect("failed to execute live Claude SDK command");
    assert!(
        output.status.success(),
        "live claude-sdk run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run_events = parse_stdout_ndjson(&output, "live claude-sdk audit output");
    let run_json = find_ndjson_event(&run_events, "libra_result", "live claude-sdk audit output");
    let raw_artifact_path = PathBuf::from(
        run_json["rawArtifactPath"]
            .as_str()
            .expect("rawArtifactPath should be present"),
    );
    let raw_artifact = read_json_file(&raw_artifact_path);

    let (_, history) = load_intent_history(repo.path()).await;
    let approval_requests = history
        .list_objects("approval_request")
        .await
        .expect("list live approval_request objects");
    assert!(
        !approval_requests.is_empty(),
        "live run should persist at least one approval_request"
    );
    let tool_invocation_events = history
        .list_objects("tool_invocation_event")
        .await
        .expect("list live tool_invocation_event objects");
    assert!(
        !tool_invocation_events.is_empty(),
        "live run should persist tool_invocation_event objects"
    );
    let reasoning_objects = history
        .list_objects("reasoning")
        .await
        .expect("list live reasoning objects");
    let has_thinking = raw_artifact["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message.get("type").and_then(Value::as_str) == Some("assistant")
                && message
                    .get("message")
                    .and_then(|inner| inner.get("content"))
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            block.get("type").and_then(Value::as_str) == Some("thinking")
                        })
                    })
        })
    });
    if has_thinking {
        assert!(
            !reasoning_objects.is_empty(),
            "live run exposed thinking blocks, so reasoning objects should exist"
        );
    } else {
        assert!(
            reasoning_objects.is_empty(),
            "without live thinking blocks, no reasoning objects should be derived"
        );
    }

    for (object_id, object_type) in [
        (approval_requests[0].0.as_str(), "approval_request"),
        (
            tool_invocation_events[0].0.as_str(),
            "tool_invocation_event",
        ),
    ] {
        let ai_type = run_libra_command(&["cat-file", "--ai-type", object_id], repo.path());
        assert_cli_success(
            &ai_type,
            "cat-file --ai-type should succeed for live derived audit objects",
        );
        assert_eq!(String::from_utf8_lossy(&ai_type.stdout).trim(), object_type);
    }
    if has_thinking {
        let ai_type = run_libra_command(
            &["cat-file", "--ai-type", &reasoning_objects[0].0],
            repo.path(),
        );
        assert_cli_success(
            &ai_type,
            "cat-file --ai-type should succeed for live reasoning objects",
        );
        assert_eq!(String::from_utf8_lossy(&ai_type.stdout).trim(), "reasoning");
    }
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_recognizes_managed_input_layers() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn managed_input_layers() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let helper_path = repo.path().join("fake-managed-helper.sh");
    write_shell_helper(&helper_path, &artifact_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--enable-file-checkpointing",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();
    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&resolve, "resolve-extraction should succeed");

    let persist_intent = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&persist_intent, "persist-intent should succeed");

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let managed_evidence_input = run_libra_command(
        &[
            "claude-sdk",
            "build-managed-evidence-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &managed_evidence_input,
        "build-managed-evidence-input should succeed",
    );
    let managed_evidence_input_json = parse_stdout_json(
        &managed_evidence_input,
        "build-managed-evidence-input output",
    );
    let managed_evidence_input_path = managed_evidence_input_json["artifactPath"]
        .as_str()
        .expect("artifactPath should be present")
        .to_string();

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    assert_eq!(
        evidence_binding["managedEvidenceInputPath"],
        json!(managed_evidence_input_path)
    );
    let bound_evidence_kinds = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array")
        .iter()
        .map(|entry| {
            entry["kind"]
                .as_str()
                .expect("kind should be a string")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert!(
        bound_evidence_kinds.contains("managed_evidence_input_summary"),
        "persist-evidence should recognize the managed evidence input layer"
    );

    let decision_input = run_libra_command(
        &[
            "claude-sdk",
            "build-decision-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision_input, "build-decision-input should succeed");
    let decision_input_json = parse_stdout_json(&decision_input, "build-decision-input output");
    let decision_input_path = decision_input_json["artifactPath"]
        .as_str()
        .expect("artifactPath should be present")
        .to_string();

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision, "persist-decision should succeed");
    let decision_json = parse_stdout_json(&decision, "persist-decision output");
    let decision_binding_path = PathBuf::from(
        decision_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let decision_binding = read_json_file(&decision_binding_path);
    assert_eq!(
        decision_binding["decisionInputPath"],
        json!(decision_input_path)
    );
    assert!(
        decision_binding["rationale"]
            .as_str()
            .is_some_and(|rationale| rationale.contains("decision_input_runtime_events=4")),
        "persist-decision should recognize the decision input layer"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_patchset_links_formal_evidence_and_decision_objects() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn patchset_chain() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--enable-file-checkpointing",
            "true",
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();
    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let managed_evidence_input = run_libra_command(
        &[
            "claude-sdk",
            "build-managed-evidence-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &managed_evidence_input,
        "build-managed-evidence-input should succeed",
    );

    let patchset = run_libra_command(
        &[
            "claude-sdk",
            "persist-patchset",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&patchset, "persist-patchset should succeed");
    let patchset_json = parse_stdout_json(&patchset, "persist-patchset output");
    let patchset_id = patchset_json["patchsetId"]
        .as_str()
        .expect("patchsetId should be present")
        .to_string();
    let patchset_binding_path = PathBuf::from(
        patchset_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_ids = evidence_json["evidenceIds"]
        .as_array()
        .expect("evidenceIds should be an array")
        .iter()
        .map(|value| value.as_str().expect("evidence id").to_string())
        .collect::<Vec<_>>();
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    assert_eq!(evidence_binding["patchsetId"], json!(patchset_id));
    assert_eq!(
        evidence_binding["patchsetBindingPath"],
        json!(patchset_binding_path.to_string_lossy().to_string())
    );

    let decision_input = run_libra_command(
        &[
            "claude-sdk",
            "build-decision-input",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision_input, "build-decision-input should succeed");

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision, "persist-decision should succeed");
    let decision_json = parse_stdout_json(&decision, "persist-decision output");
    let decision_id = decision_json["decisionId"]
        .as_str()
        .expect("decisionId should be present")
        .to_string();
    let decision_binding_path = PathBuf::from(
        decision_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let decision_binding = read_json_file(&decision_binding_path);
    assert_eq!(decision_binding["patchsetId"], json!(patchset_id));
    assert_eq!(
        decision_binding["patchsetBindingPath"],
        json!(patchset_binding_path.to_string_lossy().to_string())
    );
    assert!(
        decision_binding["rationale"]
            .as_str()
            .is_some_and(|rationale| rationale.contains(&format!("patchset_id={patchset_id}"))),
        "persist-decision should fold the formal patchset id into its rationale"
    );

    for (object_id, object_type) in [
        (patchset_id.as_str(), "patchset"),
        (evidence_ids[0].as_str(), "evidence"),
        (decision_id.as_str(), "decision"),
    ] {
        let selector = format!("{object_type}:{object_id}");
        let ai_type = run_libra_command(&["cat-file", "--ai-type", &selector], repo.path());
        assert_cli_success(&ai_type, "cat-file --ai-type should succeed");
        assert_eq!(String::from_utf8_lossy(&ai_type.stdout).trim(), object_type);

        let ai_pretty = run_libra_command(&["cat-file", "--ai", &selector], repo.path());
        assert_cli_success(&ai_pretty, "cat-file --ai should succeed");
        let pretty_stdout = String::from_utf8_lossy(&ai_pretty.stdout);
        assert!(
            pretty_stdout.contains(&format!("type: {object_type}")),
            "cat-file should pretty-print the {object_type} object"
        );
    }

    let patchset_pretty = run_libra_command(
        &["cat-file", "--ai", &format!("patchset:{patchset_id}")],
        repo.path(),
    );
    assert_cli_success(&patchset_pretty, "cat-file --ai patchset should succeed");
    assert!(
        String::from_utf8_lossy(&patchset_pretty.stdout).contains("src/lib.rs"),
        "cat-file patchset output should expose the touched file list"
    );

    let evidence_pretty = run_libra_command(
        &["cat-file", "--ai", &format!("evidence:{}", evidence_ids[0])],
        repo.path(),
    );
    assert_cli_success(&evidence_pretty, "cat-file --ai evidence should succeed");
    assert!(
        String::from_utf8_lossy(&evidence_pretty.stdout).contains(&patchset_id),
        "cat-file evidence output should expose the linked patchset id"
    );

    let decision_pretty = run_libra_command(
        &["cat-file", "--ai", &format!("decision:{decision_id}")],
        repo.path(),
    );
    assert_cli_success(&decision_pretty, "cat-file --ai decision should succeed");
    assert!(
        String::from_utf8_lossy(&decision_pretty.stdout).contains(&patchset_id),
        "cat-file decision output should expose the chosen patchset id"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_materializes_thread_scheduler_views() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn materialize_views() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed for projection rebuild test",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run projection rebuild");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();

    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    for args in [
        vec![
            "claude-sdk".to_string(),
            "resolve-extraction".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-intent".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "bridge-run".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "build-managed-evidence-input".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-patchset".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-evidence".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-decision".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
    ] {
        let argv = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_libra_command(&argv, repo.path());
        assert_cli_success(&output, "formal bridge command should succeed");
    }

    let (storage, history) = load_intent_history(repo.path()).await;
    let db_conn = libra::internal::db::establish_connection(
        repo.path()
            .join(".libra/libra.db")
            .to_str()
            .expect("db path should be valid UTF-8"),
    )
    .await
    .expect("failed to connect test database");
    let rebuilder = ProjectionRebuilder::new(storage.as_ref(), &history);
    let rebuild = rebuilder
        .materialize_latest_thread(&db_conn)
        .await
        .expect("materialize latest thread")
        .expect("projection rebuild should produce a thread");

    let intent_id = rebuild
        .thread
        .current_intent_id
        .expect("projection should select current intent");
    let stored_thread = ThreadProjection::find_by_intent_id(&db_conn, intent_id)
        .await
        .expect("load stored thread by intent")
        .expect("thread row should exist");
    assert_eq!(stored_thread.thread_id, rebuild.thread.thread_id);
    assert_eq!(
        stored_thread.current_intent_id,
        rebuild.thread.current_intent_id
    );

    let scheduler_row =
        ai_scheduler_state::Entity::find_by_id(rebuild.thread.thread_id.to_string())
            .one(&db_conn)
            .await
            .expect("query scheduler row")
            .expect("scheduler row should exist");
    assert_eq!(
        scheduler_row.active_task_id.as_deref(),
        rebuild
            .scheduler
            .active_task_id
            .as_ref()
            .map(Uuid::to_string)
            .as_deref()
    );
    assert_eq!(
        scheduler_row.active_run_id.as_deref(),
        rebuild
            .scheduler
            .active_run_id
            .as_ref()
            .map(Uuid::to_string)
            .as_deref()
    );

    let scheduler_metadata: Value = serde_json::from_str(
        scheduler_row
            .metadata_json
            .as_deref()
            .expect("scheduler metadata should exist"),
    )
    .expect("scheduler metadata should be JSON");
    assert_eq!(
        scheduler_metadata["projection_source"],
        json!("formal_history_rebuild_v1")
    );

    let task_run_rows = ai_index_task_run::Entity::find()
        .all(&db_conn)
        .await
        .expect("query task-run rows");
    assert_eq!(
        task_run_rows.len(),
        1,
        "bridge-run should produce one task->run row"
    );
    assert!(
        task_run_rows[0].is_latest,
        "the single task->run row should be marked latest"
    );

    let run_event_rows = ai_index_run_event::Entity::find()
        .all(&db_conn)
        .await
        .expect("query run-event rows");
    assert_eq!(
        run_event_rows.len(),
        1,
        "bridge-run should produce one run-event row"
    );
    assert!(
        run_event_rows[0].is_latest,
        "the single run-event row should be marked latest"
    );

    let run_patchset_rows = ai_index_run_patchset::Entity::find()
        .all(&db_conn)
        .await
        .expect("query run-patchset rows");
    assert_eq!(
        run_patchset_rows.len(),
        1,
        "persist-patchset should produce one run->patchset row"
    );
    assert!(
        run_patchset_rows[0].is_latest,
        "the single run->patchset row should be marked latest"
    );

    let plan_head_rows = ai_scheduler_plan_head::Entity::find()
        .all(&db_conn)
        .await
        .expect("query scheduler plan heads");
    assert!(
        plan_head_rows.is_empty(),
        "current Claude bridge should not synthesize plan heads yet"
    );

    let live_context_rows = ai_live_context_window::Entity::find()
        .all(&db_conn)
        .await
        .expect("query live context rows");
    assert!(
        !live_context_rows.is_empty(),
        "current Claude bridge should materialize live context frames from formal context objects"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_full_family_no_plan_snapshot_event_framework() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn full_family_no_plan() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed for no-plan snapshot framework",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run no-plan snapshot framework");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present")
        .to_string();
    stage_provider_session_evidence_artifacts(repo.path(), &provider_session_id);

    for args in [
        vec![
            "claude-sdk".to_string(),
            "resolve-extraction".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-intent".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "bridge-run".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "build-managed-evidence-input".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-patchset".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-evidence".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-decision".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
    ] {
        let argv = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_libra_command(&argv, repo.path());
        assert_cli_success(&output, "full-family no-plan command should succeed");
    }

    let intent_snapshots = list_history_object_ids(repo.path(), "intent_snapshot").await;
    let intent_events = list_history_object_ids(repo.path(), "intent_event").await;
    let run_snapshots = list_history_object_ids(repo.path(), "run_snapshot").await;
    let run_events = list_history_object_ids(repo.path(), "run_event").await;
    let patchset_snapshots = list_history_object_ids(repo.path(), "patchset_snapshot").await;
    let provenance_snapshots = list_history_object_ids(repo.path(), "provenance_snapshot").await;
    let plan_snapshots = list_history_object_ids(repo.path(), "plan_snapshot").await;
    let plan_step_snapshots = list_history_object_ids(repo.path(), "plan_step_snapshot").await;
    let task_snapshots = list_history_object_ids(repo.path(), "task_snapshot").await;
    let plan_step_events = list_history_object_ids(repo.path(), "plan_step_event").await;

    assert_eq!(intent_snapshots.len(), 1);
    assert!(
        intent_events.len() >= 2,
        "no-plan path should at least emit analyzed/created + completed intent events"
    );
    assert_eq!(run_snapshots.len(), 1);
    assert!(
        !run_events.is_empty(),
        "no-plan path should emit run_event records"
    );
    assert_eq!(patchset_snapshots.len(), 1);
    assert_eq!(provenance_snapshots.len(), 1);

    assert!(
        plan_snapshots.is_empty(),
        "no-plan path should not fabricate plan snapshots"
    );
    assert!(
        plan_step_snapshots.is_empty(),
        "no-plan path should not fabricate plan-step snapshots"
    );
    assert!(
        task_snapshots.is_empty(),
        "no-plan path should not fabricate per-step task snapshots"
    );
    assert!(
        plan_step_events.is_empty(),
        "no-plan path should not fabricate plan-step events"
    );

    let intent_snapshot: Value =
        read_tracked_object(repo.path(), "intent_snapshot", &intent_snapshots[0]).await;
    let formal_intent: Intent =
        read_tracked_object(repo.path(), "intent", &intent_snapshots[0]).await;
    assert_eq!(intent_snapshot["thread_id"], json!(ai_session_id));
    assert!(
        intent_snapshot["content"]
            .as_str()
            .is_some_and(|text| text == formal_intent.prompt()),
        "intent snapshot should mirror the persisted formal intent prompt"
    );

    let intent_event_values = futures::future::join_all(
        intent_events
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "intent_event", id)),
    )
    .await;
    let intent_statuses = intent_event_values
        .iter()
        .filter_map(|value| value["status"].as_str().or_else(|| value["kind"].as_str()))
        .collect::<BTreeSet<_>>();
    assert!(
        intent_statuses.contains("created") || intent_statuses.contains("analyzed"),
        "no-plan path should emit an initial intent lifecycle event"
    );
    assert!(intent_statuses.contains("completed"));

    let run_snapshot: Value =
        read_tracked_object(repo.path(), "run_snapshot", &run_snapshots[0]).await;
    assert_eq!(run_snapshot["thread_id"], json!(ai_session_id));
    assert!(run_snapshot["task_id"].is_string());
    assert!(run_snapshot["started_at"].is_string());

    let run_event_values = futures::future::join_all(
        run_events
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "run_event", id)),
    )
    .await;
    let run_statuses = run_event_values
        .iter()
        .filter_map(|value| value["status"].as_str().or_else(|| value["kind"].as_str()))
        .collect::<BTreeSet<_>>();
    assert!(
        run_statuses.contains("created")
            || run_statuses.contains("completed")
            || run_statuses.contains("failed")
            || run_statuses.contains("timed_out"),
        "no-plan path should emit at least one meaningful run lifecycle status"
    );

    let patchset_snapshot: Value =
        read_tracked_object(repo.path(), "patchset_snapshot", &patchset_snapshots[0]).await;
    assert_eq!(patchset_snapshot["status"], json!("completed"));
    assert!(patchset_snapshot["run_id"].is_string());

    let provenance_snapshot: Value =
        read_tracked_object(repo.path(), "provenance_snapshot", &provenance_snapshots[0]).await;
    assert_eq!(provenance_snapshot["provider"], json!("claude"));
    assert!(provenance_snapshot["run_id"].is_string());
    assert!(provenance_snapshot["parameters"].is_object());

    assert_ai_type_matches(repo.path(), &intent_snapshots[0], "intent_snapshot");
    assert_ai_type_matches(repo.path(), &run_snapshots[0], "run_snapshot");
    assert_ai_type_matches(repo.path(), &patchset_snapshots[0], "patchset_snapshot");
    assert_ai_type_matches(repo.path(), &provenance_snapshots[0], "provenance_snapshot");
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_full_family_plan_aware_snapshot_event_framework() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn full_family_plan_aware() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&plan_task_only_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            PLAN_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed for plan-aware snapshot framework",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run plan-aware snapshot framework");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let mut bridged_plan_id: Option<String> = None;

    for args in [
        vec![
            "claude-sdk".to_string(),
            "resolve-extraction".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-intent".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "bridge-run".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
    ] {
        let argv = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_libra_command(&argv, repo.path());
        assert_cli_success(&output, "plan-aware full-family command should succeed");
        if args[1] == "bridge-run" {
            let bridge_json = parse_stdout_json(&output, "plan-aware bridge-run output");
            bridged_plan_id = Some(
                bridge_json["planId"]
                    .as_str()
                    .expect("plan-aware bridge-run should emit planId")
                    .to_string(),
            );
        }
    }
    let plan_id = bridged_plan_id.expect("plan-aware full-family path should capture a plan id");

    let intent_snapshots = list_history_object_ids(repo.path(), "intent_snapshot").await;
    let intent_events = list_history_object_ids(repo.path(), "intent_event").await;
    let plan_snapshots = list_history_object_ids(repo.path(), "plan_snapshot").await;
    let plan_step_snapshots = list_history_object_ids(repo.path(), "plan_step_snapshot").await;
    let task_snapshots = list_history_object_ids(repo.path(), "task_snapshot").await;
    let plan_step_events = list_history_object_ids(repo.path(), "plan_step_event").await;
    let run_snapshots = list_history_object_ids(repo.path(), "run_snapshot").await;
    let provenance_snapshots = list_history_object_ids(repo.path(), "provenance_snapshot").await;

    assert_eq!(intent_snapshots.len(), 1);
    assert_eq!(plan_snapshots.len(), 1);
    assert_eq!(
        plan_step_snapshots.len(),
        real_plan_step_descriptions().len()
    );
    assert_eq!(task_snapshots.len(), real_plan_step_descriptions().len());
    assert_eq!(plan_step_events.len(), real_plan_step_descriptions().len());
    assert_eq!(run_snapshots.len(), 1);
    assert_eq!(provenance_snapshots.len(), 1);

    let rebridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            ai_session_id.as_str(),
        ],
        repo.path(),
    );
    assert_cli_success(
        &rebridge,
        "re-running bridge-run should remain idempotent for plan-step events",
    );
    let rebridge_plan_step_events = list_history_object_ids(repo.path(), "plan_step_event").await;
    assert_eq!(
        rebridge_plan_step_events.len(),
        real_plan_step_descriptions().len(),
        "bridge-run reruns should not duplicate derived plan_step_event objects"
    );

    let intent_event_values = futures::future::join_all(
        intent_events
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "intent_event", id)),
    )
    .await;
    let intent_statuses = intent_event_values
        .iter()
        .filter_map(|value| value["status"].as_str().or_else(|| value["kind"].as_str()))
        .collect::<BTreeSet<_>>();
    assert!(
        intent_statuses.contains("created") || intent_statuses.contains("analyzed"),
        "plan-aware path should still emit an initial intent lifecycle event"
    );
    assert!(
        !intent_statuses.contains("completed"),
        "without terminal decision, plan-aware path should not yet mark the intent completed"
    );

    let plan_snapshot: Value =
        read_tracked_object(repo.path(), "plan_snapshot", &plan_snapshots[0]).await;
    assert!(
        plan_snapshot["step_text"]
            .as_str()
            .is_some_and(|text| text.contains(&real_plan_step_descriptions()[0])),
        "plan snapshot should retain the numbered plan content"
    );
    assert_eq!(plan_snapshot["thread_id"], json!(ai_session_id));
    assert!(plan_snapshot["intent_id"].is_string());

    let plan_step_snapshot_values = futures::future::join_all(
        plan_step_snapshots
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "plan_step_snapshot", id)),
    )
    .await;
    let mut plan_step_ordinals = plan_step_snapshot_values
        .iter()
        .map(|value| {
            value["ordinal"]
                .as_i64()
                .expect("plan-step snapshot should contain ordinal")
        })
        .collect::<Vec<_>>();
    plan_step_ordinals.sort_unstable();
    assert_eq!(
        plan_step_ordinals,
        (0..real_plan_step_descriptions().len() as i64).collect::<Vec<_>>(),
        "plan-step snapshots should preserve step ordering"
    );

    let task_snapshot_values = futures::future::join_all(
        task_snapshots
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "task_snapshot", id)),
    )
    .await;
    let plan_step_snapshot_ids = plan_step_snapshots.iter().cloned().collect::<BTreeSet<_>>();
    assert!(
        task_snapshot_values.iter().all(|value| {
            value["origin_step_id"]
                .as_str()
                .is_some_and(|id| plan_step_snapshot_ids.contains(id))
                && value["thread_id"] == json!(ai_session_id)
                && value["plan_id"] == json!(plan_id.clone())
                && value["intent_id"].is_string()
        }),
        "each derived task snapshot should remain a projection linked to the formal plan"
    );

    let plan_step_event_values = futures::future::join_all(
        plan_step_events
            .iter()
            .map(|id| read_tracked_object::<Value>(repo.path(), "plan_step_event", id)),
    )
    .await;
    assert!(
        plan_step_event_values.iter().all(|value| {
            value["status"] == json!("pending")
                && value["step_id"]
                    .as_str()
                    .is_some_and(|id| plan_step_snapshot_ids.contains(id))
                && value["run_id"].is_string()
        }),
        "initial plan-step events should be emitted in pending state"
    );

    let run_snapshot: Value =
        read_tracked_object(repo.path(), "run_snapshot", &run_snapshots[0]).await;
    assert_eq!(run_snapshot["thread_id"], json!(ai_session_id));
    assert_eq!(run_snapshot["plan_id"], json!(plan_id.clone()));
    assert!(run_snapshot["task_id"].is_string());

    let provenance_snapshot: Value =
        read_tracked_object(repo.path(), "provenance_snapshot", &provenance_snapshots[0]).await;
    assert_eq!(provenance_snapshot["provider"], json!("claude"));
    assert!(provenance_snapshot["run_id"].is_string());
    assert!(provenance_snapshot["parameters"].is_object());

    assert_ai_type_matches(repo.path(), &intent_snapshots[0], "intent_snapshot");
    assert_ai_type_matches(repo.path(), &plan_snapshots[0], "plan_snapshot");
    assert_ai_type_matches(repo.path(), &plan_step_snapshots[0], "plan_step_snapshot");
    assert_ai_type_matches(repo.path(), &task_snapshots[0], "task_snapshot");
    assert_ai_type_matches(repo.path(), &plan_step_events[0], "plan_step_event");
    assert_ai_type_matches(repo.path(), &run_snapshots[0], "run_snapshot");
    assert_ai_type_matches(repo.path(), &provenance_snapshots[0], "provenance_snapshot");
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_materializes_plan_heads_when_plan_text_is_present() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn materialize_plan_heads() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&plan_task_only_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            PLAN_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &run,
        "claude-sdk run should succeed for plan-head rebuild test",
    );
    let run_json = parse_stdout_json(&run, "claude-sdk run plan-head rebuild");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let mut bridged_plan_id = None;

    for args in [
        vec![
            "claude-sdk".to_string(),
            "resolve-extraction".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "persist-intent".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
        vec![
            "claude-sdk".to_string(),
            "bridge-run".to_string(),
            "--ai-session-id".to_string(),
            ai_session_id.clone(),
        ],
    ] {
        let argv = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_libra_command(&argv, repo.path());
        assert_cli_success(&output, "plan-aware formal bridge command should succeed");
        if args[1] == "bridge-run" {
            let bridge_json = parse_stdout_json(&output, "plan-aware bridge-run output");
            bridged_plan_id = Some(
                bridge_json["planId"]
                    .as_str()
                    .expect("plan-aware bridge-run should emit planId")
                    .to_string(),
            );
        }
    }
    let plan_id = bridged_plan_id.expect("plan-aware bridge-run should capture a plan id");

    let (storage, history) = load_intent_history(repo.path()).await;
    let db_conn = libra::internal::db::establish_connection(
        repo.path()
            .join(".libra/libra.db")
            .to_str()
            .expect("db path should be valid UTF-8"),
    )
    .await
    .expect("failed to connect test database");

    let formal_plan: Plan = read_tracked_object(repo.path(), "plan", &plan_id).await;
    let formal_plan_steps = formal_plan
        .steps()
        .iter()
        .map(|step| step.description().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        formal_plan_steps,
        real_plan_step_descriptions(),
        "formal plan should preserve the numbered Claude plan steps from the real-artifact-shaped fixture"
    );

    let rebuilder = ProjectionRebuilder::new(storage.as_ref(), &history);
    let rebuild = rebuilder
        .materialize_latest_thread(&db_conn)
        .await
        .expect("materialize latest thread")
        .expect("projection rebuild should produce a thread");

    assert!(
        rebuild.scheduler.selected_plan_id.is_some(),
        "plan-aware bridge should surface a selected plan"
    );
    assert_eq!(
        rebuild.scheduler.current_plan_heads.len(),
        1,
        "plan-aware bridge should materialize one plan head"
    );

    let scheduler_row =
        ai_scheduler_state::Entity::find_by_id(rebuild.thread.thread_id.to_string())
            .one(&db_conn)
            .await
            .expect("query scheduler row")
            .expect("scheduler row should exist");
    assert_eq!(
        scheduler_row.selected_plan_id.as_deref(),
        rebuild
            .scheduler
            .selected_plan_id
            .as_ref()
            .map(Uuid::to_string)
            .as_deref()
    );

    let plan_head_rows = ai_scheduler_plan_head::Entity::find()
        .all(&db_conn)
        .await
        .expect("query scheduler plan heads");
    assert_eq!(
        plan_head_rows.len(),
        1,
        "plan-aware bridge should persist one scheduler plan head"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_without_intent_binding_creates_standalone_task() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn standalone_bridge() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let bridge = run_libra_command(
        &["claude-sdk", "bridge-run", "--ai-session-id", ai_session_id],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed without intent binding");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    assert!(
        bridge_json["intentId"].is_null(),
        "standalone bridge should not attach an intent id"
    );

    let task_id = bridge_json["taskId"]
        .as_str()
        .expect("taskId should be present");
    let task: Task = read_tracked_object(repo.path(), "task", task_id).await;
    assert!(
        task.intent().is_none(),
        "task should not be linked to an intent"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_rejects_missing_explicit_intent_binding() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn missing_intent_binding() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");
    let missing_binding = repo.path().join("missing-intent-binding.json");

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            ai_session_id,
            "--intent-binding",
            missing_binding.to_str().expect("binding path utf-8"),
        ],
        repo.path(),
    );
    assert!(
        !bridge.status.success(),
        "bridge-run should reject an explicit missing intent binding"
    );
    assert!(
        String::from_utf8_lossy(&bridge.stderr).contains("does not exist"),
        "error should explain that the requested binding path is missing"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_rejects_mismatched_existing_intent_link() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn delayed_intent_link() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let standalone_bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &standalone_bridge,
        "standalone bridge-run should succeed before intent persistence",
    );

    let resolve = run_libra_command(
        &[
            "claude-sdk",
            "resolve-extraction",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&resolve, "resolve-extraction should succeed");

    let persist_intent = run_libra_command(
        &[
            "claude-sdk",
            "persist-intent",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&persist_intent, "persist-intent should succeed");
    let persist_intent_json = parse_stdout_json(&persist_intent, "persist-intent output");
    let binding_path = persist_intent_json["bindingPath"]
        .as_str()
        .expect("bindingPath should be present");

    let bridge_with_intent = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
            "--intent-binding",
            binding_path,
        ],
        repo.path(),
    );
    assert!(
        !bridge_with_intent.status.success(),
        "bridge-run should reject reusing a standalone binding when a concrete intent is requested"
    );
    assert!(
        String::from_utf8_lossy(&bridge_with_intent.stderr)
            .contains("remove the stale binding to rebuild intentionally"),
        "error should explain how to recover from the stale standalone binding"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_rejects_invalid_binding_schema_on_reuse() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn invalid_binding_schema() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    let binding_path = PathBuf::from(
        bridge_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let mut binding_json: Value =
        serde_json::from_slice(&fs::read(&binding_path).expect("read formal run binding"))
            .expect("deserialize formal run binding");
    binding_json["schema"] = json!("libra.invalid_binding.v1");
    fs::write(
        &binding_path,
        serde_json::to_vec_pretty(&binding_json).expect("serialize invalid binding"),
    )
    .expect("write invalid binding");

    let bridge_repeat = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !bridge_repeat.status.success(),
        "bridge-run should reject a cached binding with the wrong schema"
    );
    assert!(
        String::from_utf8_lossy(&bridge_repeat.stderr)
            .contains("unsupported Claude formal run binding schema"),
        "error should name the invalid cached binding schema"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_bridge_run_reuses_binding_when_stored_audit_bundle_path_is_stale() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn missing_audit_bundle() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    let original_task_id = bridge_json["taskId"]
        .as_str()
        .expect("taskId should be present")
        .to_string();
    let original_run_id = bridge_json["runId"]
        .as_str()
        .expect("runId should be present")
        .to_string();
    let binding_path = PathBuf::from(
        bridge_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let mut binding_json: Value =
        serde_json::from_slice(&fs::read(&binding_path).expect("read formal run binding"))
            .expect("deserialize formal run binding");
    binding_json["auditBundlePath"] = json!(
        repo.path()
            .join(".libra")
            .join("audit-bundles")
            .join("missing.json")
            .to_string_lossy()
            .to_string()
    );
    fs::write(
        &binding_path,
        serde_json::to_vec_pretty(&binding_json).expect("serialize invalid binding"),
    )
    .expect("write invalid binding");

    let bridge_repeat = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &bridge_repeat,
        "bridge-run should keep reusing the binding when the stored audit bundle path is stale but the current bundle exists",
    );
    let bridge_repeat_json = parse_stdout_json(&bridge_repeat, "bridge-run repeat output");
    assert_eq!(bridge_repeat_json["taskId"], json!(original_task_id));
    assert_eq!(bridge_repeat_json["runId"], json!(original_run_id));
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_rebuilds_stale_evidence_and_decision_bindings() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn stale_binding_upgrade() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let original_evidence_ids = evidence_json["evidenceIds"]
        .as_array()
        .expect("evidenceIds should be an array")
        .iter()
        .map(|value| value.as_str().expect("evidence id").to_string())
        .collect::<Vec<_>>();
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&decision, "persist-decision should succeed");
    let decision_json = parse_stdout_json(&decision, "persist-decision output");
    let original_decision_id = decision_json["decisionId"]
        .as_str()
        .expect("decisionId should be present")
        .to_string();

    let mut stale_evidence_binding: Value =
        serde_json::from_slice(&fs::read(&evidence_binding_path).expect("read evidence binding"))
            .expect("deserialize evidence binding");
    stale_evidence_binding["evidenceIds"] = json!(
        original_evidence_ids
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
    );
    stale_evidence_binding["evidences"] = Value::Array(
        stale_evidence_binding["evidences"]
            .as_array()
            .expect("evidences should be an array")
            .iter()
            .take(3)
            .cloned()
            .collect(),
    );
    fs::write(
        &evidence_binding_path,
        serde_json::to_vec_pretty(&stale_evidence_binding)
            .expect("serialize stale evidence binding"),
    )
    .expect("write stale evidence binding");

    let evidence_rebuild = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &evidence_rebuild,
        "persist-evidence should rebuild a stale binding at the same path",
    );
    let evidence_rebuild_json =
        parse_stdout_json(&evidence_rebuild, "persist-evidence rebuild output");
    let rebuilt_evidence_ids = evidence_rebuild_json["evidenceIds"]
        .as_array()
        .expect("evidenceIds should be an array")
        .iter()
        .map(|value| value.as_str().expect("evidence id").to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        rebuilt_evidence_ids.len(),
        9,
        "rebuild should restore the full native/runtime evidence set"
    );
    assert_ne!(
        rebuilt_evidence_ids,
        original_evidence_ids
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>(),
        "persist-evidence should not reuse the stale subset binding"
    );

    let decision_rebuild = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &decision_rebuild,
        "persist-decision should rebuild when evidence binding content changes at the same path",
    );
    let decision_rebuild_json =
        parse_stdout_json(&decision_rebuild, "persist-decision rebuild output");
    let rebuilt_decision_id = decision_rebuild_json["decisionId"]
        .as_str()
        .expect("decisionId should be present")
        .to_string();
    assert_ne!(
        rebuilt_decision_id, original_decision_id,
        "persist-decision should not reuse a stale decision bound to old evidence ids"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_evidence_rebuilds_when_evidence_ids_are_stale() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn stale_evidence_ids() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let mut stale_binding: Value =
        serde_json::from_slice(&fs::read(&evidence_binding_path).expect("read evidence binding"))
            .expect("deserialize evidence binding");
    stale_binding["evidenceIds"] = json!(vec![
        "11111111-1111-7111-8111-111111111111",
        "22222222-2222-7222-8222-222222222222"
    ]);
    fs::write(
        &evidence_binding_path,
        serde_json::to_vec_pretty(&stale_binding).expect("serialize stale evidenceIds"),
    )
    .expect("write stale evidenceIds");

    let rebuilt = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &rebuilt,
        "persist-evidence should rebuild when evidenceIds diverge from evidence entries",
    );
    let rebuilt_json = parse_stdout_json(&rebuilt, "persist-evidence rebuild output");
    let rebuilt_ids = rebuilt_json["evidenceIds"]
        .as_array()
        .expect("evidenceIds should be an array")
        .iter()
        .map(|value| value.as_str().expect("evidence id").to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        rebuilt_ids.len(),
        9,
        "rebuild should restore the full evidence binding"
    );
    assert_ne!(
        rebuilt_ids,
        vec![
            "11111111-1111-7111-8111-111111111111".to_string(),
            "22222222-2222-7222-8222-222222222222".to_string()
        ],
        "persist-evidence should not reuse stale evidenceIds"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_bridge_requires_prior_bindings() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn missing_bindings() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present");

    let evidence_without_bridge = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !evidence_without_bridge.status.success(),
        "persist-evidence should reject missing bridge-run binding"
    );
    assert!(
        String::from_utf8_lossy(&evidence_without_bridge.stderr).contains("bridge-run"),
        "error should guide the user toward bridge-run first"
    );

    let bridge = run_libra_command(
        &["claude-sdk", "bridge-run", "--ai-session-id", ai_session_id],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let decision_without_evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !decision_without_evidence.status.success(),
        "persist-decision should reject missing evidence binding"
    );
    assert!(
        String::from_utf8_lossy(&decision_without_evidence.stderr).contains("persist-evidence"),
        "error should guide the user toward persist-evidence first"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_decision_rejects_mismatched_evidence_binding() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn mismatched_evidence_binding() {}\n")
        .expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");
    let bridge_json = parse_stdout_json(&bridge, "bridge-run output");
    let run_id = bridge_json["runId"]
        .as_str()
        .expect("runId should be present")
        .to_string();

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let mut binding_json: Value =
        serde_json::from_slice(&fs::read(&evidence_binding_path).expect("read evidence binding"))
            .expect("deserialize evidence binding");
    binding_json["runId"] = json!("11111111-1111-7111-8111-111111111111");
    fs::write(
        &evidence_binding_path,
        serde_json::to_vec_pretty(&binding_json).expect("serialize mismatched evidence binding"),
    )
    .expect("write mismatched evidence binding");

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !decision.status.success(),
        "persist-decision should reject an evidence binding that points to a different run"
    );
    assert!(
        String::from_utf8_lossy(&decision.stderr).contains(&format!(
            "Claude evidence binding belongs to run '11111111-1111-7111-8111-111111111111', not '{run_id}'"
        )),
        "error should explain the binding/run mismatch"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_decision_rejects_missing_evidence_objects() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn missing_evidence_objects() {}\n").expect("write source file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );

    let mut binding_json: Value =
        serde_json::from_slice(&fs::read(&evidence_binding_path).expect("read evidence binding"))
            .expect("deserialize evidence binding");
    binding_json["evidenceIds"] = json!(vec!["11111111-1111-7111-8111-111111111111".to_string()]);
    fs::write(
        &evidence_binding_path,
        serde_json::to_vec_pretty(&binding_json).expect("serialize missing evidenceIds binding"),
    )
    .expect("write missing evidenceIds binding");

    let decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert!(
        !decision.status.success(),
        "persist-decision should reject when evidenceIds no longer point at all persisted Evidence objects"
    );
    assert!(
        String::from_utf8_lossy(&decision.stderr)
            .contains("Claude evidence binding references missing Evidence objects"),
        "error should guide the user to rerun persist-evidence"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_evidence_redacts_repo_external_touch_hints() {
    let repo = tempdir().expect("failed to create repo root");
    let external = tempdir().expect("failed to create external root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = external.path().join("external.rs");
    fs::write(&touched_file, "pub fn external_touch() {}\n").expect("write external file");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&semantic_full_artifact(repo.path(), &touched_file))
            .expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    let tool_runtime_summary = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array")
        .iter()
        .find(|entry| entry["kind"] == json!("managed_tool_runtime_summary"))
        .and_then(|entry| entry["summary"].as_str())
        .expect("managed_tool_runtime_summary should exist");
    assert!(
        !tool_runtime_summary.contains(&touched_file.to_string_lossy().to_string()),
        "repo-external absolute touch hints should not be persisted into formal Evidence summaries"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_evidence_redacts_repo_external_relative_touch_hints() {
    let repo = tempdir().expect("failed to create repo root");
    let external = tempdir().expect("failed to create external root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn relative_escape_touch() {}\n").expect("write source file");

    let escaped_path = external.path().join("outside.rs");
    fs::write(&escaped_path, "pub fn escaped_touch() {}\n").expect("write external file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["hookEvents"][1]["input"]["tool_input"]["file_path"] = json!("../outside.rs");
    artifact["hookEvents"][2]["input"]["tool_input"]["file_path"] = json!("../outside.rs");
    artifact["hookEvents"][2]["input"]["tool_response"]["file"]["filePath"] =
        json!("../outside.rs");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    let tool_runtime_summary = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array")
        .iter()
        .find(|entry| entry["kind"] == json!("managed_tool_runtime_summary"))
        .and_then(|entry| entry["summary"].as_str())
        .expect("managed_tool_runtime_summary should exist");
    assert!(
        !tool_runtime_summary.contains("../outside.rs"),
        "repo-external relative touch hints should not be persisted into formal Evidence summaries"
    );
    assert!(
        !tool_runtime_summary.contains(&escaped_path.to_string_lossy().to_string()),
        "repo-external relative touch hints should not resolve into leaked absolute paths"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_evidence_redacts_windows_style_absolute_touch_hints() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn windows_absolute_touch() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    artifact["hookEvents"][1]["input"]["tool_input"]["file_path"] = json!("C:/external/outside.rs");
    artifact["hookEvents"][2]["input"]["tool_input"]["file_path"] = json!("C:/external/outside.rs");
    artifact["hookEvents"][2]["input"]["tool_response"]["file"]["filePath"] =
        json!("C:/external/outside.rs");

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    let tool_runtime_summary = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array")
        .iter()
        .find(|entry| entry["kind"] == json!("managed_tool_runtime_summary"))
        .and_then(|entry| entry["summary"].as_str())
        .expect("managed_tool_runtime_summary should exist");
    assert!(
        !tool_runtime_summary.contains("C:/external/outside.rs"),
        "Windows-style absolute touch hints should not be persisted into formal Evidence summaries"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_evidence_preserves_relative_touch_hints_for_windows_bundle() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn windows_bundle_relative_touch() {}\n")
        .expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    replace_template_slots(
        &mut artifact,
        &[
            (
                repo.path().to_string_lossy().as_ref(),
                json!("C:/workspace/libra"),
            ),
            (touched_file.to_string_lossy().as_ref(), json!("src/lib.rs")),
        ],
    );

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    let tool_runtime_summary = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array")
        .iter()
        .find(|entry| entry["kind"] == json!("managed_tool_runtime_summary"))
        .and_then(|entry| entry["summary"].as_str())
        .expect("managed_tool_runtime_summary should exist");
    assert!(
        tool_runtime_summary.contains("src/lib.rs"),
        "relative touch hints from Windows-origin bundles should still be preserved"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_evidence_preserves_relative_touch_hints_for_windows_drive_root_bundle()
 {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn windows_drive_root_touch() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    replace_template_slots(
        &mut artifact,
        &[
            (repo.path().to_string_lossy().as_ref(), json!("C:/")),
            (touched_file.to_string_lossy().as_ref(), json!("src/lib.rs")),
        ],
    );

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    let tool_runtime_summary = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array")
        .iter()
        .find(|entry| entry["kind"] == json!("managed_tool_runtime_summary"))
        .and_then(|entry| entry["summary"].as_str())
        .expect("managed_tool_runtime_summary should exist");
    assert!(
        tool_runtime_summary.contains("src/lib.rs"),
        "relative touch hints from Windows drive-root bundles should still be preserved"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_persist_evidence_preserves_mixed_case_windows_absolute_touch_hints() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let touched_file = repo.path().join("src").join("lib.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn mixed_case_windows_touch() {}\n").expect("write source file");

    let mut artifact = semantic_full_artifact(repo.path(), &touched_file);
    replace_template_slots(
        &mut artifact,
        &[
            (
                repo.path().to_string_lossy().as_ref(),
                json!("C:/Workspace/Libra"),
            ),
            (
                touched_file.to_string_lossy().as_ref(),
                json!("c:/workspace/libra/src/lib.rs"),
            ),
        ],
    );

    let artifact_path = repo.path().join("managed-run-artifact.json");
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).expect("serialize test artifact"),
    )
    .expect("write test artifact");
    let request_path = repo.path().join("helper-request.json");
    let helper_path = repo.path().join("capture-managed-helper.sh");
    write_request_capture_shell_helper(&helper_path, &artifact_path, &request_path);

    let run = run_libra_command(
        &[
            "claude-sdk",
            "run",
            "--prompt",
            DEFAULT_MANAGED_PROMPT,
            "--helper-path",
            helper_path.to_str().expect("helper path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&run, "claude-sdk run should succeed");
    let run_json = parse_stdout_json(&run, "claude-sdk run output");
    let ai_session_id = run_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();
    let provider_session_id = run_json["providerSessionId"]
        .as_str()
        .expect("providerSessionId should be present");
    stage_provider_session_evidence_artifacts(repo.path(), provider_session_id);

    let bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&bridge, "bridge-run should succeed");

    let evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(&evidence, "persist-evidence should succeed");
    let evidence_json = parse_stdout_json(&evidence, "persist-evidence output");
    let evidence_binding_path = PathBuf::from(
        evidence_json["bindingPath"]
            .as_str()
            .expect("bindingPath should be present"),
    );
    let evidence_binding = read_json_file(&evidence_binding_path);
    let tool_runtime_summary = evidence_binding["evidences"]
        .as_array()
        .expect("evidences should be an array")
        .iter()
        .find(|entry| entry["kind"] == json!("managed_tool_runtime_summary"))
        .and_then(|entry| entry["summary"].as_str())
        .expect("managed_tool_runtime_summary should exist");
    assert!(
        tool_runtime_summary.contains("src/lib.rs"),
        "mixed-case Windows absolute touch hints inside the repo should still collapse to repo-relative paths"
    );
}

#[tokio::test]
#[serial]
async fn test_claude_sdk_formal_decision_maps_invalid_and_timeout_cases() {
    let repo = tempdir().expect("failed to create repo root");
    test::setup_with_new_libra_in(repo.path()).await;

    let invalid_artifact_path = repo.path().join("probe-like-artifact.json");
    fs::write(&invalid_artifact_path, PROBE_LIKE_ARTIFACT).expect("write invalid artifact");
    let invalid_import = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            invalid_artifact_path
                .to_str()
                .expect("invalid artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_import,
        "claude-sdk import should succeed for invalid extraction",
    );
    let invalid_import_json = parse_stdout_json(&invalid_import, "invalid import output");
    let invalid_ai_session_id = invalid_import_json["aiSessionId"]
        .as_str()
        .expect("aiSessionId should be present")
        .to_string();

    let invalid_bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &invalid_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_bridge,
        "bridge-run should succeed for invalid extraction",
    );
    let invalid_evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &invalid_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_evidence,
        "persist-evidence should succeed for invalid extraction",
    );
    let invalid_decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &invalid_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &invalid_decision,
        "persist-decision should succeed for invalid extraction",
    );
    let invalid_decision_json =
        parse_stdout_json(&invalid_decision, "invalid persist-decision output");
    assert_eq!(invalid_decision_json["decisionType"], json!("abandon"));

    let touched_file = repo.path().join("src").join("timed.rs");
    fs::create_dir_all(touched_file.parent().expect("source file parent")).expect("mkdir src");
    fs::write(&touched_file, "pub fn timed_out() {}\n").expect("write timed source");
    let timed_artifact_path = repo.path().join("timed-artifact.json");
    fs::write(
        &timed_artifact_path,
        serde_json::to_vec_pretty(&timed_out_partial_artifact(repo.path(), &touched_file))
            .expect("serialize timed artifact"),
    )
    .expect("write timed artifact");

    let timed_import = run_libra_command(
        &[
            "claude-sdk",
            "import",
            "--artifact",
            timed_artifact_path
                .to_str()
                .expect("timed artifact path utf-8"),
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_import,
        "claude-sdk import should succeed for timed artifact",
    );
    let timed_import_json = parse_stdout_json(&timed_import, "timed import output");
    let timed_ai_session_id = timed_import_json["aiSessionId"]
        .as_str()
        .expect("timed aiSessionId should be present")
        .to_string();

    let timed_bridge = run_libra_command(
        &[
            "claude-sdk",
            "bridge-run",
            "--ai-session-id",
            &timed_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_bridge,
        "bridge-run should succeed for timed artifact",
    );
    let timed_evidence = run_libra_command(
        &[
            "claude-sdk",
            "persist-evidence",
            "--ai-session-id",
            &timed_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_evidence,
        "persist-evidence should succeed for timed artifact",
    );
    let timed_decision = run_libra_command(
        &[
            "claude-sdk",
            "persist-decision",
            "--ai-session-id",
            &timed_ai_session_id,
        ],
        repo.path(),
    );
    assert_cli_success(
        &timed_decision,
        "persist-decision should succeed for timed artifact",
    );
    let timed_decision_json = parse_stdout_json(&timed_decision, "timed persist-decision output");
    assert_eq!(timed_decision_json["decisionType"], json!("retry"));
}
