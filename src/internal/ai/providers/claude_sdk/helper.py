#!/usr/bin/env python3

import asyncio
import json
import os
import sys
from dataclasses import is_dataclass
from typing import Any

CLAUDE_AGENT_SDK_IMPORT_ERROR: Exception | None = None

try:
    from claude_agent_sdk import ClaudeSDKClient, get_session_messages, list_sessions, query
    from claude_agent_sdk.types import (
        AssistantMessage,
        ClaudeAgentOptions,
        HookMatcher,
        PermissionResultAllow,
        PermissionResultDeny,
        PermissionUpdate,
        ResultMessage,
        StreamEvent,
        SystemMessage,
        TextBlock,
        ThinkingBlock,
        ToolResultBlock,
        ToolUseBlock,
    )
except Exception as exc:  # pragma: no cover - runtime import guard
    CLAUDE_AGENT_SDK_IMPORT_ERROR = exc
    ClaudeSDKClient = None
    get_session_messages = None
    list_sessions = None
    query = None


def read_stdin() -> str:
    return sys.stdin.read()


def ensure_claude_agent_sdk_available() -> None:
    if CLAUDE_AGENT_SDK_IMPORT_ERROR is None:
        return
    raise RuntimeError(
        "failed to import claude_agent_sdk from the selected Python environment; "
        "install it with '<python> -m pip install claude-agent-sdk'"
    ) from CLAUDE_AGENT_SDK_IMPORT_ERROR


def collect_provider_env() -> dict[str, str]:
    """Collect provider-facing environment and validate common auth mistakes."""
    env = dict(os.environ)
    base_url = env.get("ANTHROPIC_BASE_URL", "").strip()
    api_key = env.get("ANTHROPIC_API_KEY", "").strip()
    auth_token = env.get("ANTHROPIC_AUTH_TOKEN", "").strip()

    if not api_key and not auth_token:
        raise RuntimeError(
            "missing Anthropic credentials; set ANTHROPIC_AUTH_TOKEN or ANTHROPIC_API_KEY "
            "before running claude-sdk"
        )

    if base_url and api_key and not auth_token:
        sys.stderr.write(
            "warning: ANTHROPIC_BASE_URL is set but ANTHROPIC_AUTH_TOKEN is not. "
            "Some third-party gateways accept raw /v1/messages with x-api-key but reject "
            "Claude Code / Python SDK traffic unless Authorization bearer auth is configured.\n"
        )
        sys.stderr.flush()

    return env


def stable_normalize(value: Any) -> Any:
    if isinstance(value, list):
        return [stable_normalize(item) for item in value]
    if isinstance(value, dict):
        normalized = {}
        for key in sorted(value):
            if value[key] is not None:
                normalized[key] = stable_normalize(value[key])
        return normalized
    return value


def stable_stringify(value: Any) -> str:
    return json.dumps(stable_normalize(value), separators=(",", ":"), sort_keys=True)


def should_use_stream_mode(request: dict[str, Any]) -> bool:
    return request.get("mode") == "queryStream" or request.get("stream") is True


def emit_ndjson_event(enabled: bool, event_type: str, payload: dict[str, Any] | None = None) -> None:
    if not enabled:
        return
    event = {"event": event_type}
    if payload:
        event.update(payload)
    sys.stdout.write(json.dumps(event) + "\n")
    sys.stdout.flush()


def find_last_result_message(messages: list[dict[str, Any]]) -> dict[str, Any] | None:
    for message in reversed(messages):
        if message.get("type") == "result":
            return message
    return None


def build_runtime_snapshot(
    hook_events: list[dict[str, Any]],
    messages: list[dict[str, Any]],
    helper_timed_out: bool,
    helper_error: str | None,
) -> dict[str, Any]:
    last_message = messages[-1] if messages else None
    return {
        "hookEventCount": len(hook_events),
        "messageCount": len(messages),
        "helperTimedOut": helper_timed_out,
        "helperError": helper_error,
        "lastMessageType": last_message.get("type") if last_message else None,
        "lastMessageSubtype": last_message.get("subtype") if last_message else None,
        "hasResultMessage": any(message.get("type") == "result" for message in messages),
    }


def build_artifact(
    request: dict[str, Any],
    hook_events: list[dict[str, Any]],
    messages: list[dict[str, Any]],
    helper_timed_out: bool,
    helper_error: str | None,
) -> dict[str, Any]:
    return {
        "cwd": request.get("cwd"),
        "prompt": request.get("prompt"),
        "requestContext": {
            "enableFileCheckpointing": request.get("enableFileCheckpointing") is True,
            "interactiveApprovals": request.get("interactiveApprovals") is True,
            "continue": request.get("continue") is True,
            "resume": request.get("resume")
            if isinstance(request.get("resume"), str)
            else None,
            "forkSession": request.get("forkSession") is True,
            "sessionId": request.get("sessionId")
            if isinstance(request.get("sessionId"), str)
            else None,
            "resumeSessionAt": request.get("resumeSessionAt")
            if isinstance(request.get("resumeSessionAt"), str)
            else None,
        },
        "helperTimedOut": helper_timed_out,
        "helperError": helper_error,
        "hookEvents": hook_events,
        "messages": messages,
        "resultMessage": find_last_result_message(messages),
    }


def extract_assistant_delta(message: dict[str, Any]) -> str | None:
    if message.get("type") != "stream_event":
        return None
    event = message.get("event")
    if not isinstance(event, dict) or event.get("type") != "content_block_delta":
        return None
    delta = event.get("delta")
    if not isinstance(delta, dict) or delta.get("type") != "text_delta":
        return None
    text = delta.get("text")
    return text if isinstance(text, str) else None


def load_scripted_responses() -> list[dict[str, Any]]:
    raw = os.environ.get("LIBRA_CLAUDE_HELPER_SCRIPTED_RESPONSES")
    if not raw:
        return []
    parsed = json.loads(raw)
    if not isinstance(parsed, list):
        raise RuntimeError("LIBRA_CLAUDE_HELPER_SCRIPTED_RESPONSES must be a JSON array")
    return list(parsed)


def interactive_terminal_hint() -> str:
    if os.name == "posix":
        return "rerun in a terminal session (with /dev/tty available) or unset --interactive-approvals"
    return "rerun in a terminal session or unset --interactive-approvals"


def get_interactive_streams() -> tuple[Any, Any, bool] | None:
    if os.name == "posix":
        try:
            reader = open("/dev/tty", "r", encoding="utf-8")
            writer = open("/dev/tty", "w", encoding="utf-8")
            return reader, writer, True
        except OSError:
            pass

    stdin_isatty = getattr(sys.stdin, "isatty", lambda: False)()
    stdout_isatty = getattr(sys.stdout, "isatty", lambda: False)()
    stderr_isatty = getattr(sys.stderr, "isatty", lambda: False)()

    if stdin_isatty and stdout_isatty:
        return sys.stdin, sys.stdout, False
    if stdin_isatty and stderr_isatty:
        return sys.stdin, sys.stderr, False
    return None


def has_interactive_tty() -> bool:
    return get_interactive_streams() is not None


def assert_interactive_input_available(state: dict[str, Any]) -> None:
    if state["scripted_responses"]:
        return
    if not has_interactive_tty():
        raise RuntimeError(
            f"interactive approvals require an interactive terminal; {interactive_terminal_hint()}"
        )


def prompt_via_tty(lines: list[str], question: str) -> str:
    streams = get_interactive_streams()
    if streams is None:
        raise RuntimeError(
            f"interactive approvals require an interactive terminal; {interactive_terminal_hint()}"
        )

    reader, writer, should_close = streams
    try:
        for line in lines:
            writer.write(f"{line}\n")
        writer.write(question)
        writer.flush()
        answer = reader.readline()
        return answer.strip() if isinstance(answer, str) else ""
    finally:
        if should_close:
            try:
                reader.close()
            except Exception:
                pass
            try:
                writer.close()
            except Exception:
                pass


async def prompt_via_tty_async(lines: list[str], question: str) -> str:
    return await asyncio.to_thread(prompt_via_tty, lines, question)


def build_tool_approval_lines(tool_name: str, tool_input: dict[str, Any], suggestions: list[Any]) -> list[str]:
    lines = ["", "Tool approval required", f"Tool: {tool_name}"]
    if tool_name == "Bash":
        command = tool_input.get("command")
        description = tool_input.get("description")
        if isinstance(command, str):
            lines.append(f"Command: {command}")
        if isinstance(description, str):
            lines.append(f"Description: {description}")
    else:
        lines.append(f"Input: {json.dumps(tool_input, indent=2, sort_keys=True)}")
    if has_accept_edits_suggestion(suggestions):
        lines.append("Claude suggested switching this session to acceptEdits.")
    return lines


def parse_tool_approval_decision(answer: str, session_upgrade_available: bool) -> str | None:
    normalized = answer.strip().lower()
    if normalized in {"a", "allow", "approve"}:
        return "approve"
    if normalized in {
        "s",
        "session",
        "switch",
        "switch-session",
        "switch_session",
        "switch-to-acceptedits",
        "switch_to_acceptedits",
        "approve-for-session",
        "approve_for_session",
    }:
        return "switch_session" if session_upgrade_available else "approve_for_session"
    if normalized in {"d", "deny"}:
        return "deny"
    if normalized in {"b", "abort"}:
        return "abort"
    return None


def parse_question_response(response: str, options: list[dict[str, Any]], multi_select: bool) -> str:
    normalized = response.strip()
    if not normalized:
        return ""
    tokens = normalized.split(",") if multi_select else [normalized]
    labels = []
    for token in tokens:
        try:
            index = int(token.strip()) - 1
        except ValueError:
            continue
        if 0 <= index < len(options):
            label = options[index].get("label")
            if isinstance(label, str):
                labels.append(label)
    if labels:
        return ", ".join(labels) if multi_select else labels[0]
    return normalized


def build_approval_cache_key(tool_name: str, tool_input: dict[str, Any]) -> str:
    return stable_stringify({"toolName": tool_name, "toolInput": tool_input})


def build_hook_input(
    tool_name: str,
    tool_input: dict[str, Any],
    suggestions: list[Any],
    context: Any | None = None,
    extras: dict[str, Any] | None = None,
) -> dict[str, Any]:
    payload = {
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_use_id": getattr(context, "toolUseID", None),
        "agent_id": getattr(context, "agentID", None),
        "title": getattr(context, "title", None),
        "display_name": getattr(context, "displayName", None),
        "description": getattr(context, "description", None),
        "blocked_path": getattr(context, "blockedPath", None),
        "decision_reason": getattr(context, "decisionReason", None),
        "suggestions": convert_jsonable(suggestions),
    }
    if extras:
        payload.update(extras)
    return payload


def has_accept_edits_suggestion(suggestions: list[Any]) -> bool:
    for suggestion in suggestions:
        candidate = convert_jsonable(suggestion)
        if (
            isinstance(candidate, dict)
            and candidate.get("type") == "setMode"
            and candidate.get("destination") == "session"
            and candidate.get("mode") == "acceptEdits"
        ):
            return True
    return False


async def next_tool_approval_decision(
    state: dict[str, Any],
    tool_name: str,
    tool_input: dict[str, Any],
    suggestions: list[Any],
) -> dict[str, str]:
    session_upgrade_available = has_accept_edits_suggestion(suggestions)
    if state["scripted_responses"]:
        scripted = state["scripted_responses"].pop(0)
        if not isinstance(scripted, dict) or scripted.get("kind") != "tool_approval":
            raise RuntimeError("expected scripted tool_approval response")
        decision = scripted.get("decision")
        if decision == "switch_session" and not session_upgrade_available:
            decision = "approve_for_session"
        if not isinstance(decision, str):
            raise RuntimeError("scripted tool_approval response is missing a decision")
        return {"decision": decision, "prompt_source": "scripted"}

    while True:
        prompt = (
            "Choice [a]llow once/[s]witch session/[d]eny/a[b]ort: "
            if session_upgrade_available
            else "Choice [a]llow/[s]ession/[d]eny/a[b]ort: "
        )
        answer = await prompt_via_tty_async(
            build_tool_approval_lines(tool_name, tool_input, suggestions),
            prompt,
        )
        decision = parse_tool_approval_decision(answer, session_upgrade_available)
        if decision is not None:
            return {"decision": decision, "prompt_source": "interactive_tty"}


async def collect_ask_user_question_answers(
    state: dict[str, Any], tool_input: dict[str, Any]
) -> dict[str, Any]:
    if state["scripted_responses"]:
        scripted = state["scripted_responses"].pop(0)
        if not isinstance(scripted, dict) or scripted.get("kind") != "ask_user_question":
            raise RuntimeError("expected scripted ask_user_question response")
        answers = scripted.get("answers")
        if not isinstance(answers, dict):
            answers = {}
        return {"answers": answers, "prompt_source": "scripted"}

    answers: dict[str, str] = {}
    questions = tool_input.get("questions")
    if not isinstance(questions, list):
        questions = []
    for index, question in enumerate(questions):
        if not isinstance(question, dict):
            continue
        options = question.get("options")
        if not isinstance(options, list):
            options = []
        key = question.get("question")
        if not isinstance(key, str) or not key.strip():
            key = f"question_{index + 1}"
        lines = ["", "Agent question"]
        header = question.get("header")
        if isinstance(header, str) and isinstance(question.get("question"), str):
            lines.append(f"{header}: {question['question']}")
        else:
            lines.append(key)
        for option_index, option in enumerate(options):
            if not isinstance(option, dict):
                continue
            label = option.get("label", "")
            description = option.get("description")
            suffix = f" - {description}" if isinstance(description, str) else ""
            lines.append(f"  {option_index + 1}. {label}{suffix}")
        lines.append(
            "  (Enter numbers separated by commas, or type your own answer)"
            if question.get("multiSelect") is True
            else "  (Enter a number, or type your own answer)"
        )
        response = await prompt_via_tty_async(lines, "Your choice: ")
        answers[key] = parse_question_response(
            response, options, question.get("multiSelect") is True
        )
    return {"answers": answers, "prompt_source": "interactive_tty"}


def convert_content_block(block: Any) -> dict[str, Any]:
    if isinstance(block, TextBlock):
        return {"type": "text", "text": block.text}
    if isinstance(block, ThinkingBlock):
        return {
            "type": "thinking",
            "thinking": block.thinking,
            "signature": block.signature,
        }
    if isinstance(block, ToolUseBlock):
        return {"type": "tool_use", "id": block.id, "name": block.name, "input": block.input}
    if isinstance(block, ToolResultBlock):
        return {
            "type": "tool_result",
            "tool_use_id": block.tool_use_id,
            "content": convert_jsonable(block.content),
            "is_error": block.is_error,
        }
    return convert_jsonable(block)


def convert_jsonable(value: Any) -> Any:
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if hasattr(value, "to_dict") and callable(value.to_dict):
        return convert_jsonable(value.to_dict())
    if is_dataclass(value):
        return {
            key: convert_jsonable(getattr(value, key))
            for key in value.__dataclass_fields__
            if getattr(value, key) is not None
        }
    if isinstance(value, dict):
        return {str(key): convert_jsonable(item) for key, item in value.items() if item is not None}
    if isinstance(value, (list, tuple)):
        return [convert_jsonable(item) for item in value]
    return str(value)


def convert_session_info(session: Any) -> dict[str, Any]:
    return {
        "sessionId": getattr(session, "session_id", ""),
        "summary": getattr(session, "summary", ""),
        "lastModified": getattr(session, "last_modified", 0),
        "fileSize": getattr(session, "file_size", None),
        "customTitle": getattr(session, "custom_title", None),
        "firstPrompt": getattr(session, "first_prompt", None),
        "gitBranch": getattr(session, "git_branch", None),
        "cwd": getattr(session, "cwd", None),
        "tag": getattr(session, "tag", None),
        "createdAt": getattr(session, "created_at", None),
    }


def convert_session_message(message: Any) -> dict[str, Any]:
    return {
        "type": getattr(message, "type", ""),
        "uuid": getattr(message, "uuid", ""),
        "session_id": getattr(message, "session_id", ""),
        "message": convert_jsonable(getattr(message, "message", None)),
        "parent_tool_use_id": getattr(message, "parent_tool_use_id", None),
    }


def convert_sdk_message(message: Any, state: dict[str, Any]) -> dict[str, Any]:
    if isinstance(message, SystemMessage):
        payload = {"type": "system", "subtype": message.subtype}
        payload.update(convert_jsonable(message.data))
        session_id = payload.get("session_id")
        if isinstance(session_id, str) and session_id:
            state["session_id"] = session_id
        model = payload.get("model")
        if isinstance(model, str) and model:
            state["model"] = model
        return payload

    if isinstance(message, AssistantMessage):
        content = [convert_content_block(block) for block in message.content]
        payload = {
            "type": "assistant",
            "session_id": state.get("session_id"),
            "model": message.model,
            "message": {"role": "assistant", "content": content},
        }
        if message.parent_tool_use_id is not None:
            payload["parent_tool_use_id"] = message.parent_tool_use_id
        if message.error is not None:
            payload["error"] = message.error
        if message.usage is not None:
            payload["usage"] = convert_jsonable(message.usage)
        return payload

    if isinstance(message, ResultMessage):
        state["session_id"] = message.session_id
        return {
            "type": "result",
            "subtype": getattr(message, "subtype", None),
            "duration_ms": getattr(message, "duration_ms", None),
            "duration_api_ms": getattr(message, "duration_api_ms", None),
            "is_error": getattr(message, "is_error", None),
            "num_turns": getattr(message, "num_turns", None),
            "session_id": getattr(message, "session_id", None),
            "stop_reason": getattr(message, "stop_reason", None),
            "total_cost_usd": getattr(message, "total_cost_usd", None),
            "usage": convert_jsonable(getattr(message, "usage", None)),
            "modelUsage": convert_jsonable(getattr(message, "model_usage", None)),
            "permission_denials": convert_jsonable(
                getattr(message, "permission_denials", None)
            ),
            "result": getattr(message, "result", None),
            "structured_output": convert_jsonable(
                getattr(message, "structured_output", None)
            ),
            "fast_mode_state": convert_jsonable(
                getattr(message, "fast_mode_state", None)
            ),
            "uuid": getattr(message, "uuid", None),
        }

    if isinstance(message, StreamEvent):
        state["session_id"] = message.session_id
        payload = {
            "type": "stream_event",
            "uuid": message.uuid,
            "session_id": message.session_id,
            "event": convert_jsonable(message.event),
        }
        if message.parent_tool_use_id is not None:
            payload["parent_tool_use_id"] = message.parent_tool_use_id
        return payload

    converted = convert_jsonable(message)
    if isinstance(converted, dict):
        if "session_id" in converted and isinstance(converted["session_id"], str):
            state["session_id"] = converted["session_id"]
        return converted
    return {"type": "unknown", "value": converted}


def build_hooks(
    hook_events: list[dict[str, Any]],
    emit_event,
    emit_snapshot,
):
    async def record_hook(hook_input: dict[str, Any], _tool_use_id: str | None, _context: Any):
        hook_name = hook_input.get("hook_event_name", "unknown")
        normalized = convert_jsonable(hook_input)
        hook_events.append({"hook": hook_name, "input": normalized})
        if hook_name == "PermissionRequest":
            emit_event("permission_request", {"hook": hook_name, "input": normalized})
        elif hook_name == "PreToolUse":
            emit_event("tool_call", {"hook": hook_name, "input": normalized})
        elif hook_name in {"PostToolUse", "PostToolUseFailure"}:
            emit_event("tool_result", {"hook": hook_name, "input": normalized})
        emit_snapshot()
        return {"continue": True}

    supported_hooks = [
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "UserPromptSubmit",
        "Stop",
        "SubagentStop",
        "PreCompact",
        "Notification",
        "SubagentStart",
        "PermissionRequest",
    ]
    return {hook_name: [HookMatcher(hooks=[record_hook])] for hook_name in supported_hooks}


async def single_prompt_stream(prompt: str):
    yield {
        "type": "user",
        "session_id": "",
        "message": {"role": "user", "content": prompt},
        "parent_tool_use_id": None,
    }


def build_extra_args(request: dict[str, Any]) -> dict[str, str | None]:
    extra_args: dict[str, str | None] = {}
    session_id = request.get("sessionId")
    if isinstance(session_id, str) and session_id:
        extra_args["session-id"] = session_id
    resume_session_at = request.get("resumeSessionAt")
    if isinstance(resume_session_at, str) and resume_session_at:
        extra_args["resume-session-at"] = resume_session_at
    if request.get("promptSuggestions") is True:
        extra_args["prompt-suggestions"] = None
    if request.get("agentProgressSummaries") is True:
        extra_args["agent-progress-summaries"] = None
    return extra_args


def execute_list_sessions(request: dict[str, Any]) -> None:
    ensure_claude_agent_sdk_available()
    sessions = list_sessions(
        directory=request.get("cwd"),
        include_worktrees=request.get("includeWorktrees") is True,
    )
    offset = request.get("offset", 0)
    limit = request.get("limit")
    if not isinstance(offset, int) or offset < 0:
        offset = 0
    if not isinstance(limit, int) or limit <= 0:
        limit = None
    sliced = sessions[offset:]
    if limit is not None:
        sliced = sliced[:limit]
    sys.stdout.write(json.dumps([convert_session_info(session) for session in sliced]))
    sys.stdout.flush()


def execute_get_session_messages(request: dict[str, Any]) -> None:
    ensure_claude_agent_sdk_available()
    provider_session_id = request.get("providerSessionId")
    if not isinstance(provider_session_id, str) or not provider_session_id.strip():
        raise RuntimeError("providerSessionId is required for getSessionMessages")
    messages = get_session_messages(
        provider_session_id,
        directory=request.get("cwd"),
        limit=request.get("limit"),
        offset=request.get("offset", 0),
    )
    sys.stdout.write(json.dumps([convert_session_message(message) for message in messages]))
    sys.stdout.flush()


async def execute_query(request: dict[str, Any]) -> None:
    ensure_claude_agent_sdk_available()
    mode = request.get("mode")
    if mode not in {"query", "queryStream"}:
        raise RuntimeError(
            f"python helper supports only query/queryStream, got {mode!r}"
        )

    stream_mode = should_use_stream_mode(request)
    hook_events: list[dict[str, Any]] = []
    messages: list[dict[str, Any]] = []
    helper_timed_out = False
    helper_error: str | None = None
    state = {
        "scripted_responses": load_scripted_responses(),
        "approved_tool_cache_keys": set(),
        "session_permission_mode": request.get("permissionMode") or "default",
        "session_id": None,
        "model": None,
    }

    def emit_snapshot() -> None:
        emit_ndjson_event(
            stream_mode,
            "runtime_snapshot",
            {
                "snapshot": build_runtime_snapshot(
                    hook_events, messages, helper_timed_out, helper_error
                ),
                "artifact": build_artifact(
                    request, hook_events, messages, helper_timed_out, helper_error
                ),
            },
        )

    env = collect_provider_env()

    options = ClaudeAgentOptions(
        cwd=request.get("cwd"),
        model=request.get("model"),
        permission_mode=request.get("permissionMode"),
        continue_conversation=request.get("continue") is True,
        resume=request.get("resume") if isinstance(request.get("resume"), str) else None,
        allowed_tools=(
            list(request.get("allowedTools", []))
            if isinstance(request.get("allowedTools"), list)
            else []
        ),
        include_partial_messages=request.get("includePartialMessages") is True,
        fork_session=request.get("forkSession") is True,
        setting_sources=["user"],
        env=env,
        extra_args=build_extra_args(request),
        hooks=build_hooks(
            hook_events,
            lambda event_type, payload: emit_ndjson_event(stream_mode, event_type, payload),
            emit_snapshot,
        ),
        enable_file_checkpointing=request.get("enableFileCheckpointing") is True,
    )

    tools = request.get("tools")
    if isinstance(tools, list):
        options.tools = list(tools)

    system_prompt = request.get("systemPrompt")
    if system_prompt is not None:
        options.system_prompt = system_prompt

    output_schema = request.get("outputSchema")
    if output_schema is not None:
        options.output_format = {"type": "json_schema", "schema": output_schema}

    if tools and request.get("interactiveApprovals") is True:
        assert_interactive_input_available(state)

        async def can_use_tool(tool_name: str, tool_input: dict[str, Any], context: Any):
            suggestions = getattr(context, "suggestions", []) or []
            if tool_name == "AskUserQuestion":
                emit_ndjson_event(
                    stream_mode, "ask_user_question", {"toolName": tool_name, "input": tool_input}
                )
                response = await collect_ask_user_question_answers(state, tool_input)
                hook_events.append(
                    {
                        "hook": "CanUseTool",
                        "input": build_hook_input(
                            tool_name,
                            tool_input,
                            suggestions,
                            context,
                            {
                                "interaction_kind": "ask_user_question",
                                "prompt_source": response["prompt_source"],
                                "question_count": len(tool_input.get("questions", []))
                                if isinstance(tool_input.get("questions"), list)
                                else 0,
                                "answer_count": len(response["answers"]),
                                "answers": response["answers"],
                            },
                        ),
                    }
                )
                emit_snapshot()
                return PermissionResultAllow(
                    updated_input={**tool_input, "answers": response["answers"]}
                )

            cache_key = build_approval_cache_key(tool_name, tool_input)
            if cache_key in state["approved_tool_cache_keys"]:
                hook_events.append(
                    {
                        "hook": "CanUseTool",
                        "input": build_hook_input(
                            tool_name,
                            tool_input,
                            suggestions,
                            context,
                            {
                                "interaction_kind": "tool_approval",
                                "approval_decision": "allow",
                                "approval_scope": "session",
                                "prompt_source": "session_cache",
                                "cached": True,
                            },
                        ),
                    }
                )
                emit_snapshot()
                return PermissionResultAllow(updated_input=tool_input)

            response = await next_tool_approval_decision(
                state, tool_name, tool_input, suggestions
            )
            decision = response["decision"]
            prompt_source = response["prompt_source"]

            if decision == "switch_session":
                previous_mode = state["session_permission_mode"]
                state["session_permission_mode"] = "acceptEdits"
                hook_events.append(
                    {
                        "hook": "PermissionModeChanged",
                        "input": {
                            "previous_mode": previous_mode,
                            "mode": "acceptEdits",
                            "source": prompt_source,
                            "tool_name": tool_name,
                            "tool_input": tool_input,
                        },
                    }
                )
                emit_ndjson_event(
                    stream_mode,
                    "permission_mode_changed",
                    {
                        "previousMode": previous_mode,
                        "mode": "acceptEdits",
                        "source": prompt_source,
                        "toolName": tool_name,
                    },
                )
                hook_events.append(
                    {
                        "hook": "CanUseTool",
                        "input": build_hook_input(
                            tool_name,
                            tool_input,
                            suggestions,
                            context,
                            {
                                "interaction_kind": "tool_approval",
                                "approval_decision": "allow",
                                "approval_scope": "session_mode",
                                "prompt_source": prompt_source,
                                "cached": False,
                                "session_mode": "acceptEdits",
                            },
                        ),
                    }
                )
                emit_snapshot()
                return PermissionResultAllow(
                    updated_input=tool_input,
                    updated_permissions=[
                        PermissionUpdate(
                            type="setMode",
                            destination="session",
                            mode="acceptEdits",
                        )
                    ],
                )

            if decision == "approve_for_session":
                state["approved_tool_cache_keys"].add(cache_key)
                hook_events.append(
                    {
                        "hook": "CanUseTool",
                        "input": build_hook_input(
                            tool_name,
                            tool_input,
                            suggestions,
                            context,
                            {
                                "interaction_kind": "tool_approval",
                                "approval_decision": "allow",
                                "approval_scope": "session",
                                "prompt_source": prompt_source,
                                "cached": False,
                            },
                        ),
                    }
                )
                emit_snapshot()
                return PermissionResultAllow(updated_input=tool_input)

            if decision == "approve":
                hook_events.append(
                    {
                        "hook": "CanUseTool",
                        "input": build_hook_input(
                            tool_name,
                            tool_input,
                            suggestions,
                            context,
                            {
                                "interaction_kind": "tool_approval",
                                "approval_decision": "allow",
                                "approval_scope": "request",
                                "prompt_source": prompt_source,
                                "cached": False,
                            },
                        ),
                    }
                )
                emit_snapshot()
                return PermissionResultAllow(updated_input=tool_input)

            if decision == "abort":
                hook_events.append(
                    {
                        "hook": "CanUseTool",
                        "input": build_hook_input(
                            tool_name,
                            tool_input,
                            suggestions,
                            context,
                            {
                                "interaction_kind": "tool_approval",
                                "approval_decision": "abort",
                                "approval_scope": "request",
                                "prompt_source": prompt_source,
                                "cached": False,
                            },
                        ),
                    }
                )
                emit_snapshot()
                return PermissionResultDeny(
                    message="User aborted this action", interrupt=True
                )

            hook_events.append(
                {
                    "hook": "CanUseTool",
                    "input": build_hook_input(
                        tool_name,
                        tool_input,
                        suggestions,
                        context,
                        {
                            "interaction_kind": "tool_approval",
                            "approval_decision": "deny",
                            "approval_scope": "request",
                            "prompt_source": prompt_source,
                            "cached": False,
                        },
                    ),
                }
            )
            emit_snapshot()
            return PermissionResultDeny(message="User denied this action")

        options.can_use_tool = can_use_tool
    elif tools and request.get("autoApproveTools") is True:

        async def can_use_tool(tool_name: str, tool_input: dict[str, Any], context: Any):
            suggestions = getattr(context, "suggestions", []) or []
            hook_events.append(
                {
                    "hook": "CanUseTool",
                    "input": build_hook_input(
                        tool_name,
                        tool_input,
                        suggestions,
                        context,
                        {
                            "interaction_kind": "tool_approval",
                            "approval_decision": "allow",
                            "approval_scope": "request",
                            "prompt_source": "auto_approve",
                            "cached": False,
                        },
                    ),
                }
            )
            emit_snapshot()
            return PermissionResultAllow(updated_input=tool_input)

        options.can_use_tool = can_use_tool

    prompt_value: Any = request.get("prompt", "")
    if options.can_use_tool is not None:
        prompt_value = single_prompt_stream(str(request.get("prompt", "")))

    timeout_seconds = request.get("timeoutSeconds")
    idle_timeout_seconds = request.get("idleTimeoutSeconds")

    async def consume_response() -> None:
        if ClaudeSDKClient is None:  # pragma: no cover - guarded by import check
            raise RuntimeError("claude_agent_sdk client is unavailable")

        async with ClaudeSDKClient(options) as client:
            await client.query(prompt_value)
            async for raw_message in client.receive_response():
                message = convert_sdk_message(raw_message, state)
                messages.append(message)
                emit_ndjson_event(stream_mode, "sdk_message", {"message": message})

                if message.get("type") == "system" and message.get("subtype") == "init":
                    emit_ndjson_event(stream_mode, "session_init", {"message": message})

                assistant_delta = extract_assistant_delta(message)
                if assistant_delta:
                    emit_ndjson_event(
                        stream_mode,
                        "assistant_delta",
                        {"delta": assistant_delta, "message": message},
                    )

                if message.get("type") == "assistant":
                    emit_ndjson_event(stream_mode, "assistant_message", {"message": message})

                if message.get("type") == "result":
                    if message.get("is_error") is True or message.get("subtype") == "error":
                        emit_ndjson_event(stream_mode, "error", {"message": message})
                    else:
                        emit_ndjson_event(stream_mode, "result", {"message": message})

                emit_snapshot()

    async def consume_response_with_idle_timeout(idle_timeout: float) -> None:
        loop = asyncio.get_running_loop()
        async with asyncio.timeout(None) as timeout_scope:
            timeout_scope.reschedule(loop.time() + idle_timeout)

            if ClaudeSDKClient is None:  # pragma: no cover - guarded by import check
                raise RuntimeError("claude_agent_sdk client is unavailable")

            async with ClaudeSDKClient(options) as client:
                await client.query(prompt_value)
                async for raw_message in client.receive_response():
                    timeout_scope.reschedule(loop.time() + idle_timeout)

                    message = convert_sdk_message(raw_message, state)
                    messages.append(message)
                    emit_ndjson_event(stream_mode, "sdk_message", {"message": message})

                    if message.get("type") == "system" and message.get("subtype") == "init":
                        emit_ndjson_event(stream_mode, "session_init", {"message": message})

                    assistant_delta = extract_assistant_delta(message)
                    if assistant_delta:
                        emit_ndjson_event(
                            stream_mode,
                            "assistant_delta",
                            {"delta": assistant_delta, "message": message},
                        )

                    if message.get("type") == "assistant":
                        emit_ndjson_event(stream_mode, "assistant_message", {"message": message})

                    if message.get("type") == "result":
                        if message.get("is_error") is True or message.get("subtype") == "error":
                            emit_ndjson_event(stream_mode, "error", {"message": message})
                        else:
                            emit_ndjson_event(stream_mode, "result", {"message": message})

                    emit_snapshot()

    try:
        if isinstance(idle_timeout_seconds, (int, float)) and idle_timeout_seconds > 0:
            await consume_response_with_idle_timeout(float(idle_timeout_seconds))
        elif isinstance(timeout_seconds, (int, float)) and timeout_seconds > 0:
            await asyncio.wait_for(consume_response(), timeout=float(timeout_seconds))
        else:
            await consume_response()
    except asyncio.TimeoutError:
        helper_timed_out = True
        emit_ndjson_event(stream_mode, "error", {"error": "helper_timed_out"})
    except Exception as exc:
        helper_error = str(exc)
        emit_ndjson_event(stream_mode, "error", {"error": helper_error})

    artifact = build_artifact(request, hook_events, messages, helper_timed_out, helper_error)
    if stream_mode:
        emit_snapshot()
        emit_ndjson_event(True, "final_artifact", {"artifact": artifact})
    else:
        sys.stdout.write(json.dumps(artifact))
        sys.stdout.flush()


async def async_main() -> None:
    request_body = read_stdin()
    if not request_body.strip():
        raise RuntimeError("managed helper request is empty")

    request = json.loads(request_body)
    mode = request.get("mode")
    if mode in {"query", "queryStream"}:
        await execute_query(request)
        return
    if mode == "listSessions":
        execute_list_sessions(request)
        return
    if mode == "getSessionMessages":
        execute_get_session_messages(request)
        return
    raise RuntimeError(f"unsupported helper mode: {mode!r}")


def main() -> int:
    try:
        asyncio.run(async_main())
        return 0
    except Exception as exc:
        detail = getattr(exc, "__traceback__", None)
        if detail is not None:
            import traceback

            traceback.print_exc(file=sys.stderr)
        else:
            sys.stderr.write(f"{exc}\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
