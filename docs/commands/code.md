# `libra code`

Launch an interactive AI coding session with TUI, web, or MCP modes.

## Synopsis

```
libra code
libra code --web-only [-p <PORT>] [--host <HOST>]
libra code --stdio
libra code --provider <PROVIDER> [--model <MODEL>]
libra code --resume <THREAD_ID>
```

## Description

`libra code` starts an interactive coding session that pairs a human developer with an AI agent. The default mode launches a terminal UI (TUI) built on ratatui/crossterm with a background web server. Two alternative modes are available: `--web-only` runs the web server without the TUI (useful for browser access or remote hosting), and `--stdio` runs an MCP server over standard input/output for integration with AI clients like Claude Desktop.

The command supports seven AI provider backends (Gemini, OpenAI, Anthropic, DeepSeek, Zhipu, Ollama, Codex) and three operating contexts (dev, review, research) that tune the agent's behavior for different workflows. Sessions can be persisted and resumed with Libra's canonical `--resume <thread_id>` flow.

A sandboxed tool-execution layer enforces approval policies that control when the agent can run shell commands, apply patches, or perform other potentially destructive operations.

## Options

| Flag | Short | Long | Default | Description |
|------|-------|------|---------|-------------|
| Web only | | `--web-only` / `--web` | off | Run the web server without TUI. Conflicts with `--stdio`. |
| Port | `-p` | `--port` | `3000` | Web server listen port. |
| Host | | `--host` | `127.0.0.1` | Web server bind address. |
| Working directory | | `--cwd` | current dir | Working directory for the session. |
| Provider | | `--provider` | `gemini` | AI provider backend (see Provider Backends below). |
| Model | | `--model` | provider default | Provider-specific model ID. |
| Temperature | | `--temperature` | provider default | Sampling temperature for generation. |
| Context | | `--context` | none | Operating context: `dev` (alias `development`), `review` (alias `code-review`), `research` (alias `explore`). |
| Resume | | `--resume <THREAD_ID>` | none | Resume a canonical Libra thread by thread ID. |
| Approval policy | | `--approval-policy` | `on-request` | Tool approval policy (see Approval Policies below). |
| MCP port | | `--mcp-port` | `6789` | MCP server listen port. |
| Stdio | | `--stdio` / `--mcp-stdio` | off | Run MCP over stdio. Conflicts with `--web-only`. |
| API base | | `--api-base` | provider default | Provider API base URL override. |
| Codex binary | | `--codex-bin` | `codex` | Codex executable path. |
| Codex port | | `--codex-port` | random | Override Codex app-server port. |
| Plan mode | | `--plan-mode` | off | Require the agent to produce a plan before execution (Codex mode). |

### Provider Backends

| Value | Description | API Key Env | Base URL Override |
|-------|-------------|-------------|-------------------|
| `gemini` | Google Gemini (default: gemini-2.5-flash) | `GEMINI_API_KEY` | -- |
| `openai` | OpenAI (default: gpt-4o-mini) | `OPENAI_API_KEY` | `OPENAI_BASE_URL` |
| `anthropic` | Anthropic (default: claude-3.5-sonnet) | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` |
| `deepseek` | DeepSeek | `DEEPSEEK_API_KEY` | -- |
| `zhipu` | Zhipu GLM (default: glm-5) | `ZHIPU_API_KEY` | `ZHIPU_BASE_URL` |
| `ollama` | Ollama (local models) | -- | `OLLAMA_BASE_URL` or `--api-base` |
| `codex` | Codex app-server | -- | `--codex-bin` / `--codex-port` |

### Approval Policies

| Value | Aliases | Description |
|-------|---------|-------------|
| `never` | -- | No prompts; dangerous commands are rejected outright. |
| `on-failure` | `on-failure` | Prompt only when retrying after a sandbox denial. |
| `on-request` | `on-request` | Run inside sandbox by default; prompt when escalation or policy requires it (default). |
| `untrusted` | `unless-trusted`, `untrusted` | Prompt for non-trusted operations; auto-allow known-safe reads. |

### Context Modes

| Value | Aliases | Description |
|-------|---------|-------------|
| `dev` | `development` | General development workflow. |
| `review` | `code-review` | Code review focus. |
| `research` | `explore` | Exploratory research and analysis. |

## Common Commands

```bash
# Start a TUI session with default Gemini provider
libra code

# Start with Anthropic Claude
libra code --provider anthropic --model claude-sonnet-4-20250514

# Run web-only on a custom port for remote access
libra code --web-only --port 8080 --host 0.0.0.0

# Run MCP over stdio for Claude Desktop integration
libra code --stdio

# Use a local Ollama model
libra code --provider ollama --model llama3 --api-base http://localhost:11434/v1

# Resume a canonical Libra thread
libra code --resume 11111111-1111-4111-8111-111111111111

# Start in code review context with strict approval
libra code --context review --approval-policy untrusted

# Use Codex with plan-before-execute mode
libra code --provider codex --plan-mode
```

## Human Output

Output is delivered through the TUI, web interface, or MCP protocol depending on the mode. There is no line-oriented stdout in the default TUI mode. The web server serves an embedded Next.js application. The stdio mode communicates via JSON-RPC messages following the Model Context Protocol.

## Design Rationale

### Why a TUI + web server hybrid?

The TUI provides a low-latency, keyboard-driven interface for developers already working in a terminal. The background web server runs simultaneously so that the same session can be accessed from a browser -- useful for sharing context with teammates, viewing rich diffs, or accessing the session from a mobile device. The `--web-only` flag drops the TUI entirely for headless or remote server deployments where no terminal is available.

### Why multiple AI provider support?

Different providers excel at different tasks and have different cost/latency profiles. Gemini is the default for its generous free tier and fast response times. Anthropic Claude excels at careful reasoning and code review. Local Ollama support enables fully offline development. By abstracting behind a `CompletionClient` trait, adding a new provider requires only implementing the trait without touching the session, tool, or TUI layers.

### Why MCP integration?

The Model Context Protocol (MCP) is an open standard for connecting AI clients to tool servers. By supporting `--stdio` mode, Libra can act as a tool server for any MCP-compatible client (e.g., Claude Desktop). This turns Libra's version-control tools -- status, diff, commit, branch -- into first-class tools that an external AI agent can invoke without reimplementing them.

### Why approval policies?

AI agents executing shell commands on a developer's machine present real safety risks. The four-tier approval system balances productivity with control:
- `never` is for fully locked-down environments where the agent can only read.
- `on-failure` lets the agent try sandboxed execution and only asks when it fails.
- `on-request` (default) sandboxes everything and escalates when the agent or sandbox policy requires it.
- `untrusted` is the most conservative interactive mode, prompting for anything beyond known-safe reads.

### Why session persistence and resume?

Long coding sessions accumulate significant context: file edits, conversation history, tool outputs. Losing this context on an accidental terminal close is painful. Session persistence stores the full conversation and tool state, and `--resume <thread_id>` restores a canonical Libra thread.

The embedded Code UI exposes the same canonical identifier as `threadId` in its session snapshot. Older `session_id` fields remain present for compatibility, but new integrations should key resume, Web, MCP, and diagnostics flows by `threadId`.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Interactive AI session | `libra code` | Not available | Not available |
| TUI mode | Default | Not available | Not available |
| Web mode | `--web-only` | Not available | Not available |
| MCP/stdio mode | `--stdio` | Not available | Not available |
| AI provider selection | `--provider` | Not available | Not available |
| Session resume | `--resume <thread_id>` | Not available | Not available |
| Tool approval policy | `--approval-policy` | Not available | Not available |

Note: Neither Git nor jj have an equivalent to `libra code`. This command represents Libra's core differentiation as an AI-agent-native version control system. The closest analogs in the Git ecosystem are third-party tools like GitHub Copilot CLI or aider, which are separate applications rather than integrated VCS commands.

## Error Handling

| Scenario | Behavior | Exit |
|----------|----------|------|
| `--web-only` and `--stdio` both specified | Clap argument conflict error | non-zero |
| Missing API key for selected provider | Fatal error with provider name and expected env var | non-zero |
| Port already in use | Fatal error with port number | non-zero |
| No terminal available in TUI mode | Falls back or reports error | non-zero |
| Thread ID not found on resume | Fatal error with canonical `thread_id` | non-zero |
