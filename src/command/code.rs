//! # Code Command — Interactive AI-Powered Coding Sessions
//!     中文：标题：Code Command — Interactive AI-Powered Coding Sessions。
//!
//! This module implements the `libra code` subcommand, which is the primary entry point
//!     中文：该注释与英文“This module implements the `libra code` subcommand, which is the primary entry point”含义一致。
//! for AI-agent-driven and human-collaborative development within a Libra repository.
//!     中文：该注释与英文“for AI-agent-driven and human-collaborative development within a Libra repository.”含义一致。
//!
//! ## Architecture Overview
//!     中文：标题：Architecture Overview。
//!
//! The command orchestrates several concurrent subsystems:
//!     中文：该注释与英文“The command orchestrates several concurrent subsystems:”含义一致。
//!
//! - **TUI (Terminal UI)**: A `ratatui`/`crossterm`-based interactive terminal interface
//!     中文：列表项说明与英文“**TUI (Terminal UI)**: A `ratatui`/`crossterm`-based interactive terminal interface”含义一致。
//!   that renders the chat conversation, tool outputs, and approval prompts.
//!     中文：该注释与英文“that renders the chat conversation, tool outputs, and approval prompts.”含义一致。
//! - **Web Server**: An embedded `axum` HTTP server that serves the Next.js static export
//!     中文：列表项说明与英文“**Web Server**: An embedded `axum` HTTP server that serves the Next.js static export”含义一致。
//!   from `web/out/`, providing a browser-based UI alternative.
//!     中文：该注释与英文“from `web/out/`, providing a browser-based UI alternative.”含义一致。
//! - **MCP Server**: A Model Context Protocol server (using `rmcp`) that exposes Libra's
//!     中文：列表项说明与英文“**MCP Server**: A Model Context Protocol server (using `rmcp`) that exposes Libra's”含义一致。
//!   tools (read, grep, patch, shell, etc.) over Streamable HTTP or Stdio transport,
//!     中文：该注释与英文“tools (read, grep, patch, shell, etc.) over Streamable HTTP or Stdio transport,”含义一致。
//!   enabling integration with external AI clients such as Claude Desktop.
//!     中文：该注释与英文“enabling integration with external AI clients such as Claude Desktop.”含义一致。
//! - **AI Agent**: A tool-calling loop powered by configurable LLM providers (Gemini,
//!     中文：列表项说明与英文“**AI Agent**: A tool-calling loop powered by configurable LLM providers (Gemini,”含义一致。
//!   OpenAI, Anthropic, DeepSeek, Kimi, Zhipu, Ollama) or the managed Codex runtime.
//!     中文：该注释与英文“OpenAI, Anthropic, DeepSeek, Kimi, Zhipu, Ollama) or the managed Codex runtime.”含义一致。
//!
//! ## Supported Modes
//!     中文：标题：Supported Modes。
//!
//! The command supports three mutually exclusive operating modes:
//!     中文：该注释与英文“The command supports three mutually exclusive operating modes:”含义一致。
//!
//! | Mode | Flag | Description |
//!     中文：该注释与英文“| Mode | Flag | Description |”含义一致。
//! |------|------|-------------|
//! | **TUI** (default) | *(none)* | Full interactive terminal UI with background web + MCP servers |
//!     中文：该注释与英文“| **TUI** (default) | *(none)* | Full interactive terminal UI with background web + MCP servers |”含义一致。
//! | **Web-only** | `--web` | Headless web server + MCP server; no terminal UI |
//!     中文：该注释与英文“| **Web-only** | `--web` | Headless web server + MCP server; no terminal UI |”含义一致。
//! | **Stdio** | `--stdio` | MCP server over stdin/stdout for AI client integration |
//!     中文：该注释与英文“| **Stdio** | `--stdio` | MCP server over stdin/stdout for AI client integration |”含义一致。
//!
//! ## Provider Dispatch
//!     中文：标题：Provider Dispatch。
//!
//! The `--provider` flag selects the AI backend. Each provider follows the same pattern:
//!     中文：该注释与英文“The `--provider` flag selects the AI backend. Each provider follows the same pattern:”含义一致。
//! 1. Create a client from environment variables (API keys).
//!     中文：该注释与英文“1. Create a client from environment variables (API keys).”含义一致。
//! 2. Instantiate a completion model with the selected (or default) model name.
//!     中文：该注释与英文“2. Instantiate a completion model with the selected (or default) model name.”含义一致。
//! 3. Pass the model into the shared `run_tui_with_model` function.
//!     中文：该注释与英文“3. Pass the model into the shared `run_tui_with_model` function.”含义一致。
//!
//! The `codex` provider bypasses the generic completion model path and uses its
//!     中文：该注释与英文“The `codex` provider bypasses the generic completion model path and uses its”含义一致。
//! managed app-server runtime with a dedicated execution flow.
//!     中文：该注释与英文“managed app-server runtime with a dedicated execution flow.”含义一致。
//!
//! ## Sandbox & Approval
//!     中文：标题：Sandbox & Approval。
//!
//! Tool execution is governed by a layered sandbox and approval system:
//!     中文：该注释与英文“Tool execution is governed by a layered sandbox and approval system:”含义一致。
//! - **SandboxPolicy**: Controls filesystem and network access (read-only for review/research,
//!     中文：列表项说明与英文“**SandboxPolicy**: Controls filesystem and network access (read-only for review/research,”含义一致。
//!   workspace-write for dev mode).
//!     中文：该注释与英文“workspace-write for dev mode).”含义一致。
//! - **AskForApproval**: Determines when to prompt the user for tool execution approval
//!     中文：列表项说明与英文“**AskForApproval**: Determines when to prompt the user for tool execution approval”含义一致。
//!   (never, on-failure, on-request, unless-trusted).
//!     中文：该注释与英文“(never, on-failure, on-request, unless-trusted).”含义一致。
//!
//! ## Session Persistence
//!     中文：标题：Session Persistence。
//!
//! Conversation history is persisted via `SessionStore` under the `.libra/` storage
//!     中文：该注释与英文“Conversation history is persisted via `SessionStore` under the `.libra/` storage”含义一致。
//! directory, supporting `--resume <thread_id>` to continue a canonical Libra thread.
//!     中文：该注释与英文“directory, supporting `--resume <thread_id>` to continue a canonical Libra thread.”含义一致。
//!
//! Cross-references for agents extending this command:
//!     中文：该注释与英文“Cross-references for agents extending this command:”含义一致。
//! - Agent workflow and object model: `docs/agent/agent-workflow.md`
//!     中文：列表项说明与英文“Agent workflow and object model: `docs/agent/agent-workflow.md`”含义一致。
//! - MCP upgrade and transport notes: `docs/agent/mcp-upgrade-report.md`
//!     中文：列表项说明与英文“MCP upgrade and transport notes: `docs/agent/mcp-upgrade-report.md`”含义一致。
//! - IntentSpec contract examples: `docs/agent/intentspec_typical.yaml`
//!     中文：列表项说明与英文“IntentSpec contract examples: `docs/agent/intentspec_typical.yaml`”含义一致。

use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use clap::{Parser, ValueEnum};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    service::TowerToHyperService,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, session::local::LocalSessionManager,
};
use serde::{Deserialize, Serialize};
use tokio::{
    process::{Child, Command},
    sync::{mpsc, oneshot},
    time::{Duration, Instant, sleep},
};
use tokio_tungstenite::connect_async;
use url::Url;
use uuid::Uuid;

#[cfg(feature = "test-provider")]
use crate::internal::ai::providers::fake::FAKE_DEFAULT_MODEL;
use crate::{
    cli_error,
    command::code_control_files::{
        ControlInfo, ControlLockError, ControlLockGuard, ControlPaths, acquire_control_lock,
        cleanup_control_files, ensure_control_token_file, resolve_control_paths,
        write_control_info,
    },
    internal::{
        ai::{
            agent::{
                TaskIntent, ToolLoopConfig,
                profile::{AgentProfileRouter, AgentsConfig, load_profiles},
            },
            codex as agent_codex,
            commands::{CommandDispatcher, load_commands},
            completion::{
                CompletionError, CompletionModel, CompletionReasoningEffort, CompletionRequest,
                CompletionResponse, CompletionThinking, CompletionUsage,
            },
            context_budget::{
                ContextAttachmentStore, ContextBudget, ContextFrameBuilder, ContextFrameCandidate,
                ContextFrameKind, ContextFrameSource, ContextSegmentKind, ContextTrustLevel,
            },
            history::HistoryManager,
            hooks::HookRunner,
            mcp::server::LibraMcpServer,
            projection::{ProjectionRebuilder, ProjectionResolver, ResumeBundle, ThreadBundle},
            prompt::{ContextMode, SystemPromptBuilder},
            providers::{
                anthropic::CLAUDE_3_5_SONNET, gemini::GEMINI_2_5_FLASH, kimi::KIMI_K2_6,
                openai::GPT_4O_MINI, zhipu::GLM_5,
            },
            runtime::{ToolBoundaryRuntime, TracingAuditSink},
            sandbox::{
                ApprovalCachePolicy, ApprovalStore, AskForApproval, DEFAULT_APPROVAL_TTL,
                ExecApprovalRequest, NetworkAccess, SandboxPermissions, SandboxPolicy,
                ToolApprovalContext, ToolRuntimeContext, ToolSandboxContext,
            },
            session::{
                SessionState, SessionStore,
                jsonl::{SessionEvent, SessionJsonlStore},
            },
            skills::{SkillDispatcher, load_skills},
            sources::{SourcePool, register_builtin_mcp_source_from_project_config},
            tools::{
                ToolRegistry, ToolRegistryBuilder,
                context::UserInputRequest,
                handlers::{
                    ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler,
                    PlanHandler, ReadFileHandler, RequestUserInputHandler, SearchFilesHandler,
                    ShellHandler, SubmitIntentDraftHandler, SubmitPlanDraftHandler,
                    SubmitTaskCompleteHandler, WebSearchHandler, register_semantic_handlers,
                },
            },
            usage::{UsageContext, UsagePriceTable, UsageRecorder},
            web::{
                WebServerHandle, WebServerOptions,
                code_ui::{
                    CodeUiCapabilities, CodeUiControllerKind, CodeUiInitialController,
                    CodeUiInteractionStatus, CodeUiProviderAdapter, CodeUiProviderInfo,
                    CodeUiRuntimeHandle, CodeUiRuntimeOptions, CodeUiSession,
                    CodeUiSessionSnapshot, CodeUiSessionStatus, CodeUiTranscriptEntry,
                    CodeUiTranscriptEntryKind, ReadOnlyCodeUiAdapter, initial_snapshot,
                    snapshot_from_thread_bundle,
                },
                headless::{
                    HeadlessCodeRuntime, HeadlessSessionPersistence, headless_capabilities,
                },
                start as start_web_server,
            },
        },
        db::establish_connection,
        tui::{
            App, AppConfig, ExitReason, Tui, TuiCodeUiAdapter, control::TuiControlCommand,
            tui_init, tui_restore,
        },
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        storage::local::LocalStorage,
        util::{DATABASE, try_get_storage_path},
    },
};

// ---------------------------------------------------------------------------
// Constants — default network ports, bind address, and Codex startup tuning
// 中文：该注释与英文“Constants — default network ports, bind address, and Codex startup tuning”含义一致。
// ---------------------------------------------------------------------------

/// Default port for the embedded web server serving the Next.js static export.
///     中文：该注释与英文“Default port for the embedded web server serving the Next.js static export.”含义一致。
const DEFAULT_WEB_PORT: u16 = 3000;

/// Default port for the MCP (Model Context Protocol) HTTP server.
///     中文：该注释与英文“Default port for the MCP (Model Context Protocol) HTTP server.”含义一致。
const DEFAULT_MCP_PORT: u16 = 6789;

/// Default network interface to bind servers to (localhost only).
///     中文：该注释与英文“Default network interface to bind servers to (localhost only).”含义一致。
const DEFAULT_BIND_HOST: &str = "127.0.0.1";

/// Default executable name for the Codex CLI app-server.
///     中文：该注释与英文“Default executable name for the Codex CLI app-server.”含义一致。
const DEFAULT_CODEX_BIN: &str = "codex";

/// Maximum time to wait for the Codex app-server WebSocket to become reachable.
///     中文：该注释与英文“Maximum time to wait for the Codex app-server WebSocket to become reachable.”含义一致。
const CODEX_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Interval between WebSocket connectivity checks during Codex startup.
///     中文：该注释与英文“Interval between WebSocket connectivity checks during Codex startup.”含义一致。
const CODEX_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// Enums — provider selection, context mode, and approval policy
// 中文：该注释与英文“Enums — provider selection, context mode, and approval policy”含义一致。
// ---------------------------------------------------------------------------

/// Available AI provider backends for the `libra code` command.
///     中文：该注释与英文“Available AI provider backends for the `libra code` command.”含义一致。
///
/// Each variant maps to a specific LLM client implementation. The provider
///     中文：该注释与英文“Each variant maps to a specific LLM client implementation. The provider”含义一致。
/// determines which API key environment variable is required and which
///     中文：该注释与英文“determines which API key environment variable is required and which”含义一致。
/// default model is used when `--model` is omitted.
///     中文：该注释与英文“default model is used when `--model` is omitted.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeProvider {
    Gemini,
    Openai,
    Anthropic,
    Deepseek,
    Kimi,
    Zhipu,
    Ollama,
    Codex,
    #[cfg(feature = "test-provider")]
    #[value(name = "fake", hide = true)]
    Fake,
}

/// Operating context that shapes the agent's system prompt and sandbox policy.
///     中文：该注释与英文“Operating context that shapes the agent's system prompt and sandbox policy.”含义一致。
///
/// - `Dev`: Full read-write access to the workspace; the agent can modify files.
///     中文：列表项说明与英文“`Dev`: Full read-write access to the workspace; the agent can modify files.”含义一致。
/// - `Review`: Read-only sandbox; the agent focuses on code review feedback.
///     中文：列表项说明与英文“`Review`: Read-only sandbox; the agent focuses on code review feedback.”含义一致。
/// - `Research`: Read-only sandbox; the agent focuses on codebase exploration.
///     中文：列表项说明与英文“`Research`: Read-only sandbox; the agent focuses on codebase exploration.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeContext {
    #[value(alias = "development")]
    Dev,
    #[value(alias = "code-review")]
    Review,
    #[value(alias = "explore")]
    Research,
}

/// Local TUI automation control mode.
///     中文：该注释与英文“Local TUI automation control mode.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    /// Keep the current loopback-only read behavior; no write token is created.
    ///     中文：该注释与英文“Keep the current loopback-only read behavior; no write token is created.”含义一致。
    Observe,
    /// Enable local automation write control with token and controller checks.
    ///     中文：该注释与英文“Enable local automation write control with token and controller checks.”含义一致。
    Write,
}

/// Browser write-control posture for `libra code`.
///     中文：该注释与英文“Browser write-control posture for `libra code`.”含义一致。
///
/// Controls whether `/api/code/controller/attach` will issue a `Browser`
///     中文：该注释与英文“Controls whether `/api/code/controller/attach` will issue a `Browser`”含义一致。
/// lease (allowing the embedded UI to drive `/messages`,
///     中文：该注释与英文“lease (allowing the embedded UI to drive `/messages`,”含义一致。
/// `/interactions/{id}`, and `/control/cancel`). The `--host` is still
///     中文：该注释与英文“`/interactions/{id}`, and `/control/cancel`). The `--host` is still”含义一致。
/// forced to a loopback address whenever `loopback` is selected — see
///     中文：该注释与英文“forced to a loopback address whenever `loopback` is selected — see”含义一致。
/// [`ensure_loopback_browser_control_host`].
///     中文：该注释与英文“[`ensure_loopback_browser_control_host`].”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Default)]
pub enum BrowserControlMode {
    /// Browser controllers cannot attach. Default for normal TUI sessions and
    ///     中文：该注释与英文“Browser controllers cannot attach. Default for normal TUI sessions and”含义一致。
    /// for `--web-only` against non-Codex providers.
    ///     中文：该注释与英文“for `--web-only` against non-Codex providers.”含义一致。
    #[default]
    Off,
    /// Browser controllers may attach as long as the bound `--host` is
    ///     中文：该注释与英文“Browser controllers may attach as long as the bound `--host` is”含义一致。
    /// loopback. Default for `--web-only --provider codex`.
    ///     中文：该注释与英文“loopback. Default for `--web-only --provider codex`.”含义一致。
    Loopback,
}

impl BrowserControlMode {
    /// Returns the canonical wire-format string used in banners, info files,
    ///     中文：该注释与英文“Returns the canonical wire-format string used in banners, info files,”含义一致。
    /// and audit summaries — matches the clap value names exactly.
    ///     中文：该注释与英文“and audit summaries — matches the clap value names exactly.”含义一致。
    pub fn as_str(self) -> &'static str {
        match self {
            BrowserControlMode::Off => "off",
            BrowserControlMode::Loopback => "loopback",
        }
    }
}

/// Ollama-specific thinking/reasoning mode.
///     中文：该注释与英文“Ollama-specific thinking/reasoning mode.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OllamaThinkingArg {
    /// Let Ollama decide by omitting the `think` field.
    ///     中文：该注释与英文“Let Ollama decide by omitting the `think` field.”含义一致。
    Auto,
    /// Disable thinking for faster local tool-calling responses.
    ///     中文：该注释与英文“Disable thinking for faster local tool-calling responses.”含义一致。
    Off,
    /// Enable thinking without specifying a depth.
    ///     中文：该注释与英文“Enable thinking without specifying a depth.”含义一致。
    On,
    /// Request low thinking depth.
    ///     中文：该注释与英文“Request low thinking depth.”含义一致。
    Low,
    /// Request medium thinking depth.
    ///     中文：该注释与英文“Request medium thinking depth.”含义一致。
    Medium,
    /// Request high thinking depth.
    ///     中文：该注释与英文“Request high thinking depth.”含义一致。
    High,
}

impl From<OllamaThinkingArg> for CompletionThinking {
    fn from(value: OllamaThinkingArg) -> Self {
        match value {
            OllamaThinkingArg::Auto => CompletionThinking::Auto,
            OllamaThinkingArg::Off => CompletionThinking::Disabled,
            OllamaThinkingArg::On => CompletionThinking::Enabled,
            OllamaThinkingArg::Low => CompletionThinking::Low,
            OllamaThinkingArg::Medium => CompletionThinking::Medium,
            OllamaThinkingArg::High => CompletionThinking::High,
        }
    }
}

/// DeepSeek-specific thinking mode.
///     中文：该注释与英文“DeepSeek-specific thinking mode.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum DeepSeekThinkingArg {
    /// Send `thinking: {"type": "enabled"}` to DeepSeek.
    ///     中文：该注释与英文“Send `thinking: {"type": "enabled"}` to DeepSeek.”含义一致。
    Enabled,
    /// Send `thinking: {"type": "disabled"}` to DeepSeek.
    ///     中文：该注释与英文“Send `thinking: {"type": "disabled"}` to DeepSeek.”含义一致。
    Disabled,
}

impl From<DeepSeekThinkingArg> for CompletionThinking {
    fn from(value: DeepSeekThinkingArg) -> Self {
        match value {
            DeepSeekThinkingArg::Enabled => CompletionThinking::Enabled,
            DeepSeekThinkingArg::Disabled => CompletionThinking::Disabled,
        }
    }
}

/// Kimi-specific thinking mode.
///     中文：该注释与英文“Kimi-specific thinking mode.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum KimiThinkingArg {
    /// Send `thinking: {"type": "enabled"}` to Kimi.
    ///     中文：该注释与英文“Send `thinking: {"type": "enabled"}` to Kimi.”含义一致。
    Enabled,
    /// Send `thinking: {"type": "disabled"}` to Kimi.
    ///     中文：该注释与英文“Send `thinking: {"type": "disabled"}` to Kimi.”含义一致。
    Disabled,
}

impl From<KimiThinkingArg> for CompletionThinking {
    fn from(value: KimiThinkingArg) -> Self {
        match value {
            KimiThinkingArg::Enabled => CompletionThinking::Enabled,
            KimiThinkingArg::Disabled => CompletionThinking::Disabled,
        }
    }
}

/// DeepSeek-specific reasoning effort.
///     中文：该注释与英文“DeepSeek-specific reasoning effort.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum DeepSeekReasoningEffortArg {
    Low,
    Medium,
    High,
    #[value(alias = "xhigh")]
    Max,
}

impl From<DeepSeekReasoningEffortArg> for CompletionReasoningEffort {
    fn from(value: DeepSeekReasoningEffortArg) -> Self {
        match value {
            DeepSeekReasoningEffortArg::Low => CompletionReasoningEffort::Low,
            DeepSeekReasoningEffortArg::Medium => CompletionReasoningEffort::Medium,
            DeepSeekReasoningEffortArg::High => CompletionReasoningEffort::High,
            DeepSeekReasoningEffortArg::Max => CompletionReasoningEffort::Max,
        }
    }
}

/// User-facing approval policy controlling when tool execution requires
///     中文：该注释与英文“User-facing approval policy controlling when tool execution requires”含义一致。
/// explicit human confirmation in the TUI.
///     中文：该注释与英文“explicit human confirmation in the TUI.”含义一致。
///
/// This enum is the CLI-facing representation; it converts into the internal
///     中文：该注释与英文“This enum is the CLI-facing representation; it converts into the internal”含义一致。
/// [`AskForApproval`] enum via the `From` impl below.
///     中文：该注释与英文“[`AskForApproval`] enum via the `From` impl below.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeApprovalPolicy {
    /// Never prompt; dangerous commands are rejected.
    ///     中文：该注释与英文“Never prompt; dangerous commands are rejected.”含义一致。
    Never,
    /// Never prompt; allow every command for this interactive session.
    ///     中文：该注释与英文“Never prompt; allow every command for this interactive session.”含义一致。
    #[value(
        alias = "allow-all",
        alias = "allow_all",
        alias = "always",
        alias = "accept"
    )]
    AllowAll,
    /// Prompt only when retrying after sandbox denial.
    ///     中文：该注释与英文“Prompt only when retrying after sandbox denial.”含义一致。
    #[value(alias = "on-failure")]
    OnFailure,
    /// Run inside sandbox by default; prompt when escalation or policy requires it.
    ///     中文：该注释与英文“Run inside sandbox by default; prompt when escalation or policy requires it.”含义一致。
    #[value(alias = "on-request")]
    OnRequest,
    /// Prompt for non-trusted operations (safe read commands are auto-allowed).
    ///     中文：该注释与英文“Prompt for non-trusted operations (safe read commands are auto-allowed).”含义一致。
    #[value(alias = "unless-trusted", alias = "untrusted")]
    Untrusted,
}

/// Developer-selected network access policy for TUI execution.
///     中文：该注释与英文“Developer-selected network access policy for TUI execution.”含义一致。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CodeNetworkAccess {
    /// Allow shell and gate tasks to use network access.
    ///     中文：该注释与英文“Allow shell and gate tasks to use network access.”含义一致。
    Allow,
    /// Deny network access for shell and gate tasks.
    ///     中文：该注释与英文“Deny network access for shell and gate tasks.”含义一致。
    Deny,
}

impl CodeNetworkAccess {
    fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

impl CodeApprovalPolicy {
    fn allows_all_commands(self) -> bool {
        matches!(self, Self::AllowAll)
    }
}

/// Maps the user-facing [`CodeApprovalPolicy`] to the internal [`AskForApproval`]
///     中文：该注释与英文“Maps the user-facing [`CodeApprovalPolicy`] to the internal [`AskForApproval`]”含义一致。
/// enum used by the sandbox/approval subsystem.
///     中文：该注释与英文“enum used by the sandbox/approval subsystem.”含义一致。
impl From<CodeApprovalPolicy> for AskForApproval {
    fn from(value: CodeApprovalPolicy) -> Self {
        match value {
            CodeApprovalPolicy::Never => AskForApproval::Never,
            CodeApprovalPolicy::AllowAll => AskForApproval::OnRequest,
            CodeApprovalPolicy::OnFailure => AskForApproval::OnFailure,
            CodeApprovalPolicy::OnRequest => AskForApproval::OnRequest,
            CodeApprovalPolicy::Untrusted => AskForApproval::UnlessTrusted,
        }
    }
}

// ---------------------------------------------------------------------------
// CLI argument definition
// 中文：该注释与英文“CLI argument definition”含义一致。
// ---------------------------------------------------------------------------

/// `--help` examples shown in `libra code --help` output.
///     中文：该注释与英文“`--help` examples shown in `libra code --help` output.”含义一致。
///
/// `code` launches the interactive Libra Code session in one of three
///     中文：该注释与英文“`code` launches the interactive Libra Code session in one of three”含义一致。
/// modes: TUI (the default), web-only (`--web` / `--web-only`), or
///     中文：该注释与英文“modes: TUI (the default), web-only (`--web` / `--web-only`), or”含义一致。
/// stdio. The banner pins the most common invocations across modes
///     中文：该注释与英文“stdio. The banner pins the most common invocations across modes”含义一致。
/// (TUI default, web-only with a specific provider, `--browser-control
///     中文：该注释与英文“(TUI default, web-only with a specific provider, `--browser-control”含义一致。
/// loopback`, `--control write` for local automation write control,
///     中文：该注释与英文“loopback`, `--control write` for local automation write control,”含义一致。
/// resume by thread id, plan mode, and `--env-file` for vault-less
///     中文：该注释与英文“resume by thread id, plan mode, and `--env-file` for vault-less”含义一致。
/// provider bootstrap) so users see the right entry point without
///     中文：该注释与英文“provider bootstrap) so users see the right entry point without”含义一致。
/// reading the design doc. Cross-cutting `--help` EXAMPLES rollout per
///     中文：该注释与英文“reading the design doc. Cross-cutting `--help` EXAMPLES rollout per”含义一致。
/// `docs/improvement/README.md` item B.
///     中文：该注释与英文“`docs/improvement/README.md` item B.”含义一致。
pub const CODE_EXAMPLES: &str = "\
EXAMPLES:
    libra code                                       Launch the default TUI session
    libra code --provider deepseek --model deepseek-reasoner
                                                     Pick a provider/model at startup
    libra code --web                                 Run the web server only (no TUI); alias for --web-only
    libra code --web-only --provider ollama --port 4400
                                                     Browser-driven session against a local Ollama
    libra code --web-only --provider codex --browser-control loopback
                                                     Allow browser write control over loopback
    libra code --control write                       Enable local automation write control (token + controller checks)
    libra code --resume <thread-uuid>                Resume a prior canonical thread
    libra code --plan-mode                           Start in plan-only mode (no apply)
    libra code --env-file .env.test                  Load provider keys from a dotenv-style file
    libra code --stdio                               Pipe-driven session for embedding";

/// Command-line arguments for `libra code`.
///     中文：该注释与英文“Command-line arguments for `libra code`.”含义一致。
///
/// This struct is parsed by `clap` and drives all three operating modes
///     中文：该注释与英文“This struct is parsed by `clap` and drives all three operating modes”含义一致。
/// (TUI, web-only, stdio). Many flags are mode-specific and validated
///     中文：该注释与英文“(TUI, web-only, stdio). Many flags are mode-specific and validated”含义一致。
/// at runtime by [`validate_mode_args`].
///     中文：该注释与英文“at runtime by [`validate_mode_args`].”含义一致。
#[derive(Parser, Debug)]
#[command(after_help = CODE_EXAMPLES)]
pub struct CodeArgs {
    /// Run the web server only (no TUI). Alias: `--web`.
    ///     中文：该注释与英文“Run the web server only (no TUI). Alias: `--web`.”含义一致。
    #[arg(long, alias = "web", conflicts_with = "stdio")]
    pub web_only: bool,

    /// Port to listen on (web server)
    ///     中文：该注释与英文“Port to listen on (web server)”含义一致。
    #[arg(short, long, default_value_t = DEFAULT_WEB_PORT)]
    pub port: u16,

    /// Host address to bind to (web server)
    ///     中文：该注释与英文“Host address to bind to (web server)”含义一致。
    #[arg(long, default_value = DEFAULT_BIND_HOST)]
    pub host: String,

    /// Working directory for the code session (default: current directory)
    ///     中文：该注释与英文“Working directory for the code session (default: current directory)”含义一致。
    #[arg(long, value_name = "PATH")]
    pub cwd: Option<PathBuf>,

    /// Path to a Libra repository (default: discover from current directory)
    ///     中文：该注释与英文“Path to a Libra repository (default: discover from current directory)”含义一致。
    #[arg(long, value_name = "PATH")]
    pub repo: Option<PathBuf>,

    /// Load provider environment variables from a dotenv-style file.
    ///     中文：该注释与英文“Load provider environment variables from a dotenv-style file.”含义一致。
    ///
    /// Values in this file take precedence over already exported process
    ///     中文：该注释与英文“Values in this file take precedence over already exported process”含义一致。
    /// environment variables for provider bootstrap.
    ///     中文：该注释与英文“environment variables for provider bootstrap.”含义一致。
    #[arg(long = "env-file", value_name = "PATH")]
    pub env_file: Option<PathBuf>,

    /// Local TUI automation control mode.
    ///     中文：该注释与英文“Local TUI automation control mode.”含义一致。
    #[arg(long, value_enum, default_value_t = ControlMode::Observe)]
    pub control: ControlMode,

    /// Browser write-control posture (`off` | `loopback`).
    ///     中文：该注释与英文“Browser write-control posture (`off` | `loopback`).”含义一致。
    ///
    /// Defaults are mode-specific:
    ///     中文：该注释与英文“Defaults are mode-specific:”含义一致。
    /// - normal TUI session → `off`
    ///     中文：列表项说明与英文“normal TUI session → `off`”含义一致。
    /// - `--web-only --provider codex` → `loopback`
    ///     中文：列表项说明与英文“`--web-only --provider codex` → `loopback`”含义一致。
    /// - `--web-only` with any other provider → `off`
    ///     中文：列表项说明与英文“`--web-only` with any other provider → `off`”含义一致。
    ///
    /// Selecting `loopback` is rejected when `--host` is not a loopback
    ///     中文：该注释与英文“Selecting `loopback` is rejected when `--host` is not a loopback”含义一致。
    /// address, and the flag is incompatible with `--stdio`.
    ///     中文：该注释与英文“address, and the flag is incompatible with `--stdio`.”含义一致。
    #[arg(long = "browser-control", value_enum, conflicts_with = "stdio")]
    pub browser_control: Option<BrowserControlMode>,

    /// Path to the local automation control token file
    ///     中文：该注释与英文“Path to the local automation control token file”含义一致。
    #[arg(long, value_name = "PATH")]
    pub control_token_file: Option<PathBuf>,

    /// Path to the local automation control discovery info file
    ///     中文：该注释与英文“Path to the local automation control discovery info file”含义一致。
    #[arg(long, value_name = "PATH")]
    pub control_info_file: Option<PathBuf>,

    /// AI provider backend
    ///     中文：该注释与英文“AI provider backend”含义一致。
    #[arg(long, value_enum, default_value_t = CodeProvider::Gemini)]
    pub provider: CodeProvider,

    /// Model id (provider-specific)
    ///     中文：该注释与英文“Model id (provider-specific)”含义一致。
    #[arg(long)]
    pub model: Option<String>,

    /// Sampling temperature (provider-specific range, typically 0.0–2.0)
    ///     中文：该注释与英文“Sampling temperature (provider-specific range, typically 0.0–2.0)”含义一致。
    #[arg(long, value_name = "FLOAT")]
    pub temperature: Option<f64>,

    /// Ollama thinking mode: auto, off, on, low, medium, or high.
    ///     中文：该注释与英文“Ollama thinking mode: auto, off, on, low, medium, or high.”含义一致。
    ///
    /// If omitted, Ollama uses OLLAMA_THINK and then defaults to `off`.
    ///     中文：该注释与英文“If omitted, Ollama uses OLLAMA_THINK and then defaults to `off`.”含义一致。
    #[arg(long = "ollama-thinking", alias = "thinking", value_enum)]
    pub ollama_thinking: Option<OllamaThinkingArg>,

    /// Send compact Ollama tool schemas for providers that reject complex JSON schemas.
    ///     中文：该注释与英文“Send compact Ollama tool schemas for providers that reject complex JSON schemas.”含义一致。
    #[arg(long = "ollama-compact-tools")]
    pub ollama_compact_tools: bool,

    /// DeepSeek thinking mode: enabled or disabled.
    ///     中文：该注释与英文“DeepSeek thinking mode: enabled or disabled.”含义一致。
    #[arg(long = "deepseek-thinking", value_enum)]
    pub deepseek_thinking: Option<DeepSeekThinkingArg>,

    /// DeepSeek reasoning effort: low, medium, high, or max.
    ///     中文：该注释与英文“DeepSeek reasoning effort: low, medium, high, or max.”含义一致。
    #[arg(long = "deepseek-reasoning-effort", value_enum)]
    pub deepseek_reasoning_effort: Option<DeepSeekReasoningEffortArg>,

    /// DeepSeek stream mode: true or false.
    ///     中文：该注释与英文“DeepSeek stream mode: true or false.”含义一致。
    #[arg(long = "deepseek-stream", alias = "stream", value_name = "BOOL")]
    pub deepseek_stream: Option<bool>,

    /// Kimi thinking mode: enabled or disabled.
    ///     中文：该注释与英文“Kimi thinking mode: enabled or disabled.”含义一致。
    #[arg(long = "kimi-thinking", value_enum)]
    pub kimi_thinking: Option<KimiThinkingArg>,

    /// Kimi stream mode: true or false. Defaults to true for Kimi.
    ///     中文：该注释与英文“Kimi stream mode: true or false. Defaults to true for Kimi.”含义一致。
    #[arg(long = "kimi-stream", value_name = "BOOL")]
    pub kimi_stream: Option<bool>,

    /// Select an agent profile by name. When the profile carries a structured
    ///     中文：该注释与英文“Select an agent profile by name. When the profile carries a structured”含义一致。
    /// `model: provider/model[@variant]` binding, the agent's binding wins
    ///     中文：该注释与英文“`model: provider/model[@variant]` binding, the agent's binding wins”含义一致。
    /// atomically — provider, model id, and variant all come from the
    ///     中文：该注释与英文“atomically — provider, model id, and variant all come from the”含义一致。
    /// agent's spec, and a separately-supplied `--model` is ignored to avoid
    ///     中文：该注释与英文“agent's spec, and a separately-supplied `--model` is ignored to avoid”含义一致。
    /// hybrid pairs (anthropic provider + OpenAI-shaped model id). Profiles
    ///     中文：该注释与英文“hybrid pairs (anthropic provider + OpenAI-shaped model id). Profiles”含义一致。
    /// without a structured binding fall back to the CLI defaults verbatim.
    ///     中文：该注释与英文“without a structured binding fall back to the CLI defaults verbatim.”含义一致。
    /// Profiles are looked up via the same three-tier hierarchy used elsewhere
    ///     中文：该注释与英文“Profiles are looked up via the same three-tier hierarchy used elsewhere”含义一致。
    /// (project `.libra/agents/`, user `~/.config/libra/agents/`, embedded).
    ///     中文：该注释与英文“(project `.libra/agents/`, user `~/.config/libra/agents/`, embedded).”含义一致。
    #[arg(long = "agent", value_name = "NAME")]
    pub agent: Option<String>,

    /// Test-only fake provider fixture.
    ///     中文：该注释与英文“Test-only fake provider fixture.”含义一致。
    #[cfg(feature = "test-provider")]
    #[arg(long = "fake-fixture", hide = true, value_name = "PATH")]
    pub fake_fixture: Option<PathBuf>,

    /// Operating context mode (dev, review, research)
    ///     中文：该注释与英文“Operating context mode (dev, review, research)”含义一致。
    #[arg(long, value_enum)]
    pub context: Option<CodeContext>,

    /// Resume a canonical Libra thread by UUID
    ///     中文：该注释与英文“Resume a canonical Libra thread by UUID”含义一致。
    #[arg(long, value_name = "THREAD_UUID")]
    pub resume: Option<String>,

    /// Tool approval policy:
    ///     中文：该注释与英文“Tool approval policy:”含义一致。
    /// - `never`: no prompts, dangerous commands are rejected
    ///     中文：列表项说明与英文“`never`: no prompts, dangerous commands are rejected”含义一致。
    /// - `allow-all`: no prompts, all commands are allowed for this session
    ///     中文：列表项说明与英文“`allow-all`: no prompts, all commands are allowed for this session”含义一致。
    /// - `on-failure`: prompt only for retry outside sandbox after sandbox denial
    ///     中文：列表项说明与英文“`on-failure`: prompt only for retry outside sandbox after sandbox denial”含义一致。
    /// - `on-request`: run sandboxed by default; prompt for escalation/policy-required cases
    ///     中文：列表项说明与英文“`on-request`: run sandboxed by default; prompt for escalation/policy-required cases”含义一致。
    /// - `untrusted`: prompt for non-trusted operations, auto-allow known-safe reads
    ///     中文：列表项说明与英文“`untrusted`: prompt for non-trusted operations, auto-allow known-safe reads”含义一致。
    #[arg(long, value_enum, default_value_t = CodeApprovalPolicy::OnRequest)]
    pub approval_policy: CodeApprovalPolicy,

    /// Seconds that a TTL approval remains reusable for matching commands.
    ///     中文：该注释与英文“Seconds that a TTL approval remains reusable for matching commands.”含义一致。
    #[arg(long = "approval-ttl", value_name = "SECS")]
    pub approval_ttl: Option<u64>,

    /// Network access policy for TUI shell and gate execution.
    ///     中文：该注释与英文“Network access policy for TUI shell and gate execution.”含义一致。
    #[arg(long, value_enum, default_value_t = CodeNetworkAccess::Deny)]
    pub network_access: CodeNetworkAccess,

    /// Port for the embedded MCP server to listen on
    ///     中文：该注释与英文“Port for the embedded MCP server to listen on”含义一致。
    #[arg(long, value_name = "PORT", default_value_t = DEFAULT_MCP_PORT)]
    pub mcp_port: u16,

    /// Run the MCP server over Stdio (for Claude Desktop integration)
    ///     中文：该注释与英文“Run the MCP server over Stdio (for Claude Desktop integration)”含义一致。
    #[arg(long, alias = "mcp-stdio", conflicts_with = "web_only")]
    pub stdio: bool,

    /// Provider API base URL.
    ///     中文：该注释与英文“Provider API base URL.”含义一致。
    ///
    /// For Ollama, use a local/remote daemon URL such as
    ///     中文：该注释与英文“For Ollama, use a local/remote daemon URL such as”含义一致。
    /// `http://remote-host:11434/v1`, or `https://ollama.com` for direct
    ///     中文：该注释与英文“`http://remote-host:11434/v1`, or `https://ollama.com` for direct”含义一致。
    /// Ollama Cloud API access with `OLLAMA_API_KEY`.
    ///     中文：该注释与英文“Ollama Cloud API access with `OLLAMA_API_KEY`.”含义一致。
    #[arg(long, value_name = "URL")]
    pub api_base: Option<String>,

    /// Codex executable used to launch the managed app-server
    ///     中文：该注释与英文“Codex executable used to launch the managed app-server”含义一致。
    #[arg(long, value_name = "PATH", default_value = DEFAULT_CODEX_BIN)]
    pub codex_bin: String,

    /// Override the Codex app-server port (default: random local free port)
    ///     中文：该注释与英文“Override the Codex app-server port (default: random local free port)”含义一致。
    #[arg(long, value_name = "PORT")]
    pub codex_port: Option<u16>,

    /// Codex plan-first mode: require an approved plan before execution.
    ///     中文：该注释与英文“Codex plan-first mode: require an approved plan before execution.”含义一致。
    ///
    /// When `--provider=codex`, this defaults to ON so the session
    ///     中文：该注释与英文“When `--provider=codex`, this defaults to ON so the session”含义一致。
    /// follows `docs/agent/agent-workflow.md` Phase 0/1 (read-only intent &
    ///     中文：该注释与英文“follows `docs/agent/agent-workflow.md` Phase 0/1 (read-only intent &”含义一致。
    /// plan drafting) before Phase 2 execution. Pass `--plan-mode=false` to
    ///     中文：该注释与英文“plan drafting) before Phase 2 execution. Pass `--plan-mode=false` to”含义一致。
    /// opt out for a single session. For non-Codex providers, omit the flag —
    ///     中文：该注释与英文“opt out for a single session. For non-Codex providers, omit the flag —”含义一致。
    /// Libra drives Phase 0/1 through its own tool loop.
    ///     中文：该注释与英文“Libra drives Phase 0/1 through its own tool loop.”含义一致。
    ///
    /// Accepted forms:
    ///     中文：该注释与英文“Accepted forms:”含义一致。
    /// `--plan-mode` (alias for `=true`), `--plan-mode=true`, `--plan-mode=false`.
    ///     中文：该注释与英文“`--plan-mode` (alias for `=true`), `--plan-mode=true`, `--plan-mode=false`.”含义一致。
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub plan_mode: Option<bool>,

    /// Goal-mode objective. When set, the session boots with an
    ///     中文：该注释与英文“Goal-mode objective. When set, the session boots with an”含义一致。
    /// active Goal whose objective is the supplied string; the
    ///     中文：该注释与英文“active Goal whose objective is the supplied string; the”含义一致。
    /// supervisor (P6.3) drives the tool loop until completion is
    ///     中文：该注释与英文“supervisor (P6.3) drives the tool loop until completion is”含义一致。
    /// claimed and the verifier (P6.2) accepts. Equivalent to
    ///     中文：该注释与英文“claimed and the verifier (P6.2) accepts. Equivalent to”含义一致。
    /// invoking `/goal start <objective>` immediately after the
    ///     中文：该注释与英文“invoking `/goal start <objective>` immediately after the”含义一致。
    /// session opens.
    ///     中文：该注释与英文“session opens.”含义一致。
    ///
    /// The objective is validated up-front against the same shape
    ///     中文：该注释与英文“The objective is validated up-front against the same shape”含义一致。
    /// rules `GoalSpec::new` applies — non-empty after trim, ≤ 16
    ///     中文：该注释与英文“rules `GoalSpec::new` applies — non-empty after trim, ≤ 16”含义一致。
    /// KiB. A bad objective fails CLI parsing rather than crashing
    ///     中文：该注释与英文“KiB. A bad objective fails CLI parsing rather than crashing”含义一致。
    /// the supervisor at startup.
    ///     中文：该注释与英文“the supervisor at startup.”含义一致。
    #[arg(long = "goal", value_name = "OBJECTIVE")]
    pub goal: Option<String>,
}

/// Resolves the effective `plan_mode` flag for the current invocation.
///     中文：该注释与英文“Resolves the effective `plan_mode` flag for the current invocation.”含义一致。
///
/// Returns the user-supplied value when present; otherwise defaults to
///     中文：该注释与英文“Returns the user-supplied value when present; otherwise defaults to”含义一致。
/// `true` for the Codex provider and `false` for other providers.
///     中文：该注释与英文“`true` for the Codex provider and `false` for other providers.”含义一致。
///
/// **Scope of enforcement:** `plan_mode` is forwarded to Codex's
///     中文：列表项说明与英文“*Scope of enforcement:** `plan_mode` is forwarded to Codex's”含义一致。
/// `developerInstructions` / `baseInstructions` and tells Codex's own agent
///     中文：该注释与英文“`developerInstructions` / `baseInstructions` and tells Codex's own agent”含义一致。
/// loop to produce a structured plan and wait for an approval before
///     中文：该注释与英文“loop to produce a structured plan and wait for an approval before”含义一致。
/// executing. The approval gate is therefore **Codex's own approval channel**
///     中文：该注释与英文“executing. The approval gate is therefore **Codex's own approval channel**”含义一致。
/// (per-tool / per-command requests), not Libra's Phase 0 / Phase 1 review
///     中文：该注释与英文“(per-tool / per-command requests), not Libra's Phase 0 / Phase 1 review”含义一致。
/// loop. Libra's own intent / plan drafting tool loop (`phase0_plan_tool_loop_config` /
///     中文：该注释与英文“loop. Libra's own intent / plan drafting tool loop (`phase0_plan_tool_loop_config` /”含义一致。
/// `phase1_plan_tool_loop_config` in `src/internal/tui/app.rs`) requires a
///     中文：该注释与英文“`phase1_plan_tool_loop_config` in `src/internal/tui/app.rs`) requires a”含义一致。
/// generic `CompletionModel` and is bypassed when `managed_code_ui_runtime`
///     中文：该注释与英文“generic `CompletionModel` and is bypassed when `managed_code_ui_runtime`”含义一致。
/// is set (the Codex runtime is a managed backend, not a completion model —
///     中文：该注释与英文“is set (the Codex runtime is a managed backend, not a completion model —”含义一致。
/// see the bypass at `src/internal/tui/app.rs` near
///     中文：该注释与英文“see the bypass at `src/internal/tui/app.rs` near”含义一致。
/// `if self.managed_code_ui_runtime.is_none() && should_route_plain_message_to_plan(...)`).
///     中文：该注释与英文“`if self.managed_code_ui_runtime.is_none() && should_route_plain_message_to_plan(...)`).”含义一致。
///
/// Combining `--plan-mode=true` with `--approval-policy=allow-all` /
///     中文：该注释与英文“Combining `--plan-mode=true` with `--approval-policy=allow-all` /”含义一致。
/// `=never` means Codex still produces the plan, but its approval gate is
///     中文：该注释与英文“`=never` means Codex still produces the plan, but its approval gate is”含义一致。
/// auto-approved — the operator sees the plan in the transcript / log but
///     中文：该注释与英文“auto-approved — the operator sees the plan in the transcript / log but”含义一致。
/// is never asked to confirm. `start_codex_code_ui_runtime` emits a
///     中文：该注释与英文“is never asked to confirm. `start_codex_code_ui_runtime` emits a”含义一致。
/// `tracing::warn!` when this combination is detected so the operator can
///     中文：该注释与英文“`tracing::warn!` when this combination is detected so the operator can”含义一致。
/// notice that the review gate has been disabled.
///     中文：该注释与英文“notice that the review gate has been disabled.”含义一致。
pub(crate) fn effective_plan_mode(args: &CodeArgs) -> bool {
    args.plan_mode
        .unwrap_or(matches!(args.provider, CodeProvider::Codex))
}

// ---------------------------------------------------------------------------
// Top-level entry point — mode dispatch
// 中文：该注释与英文“Top-level entry point — mode dispatch”含义一致。
// ---------------------------------------------------------------------------

/// Entry point for the `libra code` subcommand.
///     中文：该注释与英文“Entry point for the `libra code` subcommand.”含义一致。
///
/// Validates CLI flag combinations, then dispatches to one of three mode-specific
///     中文：该注释与英文“Validates CLI flag combinations, then dispatches to one of three mode-specific”含义一致。
/// execution paths: stdio (MCP over stdin/stdout), web-only (headless HTTP servers),
///     中文：该注释与英文“execution paths: stdio (MCP over stdin/stdout), web-only (headless HTTP servers),”含义一致。
/// or TUI (full interactive terminal with background servers).
///     中文：该注释与英文“or TUI (full interactive terminal with background servers).”含义一致。
///
/// # Side Effects
///     中文：标题：Side Effects。
/// - May start local web, MCP, and Codex app-server processes depending on mode.
///     中文：列表项说明与英文“May start local web, MCP, and Codex app-server processes depending on mode.”含义一致。
/// - May create `.libra/objects` and connect to `.libra/libra.db` for history.
///     中文：列表项说明与英文“May create `.libra/objects` and connect to `.libra/libra.db` for history.”含义一致。
/// - In TUI mode, may mutate the workspace through registered tools, subject to
///     中文：列表项说明与英文“In TUI mode, may mutate the workspace through registered tools, subject to”含义一致。
///   sandbox and approval policy.
///     中文：该注释与英文“sandbox and approval policy.”含义一致。
/// - In stdio mode, owns stdin/stdout for the MCP session.
///     中文：列表项说明与英文“In stdio mode, owns stdin/stdout for the MCP session.”含义一致。
///
/// # Errors
///     中文：标题：Errors。
/// Returns [`CliError`] for invalid mode combinations, provider credential
///     中文：该注释与英文“Returns [`CliError`] for invalid mode combinations, provider credential”含义一致。
/// failures, network bind failures, Codex app-server startup failures, or
///     中文：该注释与英文“failures, network bind failures, Codex app-server startup failures, or”含义一致。
/// terminal/session initialization failures. Error classification follows
///     中文：该注释与英文“terminal/session initialization failures. Error classification follows”含义一致。
/// `docs/development/cli-error-contract-design.md`.
///     中文：该注释与英文“`docs/development/cli-error-contract-design.md`.”含义一致。
pub async fn execute(args: CodeArgs, output: &OutputConfig) -> CliResult<()> {
    validate_mode_args(&args, output).map_err(CliError::command_usage)?;
    if args.stdio {
        execute_stdio(&args).await
    } else if args.web_only {
        execute_web_only(&args).await
    } else {
        execute_tui(args).await
    }
}

// ---------------------------------------------------------------------------
// Server handles — RAII wrappers for graceful shutdown
// 中文：该注释与英文“Server handles — RAII wrappers for graceful shutdown”含义一致。
// ---------------------------------------------------------------------------

/// Handle to a running MCP server.
///     中文：该注释与英文“Handle to a running MCP server.”含义一致。
///
/// In addition to the shared shutdown mechanism, this tracks individual
///     中文：该注释与英文“In addition to the shared shutdown mechanism, this tracks individual”含义一致。
/// per-connection tasks so they can be aborted during shutdown — preventing
///     中文：该注释与英文“per-connection tasks so they can be aborted during shutdown — preventing”含义一致。
/// leaked tasks when the server is torn down.
///     中文：该注释与英文“leaked tasks when the server is torn down.”含义一致。
struct McpServerHandle {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
    /// Tracks spawned per-connection Hyper service tasks for cleanup.
    ///     中文：该注释与英文“Tracks spawned per-connection Hyper service tasks for cleanup.”含义一致。
    connection_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl McpServerHandle {
    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.join.await;
        let pending = match self.connection_tasks.lock() {
            Ok(mut handles) => std::mem::take(&mut *handles),
            Err(_) => Vec::new(),
        };
        for handle in pending {
            handle.abort();
            let _ = handle.await;
        }
    }
}

// ---------------------------------------------------------------------------
// Mode: Web-only — headless web + MCP servers (no TUI)
// 中文：该注释与英文“Mode: Web-only — headless web + MCP servers (no TUI)”含义一致。
// ---------------------------------------------------------------------------

/// Runs the web server and MCP server without a terminal UI.
///     中文：该注释与英文“Runs the web server and MCP server without a terminal UI.”含义一致。
///
/// Blocks on `Ctrl-C`, then performs graceful shutdown of both servers.
///     中文：该注释与英文“Blocks on `Ctrl-C`, then performs graceful shutdown of both servers.”含义一致。
/// This mode is useful for remote/headless environments where the user
///     中文：该注释与英文“This mode is useful for remote/headless environments where the user”含义一致。
/// interacts through a browser or external MCP client.
///     中文：该注释与英文“interacts through a browser or external MCP client.”含义一致。
///
/// # Side Effects
///     中文：标题：Side Effects。
/// - Starts the embedded web server and Streamable HTTP MCP server.
///     中文：列表项说明与英文“Starts the embedded web server and Streamable HTTP MCP server.”含义一致。
/// - For the Codex provider, starts and later shuts down a managed Codex
///     中文：列表项说明与英文“For the Codex provider, starts and later shuts down a managed Codex”含义一致。
///   app-server child process.
///     中文：该注释与英文“app-server child process.”含义一致。
/// - Prints connection details to stdout and listens for `Ctrl-C`.
///     中文：列表项说明与英文“Prints connection details to stdout and listens for `Ctrl-C`.”含义一致。
///
/// # Errors
///     中文：标题：Errors。
/// Returns [`CliError`] when the working directory cannot be resolved, the web
///     中文：该注释与英文“Returns [`CliError`] when the working directory cannot be resolved, the web”含义一致。
/// or MCP listener cannot bind, the Codex app-server fails to start, or the
///     中文：该注释与英文“or MCP listener cannot bind, the Codex app-server fails to start, or the”含义一致。
/// selected host would expose loopback-only browser control.
///     中文：该注释与英文“selected host would expose loopback-only browser control.”含义一致。
async fn execute_web_only(args: &CodeArgs) -> CliResult<()> {
    let working_dir = resolve_code_working_dir(args)?;
    let browser_control = resolve_browser_control_mode(args)?;
    let control_runtime = prepare_control_runtime(args, &working_dir).await?;
    let mcp_server = init_mcp_server(&working_dir).await;

    let mut managed_codex_server = None;
    let code_ui_runtime = if args.provider == CodeProvider::Codex {
        let server =
            start_managed_codex_server(&args.codex_bin, args.codex_port, &working_dir).await?;
        println!("Starting Libra Code Web UI with Codex provider");
        println!("Working directory: {}", working_dir.display());
        println!("Codex WebSocket: {}", server.ws_url);
        println!("Codex app-server: auto-started");
        println!("Browser control: {}", browser_control.as_str());
        managed_codex_server = Some(server);

        let ws_url = managed_codex_server
            .as_ref()
            .map(|server| server.ws_url.as_str())
            .unwrap_or_default();
        start_codex_code_ui_runtime(
            args,
            &working_dir,
            ws_url,
            mcp_server.clone(),
            browser_control == BrowserControlMode::Loopback,
            CodeUiInitialController::Unclaimed,
        )
        .await?
    } else {
        let storage_root = resolve_storage_root(&working_dir);
        let session_store = Arc::new(SessionStore::from_storage_path(&storage_root));
        let session_state =
            load_or_create_headless_web_session_state(args, &working_dir, &session_store)?;
        // Phase 3 v0 routes the supported providers through the new
        // 中文：该注释与英文“Phase 3 v0 routes the supported providers through the new”含义一致。
        // headless runtime. Anything not yet hooked up keeps the read-only
        // 中文：该注释与英文“headless runtime. Anything not yet hooked up keeps the read-only”含义一致。
        // placeholder so we fail closed rather than panicking on attach.
        // 中文：该注释与英文“placeholder so we fail closed rather than panicking on attach.”含义一致。
        match build_non_codex_headless_runtime(
            args,
            &working_dir,
            session_store,
            session_state,
            browser_control == BrowserControlMode::Loopback,
        )
        .await?
        {
            Some(runtime) => {
                println!("Starting Libra Code Web UI in headless mode");
                println!("Working directory: {}", working_dir.display());
                println!("Provider: {:?}", args.provider);
                println!("Browser control: {}", browser_control.as_str());
                runtime
            }
            None => build_placeholder_web_code_ui_runtime(args, &working_dir).await,
        }
    };
    mcp_server.set_code_ui_session(code_ui_runtime.adapter().session());

    let web_handle = match start_web_server(
        &args.host,
        args.port,
        working_dir.clone(),
        WebServerOptions {
            code_ui: Some(code_ui_runtime.clone()),
            automation_control_token: control_runtime.token.clone(),
            audit_sink: None,
        },
    )
    .await
    {
        Ok(handle) => handle,
        Err(err) => {
            let _ = code_ui_runtime.shutdown().await;
            if let Some(server) = managed_codex_server.as_mut() {
                server.shutdown().await;
            }
            return Err(
                CliError::network(format!("failed to start web server: {err}"))
                    .with_detail("component", "web_server"),
            );
        }
    };
    let base_url = format!("http://{}", web_handle.addr);
    let thread_id = code_ui_runtime.snapshot().await.thread_id;
    if let Err(error) =
        control_runtime.write_info_file(&working_dir, base_url.clone(), None, thread_id.clone())
    {
        let _ = code_ui_runtime.shutdown().await;
        if let Some(server) = managed_codex_server.as_mut() {
            server.shutdown().await;
        }
        web_handle.shutdown().await;
        return Err(error);
    }
    println!("Libra Code server running at {base_url}");

    // Start MCP Server
    // 中文：该注释与英文“Start MCP Server”含义一致。
    let mcp_handle = match start_mcp_server(&args.host, args.mcp_port, mcp_server.clone()).await {
        Ok(handle) => {
            let mcp_url = format!("http://{}", handle.addr);
            if let Err(error) = control_runtime.write_info_file(
                &working_dir,
                base_url.clone(),
                Some(mcp_url.clone()),
                thread_id.clone(),
            ) {
                let _ = code_ui_runtime.shutdown().await;
                if let Some(server) = managed_codex_server.as_mut() {
                    server.shutdown().await;
                }
                web_handle.shutdown().await;
                handle.shutdown().await;
                return Err(error);
            }
            println!("MCP: {mcp_url}");
            handle
        }
        Err(err) => {
            let _ = code_ui_runtime.shutdown().await;
            if let Some(server) = managed_codex_server.as_mut() {
                server.shutdown().await;
            }
            web_handle.shutdown().await;
            return Err(
                CliError::network(format!("failed to start MCP server: {err}"))
                    .with_detail("component", "mcp_server"),
            );
        }
    };

    let _ = tokio::signal::ctrl_c().await;
    let _ = code_ui_runtime.shutdown().await;
    web_handle.shutdown().await;
    mcp_handle.shutdown().await;
    if let Some(server) = managed_codex_server.as_mut() {
        server.shutdown().await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode: TUI — full interactive terminal with background servers
// 中文：该注释与英文“Mode: TUI — full interactive terminal with background servers”含义一致。
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct CodeEnvFile {
    values: BTreeMap<String, String>,
}

impl CodeEnvFile {
    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

fn load_code_env_file(path: Option<&Path>) -> CliResult<CodeEnvFile> {
    let Some(path) = path else {
        return Ok(CodeEnvFile::default());
    };

    let contents = fs::read_to_string(path).map_err(|error| {
        CliError::io(format!(
            "failed to read --env-file {}: {error}",
            path.display()
        ))
    })?;
    parse_code_env_file(&contents, path).map_err(CliError::command_usage)
}

fn parse_code_env_file(contents: &str, path: &Path) -> Result<CodeEnvFile, String> {
    let mut values = BTreeMap::new();
    for (index, raw_line) in contents.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "{}:{line_no}: expected KEY=VALUE entry",
                path.display()
            ));
        };
        let key = key.trim();
        if !is_valid_env_key(key) {
            return Err(format!(
                "{}:{line_no}: invalid environment variable name `{key}`",
                path.display()
            ));
        }

        let value = parse_env_file_value(value).map_err(|message| {
            format!(
                "{}:{line_no}: invalid value for `{key}`: {message}",
                path.display()
            )
        })?;
        values.insert(key.to_string(), value);
    }

    Ok(CodeEnvFile { values })
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn parse_env_file_value(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(String::new());
    }

    let first = value.as_bytes()[0];
    match first {
        b'\'' | b'"' => {
            if value.as_bytes().last() != Some(&first) || value.len() < 2 {
                return Err("quoted values must end with the matching quote".to_string());
            }
            let inner = &value[1..value.len() - 1];
            if first == b'"' {
                parse_double_quoted_env_value(inner)
            } else {
                Ok(inner.to_string())
            }
        }
        _ => Ok(strip_inline_env_comment(value).trim_end().to_string()),
    }
}

fn parse_double_quoted_env_value(value: &str) -> Result<String, String> {
    let mut parsed = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            parsed.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            return Err("trailing backslash in quoted value".to_string());
        };
        match escaped {
            'n' => parsed.push('\n'),
            'r' => parsed.push('\r'),
            't' => parsed.push('\t'),
            '\\' => parsed.push('\\'),
            '"' => parsed.push('"'),
            other => parsed.push(other),
        }
    }
    Ok(parsed)
}

fn strip_inline_env_comment(value: &str) -> &str {
    for (index, ch) in value.char_indices() {
        if ch == '#' && (index == 0 || value[..index].ends_with(char::is_whitespace)) {
            return &value[..index];
        }
    }
    value
}

fn provider_env_value_with_lookup(
    env_file: &CodeEnvFile,
    key: &str,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    env_file
        .get(key)
        .map(str::to_string)
        .or_else(|| lookup(key))
}

/// Build an [`AnyCompletionModel`] for every non-Codex provider through the
///     中文：该注释与英文“Build an [`AnyCompletionModel`] for every non-Codex provider through the”含义一致。
/// shared [`ProviderFactory`].
///     中文：该注释与英文“shared [`ProviderFactory`].”含义一致。
///
/// This consolidates what used to be eight near-identical match arms
///     中文：该注释与英文“This consolidates what used to be eight near-identical match arms”含义一致。
/// (`Gemini`, `Openai`, `Anthropic`, `Deepseek`, `Kimi`, `Zhipu`, `Ollama`,
///     中文：该注释与英文“(`Gemini`, `Openai`, `Anthropic`, `Deepseek`, `Kimi`, `Zhipu`, `Ollama`,”含义一致。
/// `Fake`) into a single dispatch. The Codex provider stays on its own path
///     中文：该注释与英文“`Fake`) into a single dispatch. The Codex provider stays on its own path”含义一致。
/// because it bypasses `AnyCompletionModel` entirely (managed app-server
///     中文：该注释与英文“because it bypasses `AnyCompletionModel` entirely (managed app-server”含义一致。
/// runtime).
///     中文：该注释与英文“runtime).”含义一致。
///
/// Env resolution flows through [`provider_env_value_with_lookup`] for
///     中文：该注释与英文“Env resolution flows through [`provider_env_value_with_lookup`] for”含义一致。
/// **every** provider, not just Deepseek / Kimi as before. The precedence is
///     中文：列表项说明与英文“*every** provider, not just Deepseek / Kimi as before. The precedence is”含义一致。
/// `--env-file` first then process env (documented on `--env-file` itself),
///     中文：该注释与英文“`--env-file` first then process env (documented on `--env-file` itself),”含义一致。
/// and applies to API keys, base URLs, and the boolean `OLLAMA_COMPACT_TOOLS`
///     中文：该注释与英文“and applies to API keys, base URLs, and the boolean `OLLAMA_COMPACT_TOOLS`”含义一致。
/// flag. Gemini / OpenAI / Anthropic / Zhipu used to read only from process
///     中文：该注释与英文“flag. Gemini / OpenAI / Anthropic / Zhipu used to read only from process”含义一致。
/// env via `from_env()`; this widens them to consult `--env-file` first as
///     中文：该注释与英文“env via `from_env()`; this widens them to consult `--env-file` first as”含义一致。
/// well, so a value defined in the env-file now wins over a stale process-env
///     中文：该注释与英文“well, so a value defined in the env-file now wins over a stale process-env”含义一致。
/// value for those providers.
///     中文：该注释与英文“value for those providers.”含义一致。
///
/// The function returns the resolved model name AND the effective provider
///     中文：该注释与英文“The function returns the resolved model name AND the effective provider”含义一致。
/// name string so the caller can tag usage / UI metadata against the agent's
///     中文：该注释与英文“name string so the caller can tag usage / UI metadata against the agent's”含义一致。
/// chosen provider (which may differ from `--provider` after an `--agent`
///     中文：该注释与英文“chosen provider (which may differ from `--provider` after an `--agent`”含义一致。
/// override).
///     中文：该注释与英文“override).”含义一致。
///
/// OC-Phase 2 P2.4 added the `--agent <name>` override path. When the flag
///     中文：该注释与英文“OC-Phase 2 P2.4 added the `--agent <name>` override path. When the flag”含义一致。
/// is set the helper loads the profile via the same three-tier hierarchy
///     中文：该注释与英文“is set the helper loads the profile via the same three-tier hierarchy”含义一致。
/// the runtime uses, asserts the agent is primary-eligible, and — if the
///     中文：该注释与英文“the runtime uses, asserts the agent is primary-eligible, and — if the”含义一致。
/// profile carries a structured `model: provider/model[@variant]` binding —
///     中文：该注释与英文“profile carries a structured `model: provider/model[@variant]` binding —”含义一致。
/// uses that binding **atomically**: provider id, model id, and variant all
///     中文：该注释与英文“uses that binding **atomically**: provider id, model id, and variant all”含义一致。
/// come from the agent's spec. A separately-supplied `--model` is **ignored**
///     中文：该注释与英文“come from the agent's spec. A separately-supplied `--model` is **ignored**”含义一致。
/// when the binding wins, since mixing an explicit model id with the agent's
///     中文：该注释与英文“when the binding wins, since mixing an explicit model id with the agent's”含义一致。
/// provider can produce nonsense pairs (e.g. anthropic provider with an
///     中文：该注释与英文“provider can produce nonsense pairs (e.g. anthropic provider with an”含义一致。
/// OpenAI-shaped model id). When the agent profile does NOT carry a binding,
///     中文：该注释与英文“OpenAI-shaped model id). When the agent profile does NOT carry a binding,”含义一致。
/// the CLI defaults stand verbatim.
///     中文：该注释与英文“the CLI defaults stand verbatim.”含义一致。
fn build_any_completion_model_for_args(
    args: &CodeArgs,
    env_file: &CodeEnvFile,
    working_dir: &std::path::Path,
) -> CliResult<(
    crate::internal::ai::providers::AnyCompletionModel,
    String,
    String,
)> {
    build_any_completion_model_for_args_with_lookup(args, env_file, working_dir, |key| {
        // Vault-aware fallback chain: try process env first (cheap), then
        // 中文：该注释与英文“Vault-aware fallback chain: try process env first (cheap), then”含义一致。
        // fall back to the libra config DB (repo-local + global
        // 中文：该注释与英文“fall back to the libra config DB (repo-local + global”含义一致。
        // `vault.env.<name>`) via the sync resolver. Phase 5 from_env →
        // 中文：该注释与英文“`vault.env.<name>`) via the sync resolver. Phase 5 from_env →”含义一致。
        // resolve_env call-site cutover: users who configured an API key
        // 中文：该注释与英文“resolve_env call-site cutover: users who configured an API key”含义一致。
        // once via `libra config --global add vault.env.GEMINI_API_KEY <…>`
        // 中文：该注释与英文“once via `libra config --global add vault.env.GEMINI_API_KEY <…>`”含义一致。
        // no longer need to re-export it in every shell.
        // 中文：该注释与英文“no longer need to re-export it in every shell.”含义一致。
        //
        // The DB read may fail (e.g. stale global config schema); we treat
        // 中文：该注释与英文“The DB read may fail (e.g. stale global config schema); we treat”含义一致。
        // any error as "value not present" here so the provider bootstrap
        // 中文：该注释与英文“any error as "value not present" here so the provider bootstrap”含义一致。
        // path falls through to its existing "API key not set" error,
        // 中文：该注释与英文“path falls through to its existing "API key not set" error,”含义一致。
        // matching the v0.17.534 fallback semantics. Hard schema-mismatch
        // 中文：该注释与英文“matching the v0.17.534 fallback semantics. Hard schema-mismatch”含义一致。
        // chains are still surfaced via `tracing::warn!` inside
        // 中文：该注释与英文“chains are still surfaced via `tracing::warn!` inside”含义一致。
        // `resolve_env_for_target`.
        // 中文：该注释与英文“`resolve_env_for_target`.”含义一致。
        match crate::internal::config::resolve_env_sync(key) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    key = key,
                    error = %format!("{error:#}"),
                    "vault-aware env resolution failed; falling back to None"
                );
                None
            }
        }
    })
}

fn build_any_completion_model_for_args_with_lookup(
    args: &CodeArgs,
    env_file: &CodeEnvFile,
    working_dir: &std::path::Path,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> CliResult<(
    crate::internal::ai::providers::AnyCompletionModel,
    String,
    String,
)> {
    use crate::internal::ai::{
        agent::profile::ModelBinding,
        providers::{
            ProviderBuildOptions, ProviderFactory, ProviderFactoryError, runtime::provider_id,
        },
    };

    // 1. Map `--provider` to the canonical provider id string (the factory's
    // 中文：该注释与英文“1. Map `--provider` to the canonical provider id string (the factory's”含义一致。
    //    dispatch key). Codex bypasses this helper entirely.
    // 中文：该注释与英文“dispatch key). Codex bypasses this helper entirely.”含义一致。
    let mut provider_id_str = match args.provider {
        CodeProvider::Gemini => provider_id::GEMINI.to_string(),
        CodeProvider::Openai => provider_id::OPENAI.to_string(),
        CodeProvider::Anthropic => provider_id::ANTHROPIC.to_string(),
        CodeProvider::Deepseek => provider_id::DEEPSEEK.to_string(),
        CodeProvider::Kimi => provider_id::KIMI.to_string(),
        CodeProvider::Zhipu => provider_id::ZHIPU.to_string(),
        CodeProvider::Ollama => provider_id::OLLAMA.to_string(),
        #[cfg(feature = "test-provider")]
        CodeProvider::Fake => provider_id::FAKE.to_string(),
        CodeProvider::Codex => {
            // Codex never reaches this helper — its dispatch path skips the
            // 中文：该注释与英文“Codex never reaches this helper — its dispatch path skips the”含义一致。
            // factory entirely. Treat as a programmer error rather than a
            // 中文：该注释与英文“factory entirely. Treat as a programmer error rather than a”含义一致。
            // runtime failure so a future refactor cannot silently misroute.
            // 中文：该注释与英文“runtime failure so a future refactor cannot silently misroute.”含义一致。
            return Err(CliError::command_usage(
                "internal error: Codex provider must use the managed runtime path, \
                 not the completion-model factory",
            ));
        }
    };

    // 2. Resolve the default model id from the CLI provider. Ollama errors
    // 中文：该注释与英文“2. Resolve the default model id from the CLI provider. Ollama errors”含义一致。
    //    if `--model` is omitted (no sensible local default); the rest fall
    // 中文：该注释与英文“if `--model` is omitted (no sensible local default); the rest fall”含义一致。
    //    back to a flagship model constant. Honored only when the agent
    // 中文：该注释与英文“back to a flagship model constant. Honored only when the agent”含义一致。
    //    override does not supply a binding model id below.
    // 中文：该注释与英文“override does not supply a binding model id below.”含义一致。
    let cli_default_model = |provider: CodeProvider| -> CliResult<String> {
        Ok(match provider {
            CodeProvider::Gemini => GEMINI_2_5_FLASH.to_string(),
            CodeProvider::Openai => GPT_4O_MINI.to_string(),
            CodeProvider::Anthropic => CLAUDE_3_5_SONNET.to_string(),
            CodeProvider::Deepseek => "deepseek-chat".to_string(),
            CodeProvider::Kimi => KIMI_K2_6.to_string(),
            CodeProvider::Zhipu => GLM_5.to_string(),
            CodeProvider::Ollama => {
                return Err(CliError::command_usage(
                    "--model is required when using --provider ollama \
                     (e.g. --model llama3.2)",
                ));
            }
            #[cfg(feature = "test-provider")]
            CodeProvider::Fake => FAKE_DEFAULT_MODEL.to_string(),
            CodeProvider::Codex => unreachable!("Codex filtered above"),
        })
    };

    let mut variant: Option<String> = None;
    // 3. OC-Phase 2 P2.4: apply `--agent <name>` override atomically.
    // 中文：该注释与英文“3. OC-Phase 2 P2.4: apply `--agent <name>` override atomically.”含义一致。
    //    When the profile carries a structured binding, all three of
    // 中文：该注释与英文“When the profile carries a structured binding, all three of”含义一致。
    //    (provider_id, model_id, variant) come from the spec — `--model`
    // 中文：该注释与英文“(provider_id, model_id, variant) come from the spec — `--model`”含义一致。
    //    is ignored to avoid hybrid pairs like "anthropic + gpt-4o".
    // 中文：该注释与英文“is ignored to avoid hybrid pairs like "anthropic + gpt-4o".”含义一致。
    let agent_binding = resolve_agent_binding_override(args, working_dir)?;
    let model_name: String = if let Some(binding) = agent_binding {
        provider_id_str = binding.provider_id;
        variant = binding.variant;
        binding.model_id
    } else {
        match args.model.clone() {
            Some(m) => m,
            None => cli_default_model(args.provider)?,
        }
    };

    // 4. Resolve API key / base URL by provider id (string-keyed so the
    // 中文：该注释与英文“4. Resolve API key / base URL by provider id (string-keyed so the”含义一致。
    //    agent override flows through to env-var lookup).
    // 中文：该注释与英文“agent override flows through to env-var lookup).”含义一致。
    let resolve_env = |key: &str| provider_env_value_with_lookup(env_file, key, &env_lookup);

    let api_key = match provider_id_str.as_str() {
        provider_id::GEMINI => resolve_env("GEMINI_API_KEY"),
        provider_id::OPENAI => resolve_env("OPENAI_API_KEY"),
        provider_id::ANTHROPIC => resolve_env("ANTHROPIC_API_KEY"),
        provider_id::DEEPSEEK => resolve_env("DEEPSEEK_API_KEY"),
        provider_id::KIMI => resolve_env("MOONSHOT_API_KEY"),
        provider_id::ZHIPU => resolve_env("ZHIPU_API_KEY"),
        provider_id::OLLAMA => resolve_env("OLLAMA_API_KEY"),
        #[cfg(feature = "test-provider")]
        provider_id::FAKE => None,
        _ => None,
    };

    let cli_api_base = args.api_base.clone();
    let api_base = match provider_id_str.as_str() {
        provider_id::ANTHROPIC => cli_api_base.or_else(|| resolve_env("ANTHROPIC_BASE_URL")),
        provider_id::OPENAI => cli_api_base.or_else(|| resolve_env("OPENAI_BASE_URL")),
        provider_id::DEEPSEEK => cli_api_base,
        provider_id::GEMINI => cli_api_base,
        provider_id::KIMI => cli_api_base.or_else(|| resolve_env("MOONSHOT_BASE_URL")),
        provider_id::ZHIPU => cli_api_base.or_else(|| resolve_env("ZHIPU_BASE_URL")),
        provider_id::OLLAMA => cli_api_base.or_else(|| resolve_env("OLLAMA_BASE_URL")),
        _ => None,
    };

    #[cfg(feature = "test-provider")]
    let fake_fixture_path = if provider_id_str == provider_id::FAKE {
        Some(args.fake_fixture.clone().ok_or_else(|| {
            CliError::command_usage("--fake-fixture is required with --provider=fake")
        })?)
    } else {
        None
    };
    #[cfg(not(feature = "test-provider"))]
    let fake_fixture_path: Option<std::path::PathBuf> = None;

    // The Ollama client used to read `OLLAMA_COMPACT_TOOLS` from process env
    // 中文：该注释与英文“The Ollama client used to read `OLLAMA_COMPACT_TOOLS` from process env”含义一致。
    // at construction time. The factory now sets the flag explicitly, so we
    // 中文：该注释与英文“at construction time. The factory now sets the flag explicitly, so we”含义一致。
    // need to fold that env var back in when the CLI flag is absent —
    // 中文：该注释与英文“need to fold that env var back in when the CLI flag is absent —”含义一致。
    // otherwise users with `OLLAMA_COMPACT_TOOLS=1` in their environment
    // 中文：该注释与英文“otherwise users with `OLLAMA_COMPACT_TOOLS=1` in their environment”含义一致。
    // would silently lose compact-schema mode after this migration.
    // 中文：该注释与英文“would silently lose compact-schema mode after this migration.”含义一致。
    let ollama_compact_tools = args.ollama_compact_tools
        || resolve_env("OLLAMA_COMPACT_TOOLS")
            .map(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);

    let options = ProviderBuildOptions {
        api_key,
        api_base,
        ollama_compact_tools,
        fake_fixture_path,
        // Preserve the pre-factory behaviour of accepting any model string
        // 中文：该注释与英文“Preserve the pre-factory behaviour of accepting any model string”含义一致。
        // the user passes via `--model`. The capability table is best-effort
        // 中文：该注释与英文“the user passes via `--model`. The capability table is best-effort”含义一致。
        // and the runtime will surface a real provider error if the model
        // 中文：该注释与英文“and the runtime will surface a real provider error if the model”含义一致。
        // does not exist.
        // 中文：该注释与英文“does not exist.”含义一致。
        accept_unknown_models: true,
    };

    let binding = ModelBinding {
        provider_id: provider_id_str.clone(),
        model_id: model_name.clone(),
        variant,
    };

    let model = ProviderFactory
        .build(&binding, options)
        .map_err(|err| match err {
            ProviderFactoryError::MissingApiKey { env_var, .. } => {
                if provider_id_str == provider_id::OLLAMA {
                    // Ollama Cloud needs the api key only when the base URL points
                    // 中文：该注释与英文“Ollama Cloud needs the api key only when the base URL points”含义一致。
                    // at ollama.com; preserve the pre-factory error wording so users
                    // 中文：该注释与英文“at ollama.com; preserve the pre-factory error wording so users”含义一致。
                    // who scripted against it do not see a regression.
                    // 中文：该注释与英文“who scripted against it do not see a regression.”含义一致。
                    CliError::auth(
                        "OLLAMA_API_KEY is required when using Ollama Cloud directly \
                     (set --api-base https://ollama.com or OLLAMA_BASE_URL=https://ollama.com)",
                    )
                } else {
                    CliError::auth(format!("{env_var} is not set"))
                }
            }
            ProviderFactoryError::BuildFailed { reason, .. } => CliError::io(reason),
            ProviderFactoryError::UnknownProvider { .. }
            | ProviderFactoryError::UnknownModel { .. } => CliError::command_usage(err.to_string()),
        })?;

    Ok((model, model_name, provider_id_str))
}

/// Resolve the **effective** [`CodeProvider`] enum that downstream
///     中文：该注释与英文“Resolve the **effective** [`CodeProvider`] enum that downstream”含义一致。
/// provider-specific helpers should dispatch on (OC-Phase 2 P2.4).
///     中文：该注释与英文“provider-specific helpers should dispatch on (OC-Phase 2 P2.4).”含义一致。
///
/// When `--agent <name>` is set and the agent's profile carries a structured
///     中文：该注释与英文“When `--agent <name>` is set and the agent's profile carries a structured”含义一致。
/// `model: provider/model` binding, the effective provider is the one named
///     中文：该注释与英文“`model: provider/model` binding, the effective provider is the one named”含义一致。
/// by the binding's `provider_id`. Otherwise the effective provider is the
///     中文：该注释与英文“by the binding's `provider_id`. Otherwise the effective provider is the”含义一致。
/// CLI `--provider` default.
///     中文：该注释与英文“CLI `--provider` default.”含义一致。
///
/// An agent binding whose `provider_id` does NOT map to a known
///     中文：该注释与英文“An agent binding whose `provider_id` does NOT map to a known”含义一致。
/// [`CodeProvider`] variant is rejected with a `command_usage` error.
///     中文：该注释与英文“[`CodeProvider`] variant is rejected with a `command_usage` error.”含义一致。
/// Silently falling back to `args.provider` would leave the system prompt /
///     中文：该注释与英文“Silently falling back to `args.provider` would leave the system prompt /”含义一致。
/// context-budget / completion knobs computed against the CLI provider
///     中文：该注释与英文“context-budget / completion knobs computed against the CLI provider”含义一致。
/// while the model is ultimately built for a different (or non-existent)
///     中文：该注释与英文“while the model is ultimately built for a different (or non-existent)”含义一致。
/// provider — a partial-misconfiguration trap. The list of known provider
///     中文：该注释与英文“provider — a partial-misconfiguration trap. The list of known provider”含义一致。
/// ids stays in lock-step with [`provider_id::ALL_PRODUCTION`] (plus
///     中文：该注释与英文“ids stays in lock-step with [`provider_id::ALL_PRODUCTION`] (plus”含义一致。
/// `FAKE` under the `test-provider` feature).
///     中文：该注释与英文“`FAKE` under the `test-provider` feature).”含义一致。
fn effective_code_provider_for_args(
    args: &CodeArgs,
    working_dir: &std::path::Path,
) -> CliResult<CodeProvider> {
    use crate::internal::ai::providers::runtime::provider_id;

    let Some(binding) = resolve_agent_binding_override(args, working_dir)? else {
        return Ok(args.provider);
    };
    let mapped = match binding.provider_id.as_str() {
        provider_id::GEMINI => Some(CodeProvider::Gemini),
        provider_id::OPENAI => Some(CodeProvider::Openai),
        provider_id::ANTHROPIC => Some(CodeProvider::Anthropic),
        provider_id::DEEPSEEK => Some(CodeProvider::Deepseek),
        provider_id::KIMI => Some(CodeProvider::Kimi),
        provider_id::ZHIPU => Some(CodeProvider::Zhipu),
        provider_id::OLLAMA => Some(CodeProvider::Ollama),
        #[cfg(feature = "test-provider")]
        provider_id::FAKE => Some(CodeProvider::Fake),
        _ => None,
    };
    mapped.ok_or_else(|| {
        CliError::command_usage(format!(
            "agent '{}' selects provider '{}', which is not a known `--provider` value. \
             Pick a binding whose provider id is one of: {}",
            args.agent.as_deref().unwrap_or("?"),
            binding.provider_id,
            provider_id::ALL_PRODUCTION.join(", "),
        ))
    })
}

/// Look up the agent profile selected by `--agent <name>` and return its
///     中文：该注释与英文“Look up the agent profile selected by `--agent <name>` and return its”含义一致。
/// structured `ModelBinding` if the profile carries one (OC-Phase 2 P2.4).
///     中文：该注释与英文“structured `ModelBinding` if the profile carries one (OC-Phase 2 P2.4).”含义一致。
///
/// Returns `Ok(None)` when:
///     中文：该注释与英文“Returns `Ok(None)` when:”含义一致。
/// - `--agent` was not supplied; the helper is a no-op.
///     中文：列表项说明与英文“`--agent` was not supplied; the helper is a no-op.”含义一致。
/// - The agent exists but has no `model: provider/model` binding (legacy
///     中文：列表项说明与英文“The agent exists but has no `model: provider/model` binding (legacy”含义一致。
///   `model: default` / `fast` / etc.). The CLI defaults stand.
///     中文：该注释与英文“`model: default` / `fast` / etc.). The CLI defaults stand.”含义一致。
///
/// Returns `Err(_)` when:
///     中文：该注释与英文“Returns `Err(_)` when:”含义一致。
/// - The agent name does not match any profile in the three-tier hierarchy.
///     中文：列表项说明与英文“The agent name does not match any profile in the three-tier hierarchy.”含义一致。
/// - The agent's `mode` is not primary-eligible (sub-agents are dispatched
///     中文：列表项说明与英文“The agent's `mode` is not primary-eligible (sub-agents are dispatched”含义一致。
///   via the `task` tool in OC-Phase 3, not as the session driver).
///     中文：该注释与英文“via the `task` tool in OC-Phase 3, not as the session driver).”含义一致。
fn resolve_agent_binding_override(
    args: &CodeArgs,
    working_dir: &std::path::Path,
) -> CliResult<Option<crate::internal::ai::agent::profile::ModelBinding>> {
    let Some(agent_name) = args.agent.as_deref() else {
        return Ok(None);
    };
    let profiles = load_profiles(working_dir);
    let router = AgentProfileRouter::new(profiles);
    let spec = router.execution_spec(agent_name).ok_or_else(|| {
        let mut suggestions: Vec<&str> =
            router.profiles().iter().map(|p| p.name.as_str()).collect();
        suggestions.sort();
        let suggestion_hint = if suggestions.is_empty() {
            String::from("(no profiles loaded)")
        } else {
            format!("known agents: {}", suggestions.join(", "))
        };
        CliError::command_usage(format!(
            "unknown agent '{agent_name}' for --agent; {suggestion_hint}"
        ))
    })?;
    if !spec.mode.is_primary_eligible() {
        return Err(CliError::command_usage(format!(
            "agent '{agent_name}' has mode '{:?}', which is not primary-eligible. \
             Sub-agents are dispatched via the `task` tool, not selected with --agent.",
            spec.mode
        )));
    }
    Ok(spec.model)
}

/// Main TUI execution path: initializes the AI provider, builds the tool
///     中文：该注释与英文“Main TUI execution path: initializes the AI provider, builds the tool”含义一致。
/// registry, starts background web/MCP servers, and launches the interactive
///     中文：该注释与英文“registry, starts background web/MCP servers, and launches the interactive”含义一致。
/// terminal application.
///     中文：该注释与英文“terminal application.”含义一致。
///
/// This function handles provider-specific client creation (API key validation,
///     中文：该注释与英文“This function handles provider-specific client creation (API key validation,”含义一致。
/// model selection) and delegates the actual TUI lifecycle to [`run_tui_with_model`].
///     中文：该注释与英文“model selection) and delegates the actual TUI lifecycle to [`run_tui_with_model`].”含义一致。
///
/// # Side Effects
///     中文：标题：Side Effects。
/// - Reads provider credentials from environment variables and optional dotenv
///     中文：列表项说明与英文“Reads provider credentials from environment variables and optional dotenv”含义一致。
///   files.
///     中文：该注释与英文“files.”含义一致。
/// - Registers local file, shell, planning, and MCP bridge tools for the agent.
///     中文：列表项说明与英文“Registers local file, shell, planning, and MCP bridge tools for the agent.”含义一致。
/// - May start web/MCP background services and a managed Codex app-server.
///     中文：列表项说明与英文“May start web/MCP background services and a managed Codex app-server.”含义一致。
/// - May mutate the workspace through tools when the selected context permits it.
///     中文：列表项说明与英文“May mutate the workspace through tools when the selected context permits it.”含义一致。
///
/// # Errors
///     中文：标题：Errors。
/// Returns [`CliError`] for missing credentials, invalid provider configuration,
///     中文：该注释与英文“Returns [`CliError`] for missing credentials, invalid provider configuration,”含义一致。
/// unsafe mode/host combinations, provider bootstrap failures, or failures from
///     中文：该注释与英文“unsafe mode/host combinations, provider bootstrap failures, or failures from”含义一致。
/// the shared TUI lifecycle.
///     中文：该注释与英文“the shared TUI lifecycle.”含义一致。
async fn execute_tui(args: CodeArgs) -> CliResult<()> {
    let working_dir = resolve_code_working_dir(&args)?;
    let env_file = load_code_env_file(args.env_file.as_deref())?;
    let browser_control = resolve_browser_control_mode(&args)?;
    let control_runtime = prepare_control_runtime(&args, &working_dir).await?;

    let task_intent = task_intent_for_context(args.context);
    // OC-Phase 2 P2.4: resolve `--agent <name>` once before any provider-
    // 中文：该注释与英文“OC-Phase 2 P2.4: resolve `--agent <name>` once before any provider-”含义一致。
    // specific knob (context budget, completion thinking / reasoning /
    // 中文：该注释与英文“specific knob (context budget, completion thinking / reasoning /”含义一致。
    // stream, preamble) is computed. When the agent's spec carries a
    // 中文：该注释与英文“stream, preamble) is computed. When the agent's spec carries a”含义一致。
    // structured binding, the effective provider may differ from the CLI
    // 中文：该注释与英文“structured binding, the effective provider may differ from the CLI”含义一致。
    // `--provider` default; downstream computations need the agent's
    // 中文：该注释与英文“`--provider` default; downstream computations need the agent's”含义一致。
    // provider, not the CLI one.
    // 中文：该注释与英文“provider, not the CLI one.”含义一致。
    let effective_provider = effective_code_provider_for_args(&args, &working_dir)?;
    let effective_model_for_preamble = if effective_provider == args.provider {
        args.model.as_deref().map(str::to_string)
    } else {
        // The agent override path resolves the concrete model id later
        // 中文：该注释与英文“The agent override path resolves the concrete model id later”含义一致。
        // inside `build_any_completion_model_for_args`; here we only need
        // 中文：该注释与英文“inside `build_any_completion_model_for_args`; here we only need”含义一致。
        // it for `system_preamble`'s context budget defaulting, where
        // 中文：该注释与英文“it for `system_preamble`'s context budget defaulting, where”含义一致。
        // `None` falls back to the provider's flagship via
        // 中文：该注释与英文“`None` falls back to the provider's flagship via”含义一致。
        // [`default_context_budget_model`].
        // 中文：该注释与英文“[`default_context_budget_model`].”含义一致。
        None
    };
    let preamble = system_preamble(
        &working_dir,
        args.context,
        effective_provider,
        effective_model_for_preamble.as_deref(),
    );
    let temperature = args.temperature;
    let thinking = completion_thinking_for_provider(effective_provider, &args);
    let reasoning_effort = completion_reasoning_effort_for_provider(effective_provider, &args);
    let stream = completion_stream_for_provider(effective_provider, &args);
    let preserve_reasoning_content = preserve_reasoning_content_for_provider(effective_provider);
    let resume_thread_id = args.resume.clone();
    let host = args.host.clone();
    let trace_id = resume_thread_id
        .as_deref()
        .and_then(|thread_id| Uuid::parse_str(thread_id).ok())
        .unwrap_or_else(Uuid::new_v4);

    // Prepare MCP server instance shared between the HTTP transport and TUI bridge.
    // 中文：该注释与英文“Prepare MCP server instance shared between the HTTP transport and TUI bridge.”含义一致。
    // INVARIANT: the same server instance backs both transports so an agent sees
    // 中文：该注释与英文“INVARIANT: the same server instance backs both transports so an agent sees”含义一致。
    // one coherent history/object store regardless of whether a tool is invoked
    // 中文：该注释与英文“one coherent history/object store regardless of whether a tool is invoked”含义一致。
    // through HTTP MCP or the in-process TUI bridge.
    // 中文：该注释与英文“through HTTP MCP or the in-process TUI bridge.”含义一致。
    let mcp_server = init_mcp_server(&working_dir).await;

    // Create the bridge channel for request_user_input tool <-> TUI communication.
    // 中文：该注释与英文“Create the bridge channel for request_user_input tool <-> TUI communication.”含义一致。
    let (user_input_tx, user_input_rx) = tokio::sync::mpsc::unbounded_channel::<UserInputRequest>();
    let (exec_approval_tx, exec_approval_rx) =
        tokio::sync::mpsc::unbounded_channel::<ExecApprovalRequest>();

    // Build registry: basic file tools + MCP workflow tools.
    // 中文：该注释与英文“Build registry: basic file tools + MCP workflow tools.”含义一致。
    //
    // AI user story: let a coding agent inspect files, search context, make
    // 中文：该注释与英文“AI user story: let a coding agent inspect files, search context, make”含义一致。
    // bounded edits, run verification commands, ask the human for missing
    // 中文：该注释与英文“bounded edits, run verification commands, ask the human for missing”含义一致。
    // choices, and record structured planning artifacts without leaving the
    // 中文：该注释与英文“choices, and record structured planning artifacts without leaving the”含义一致。
    // sandbox/approval model.
    // 中文：该注释与英文“sandbox/approval model.”含义一致。
    let mut builder = ToolRegistryBuilder::with_working_dir(working_dir.clone())
        .hardening(ToolBoundaryRuntime::system(
            trace_id,
            Arc::new(TracingAuditSink),
        ))
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("search_files", Arc::new(SearchFilesHandler))
        .register("web_search", Arc::new(WebSearchHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register("submit_intent_draft", Arc::new(SubmitIntentDraftHandler))
        .register("submit_plan_draft", Arc::new(SubmitPlanDraftHandler))
        .register("submit_task_complete", Arc::new(SubmitTaskCompleteHandler))
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx.clone())),
        );
    builder = register_semantic_handlers(builder);

    // AI user story: MCP bridge tools let the agent persist intent/task/run,
    // 中文：该注释与英文“AI user story: MCP bridge tools let the agent persist intent/task/run,”含义一致。
    // evidence, provenance, and Libra VCS operations in the same workflow graph
    // 中文：该注释与英文“evidence, provenance, and Libra VCS operations in the same workflow graph”含义一致。
    // that external MCP clients use. Keep these names aligned with
    // 中文：该注释与英文“that external MCP clients use. Keep these names aligned with”含义一致。
    // `docs/agent/intentspec_typical.yaml` and `docs/agent/agent-workflow.md`.
    // 中文：该注释与英文“`docs/agent/intentspec_typical.yaml` and `docs/agent/agent-workflow.md`.”含义一致。
    for (name, handler) in McpBridgeHandler::all_handlers(mcp_server.clone()) {
        builder = builder.register(name, handler);
    }

    let registry = Arc::new(builder.build());
    let allowed_tools = registry.filter_by_intent(task_intent);

    let approval_config = approval_config_from_project_config(registry.working_dir());
    let approval_ttl = args
        .approval_ttl
        .map(Duration::from_secs)
        .or(approval_config.ttl)
        .unwrap_or(DEFAULT_APPROVAL_TTL);
    let provider_name = format!("{:?}", args.provider).to_lowercase();
    let launch_config = TuiLaunchConfig {
        host,
        port: args.port,
        mcp_port: args.mcp_port,
        registry,
        preamble,
        temperature,
        thinking,
        reasoning_effort,
        stream,
        preserve_reasoning_content,
        allowed_tools: Some(allowed_tools),
        auto_classify_first_user_message: args.context.is_none(),
        context: args.context,
        resume_thread_id,
        approval_policy: args.approval_policy.into(),
        allow_all_commands: args.approval_policy.allows_all_commands(),
        approval_ttl,
        approval_cache_policy: approval_config.cache_policy,
        network_access: args.network_access.is_allowed(),
        user_input_rx,
        exec_approval_rx,
        exec_approval_tx,
        mcp_server,
        control_runtime,
        browser_control,
        initial_goal: args.goal.clone(),
    };

    // Create agent based on provider. Every non-Codex provider funnels
    // 中文：该注释与英文“Create agent based on provider. Every non-Codex provider funnels”含义一致。
    // through `ProviderFactory`; Codex keeps its own managed-runtime path.
    // 中文：该注释与英文“through `ProviderFactory`; Codex keeps its own managed-runtime path.”含义一致。
    match args.provider {
        CodeProvider::Codex => {
            let mut server =
                start_managed_codex_server(&args.codex_bin, args.codex_port, &working_dir).await?;
            let browser_write_enabled =
                launch_config.browser_control == BrowserControlMode::Loopback;
            // `LocalTui` keeps the terminal as the visible owner while letting
            // 中文：该注释与英文“`LocalTui` keeps the terminal as the visible owner while letting”含义一致。
            // browser/automation leases attach when their writer is enabled.
            // 中文：该注释与英文“browser/automation leases attach when their writer is enabled.”含义一致。
            // Fall back to `Fixed { Tui }` only when both writers are off
            // 中文：该注释与英文“Fall back to `Fixed { Tui }` only when both writers are off”含义一致。
            // (read-only observe).
            // 中文：该注释与英文“(read-only observe).”含义一致。
            let initial_controller =
                if launch_config.control_runtime.is_write() || browser_write_enabled {
                    CodeUiInitialController::LocalTui {
                        owner_label: "Terminal UI".to_string(),
                        reason: Some("The terminal UI controls this live Codex run".to_string()),
                    }
                } else {
                    CodeUiInitialController::Fixed {
                        kind: CodeUiControllerKind::Tui,
                        owner_label: "Terminal UI".to_string(),
                        reason: Some("The terminal UI controls this live Codex run".to_string()),
                    }
                };
            let code_ui_runtime = match start_codex_code_ui_runtime(
                &args,
                &working_dir,
                &server.ws_url,
                launch_config.mcp_server.clone(),
                browser_write_enabled,
                initial_controller,
            )
            .await
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    server.shutdown().await;
                    return Err(error);
                }
            };
            let model_name = args.model.clone().unwrap_or_else(|| "codex".to_string());
            let result = run_tui_with_managed_code_runtime(
                code_ui_runtime,
                launch_config,
                model_name,
                provider_name,
            )
            .await;
            server.shutdown().await;
            result?;
        }
        _ => {
            // OC-Phase 2 P2.4: the helper returns the *effective* provider
            // 中文：该注释与英文“OC-Phase 2 P2.4: the helper returns the *effective* provider”含义一致。
            // name so usage / UI metadata reports the agent-selected
            // 中文：该注释与英文“name so usage / UI metadata reports the agent-selected”含义一致。
            // provider after a `--agent <name>` override, not the CLI
            // 中文：该注释与英文“provider after a `--agent <name>` override, not the CLI”含义一致。
            // `--provider` default that the helper started from.
            // 中文：该注释与英文“`--provider` default that the helper started from.”含义一致。
            let (model, model_name, effective_provider_name) =
                build_any_completion_model_for_args(&args, &env_file, &working_dir)?;
            run_tui_with_model(model, launch_config, model_name, effective_provider_name).await?;
        }
    }

    Ok(())
}

fn completion_thinking_for_args(args: &CodeArgs) -> Option<CompletionThinking> {
    completion_thinking_for_provider(args.provider, args)
}

/// Provider-explicit variant of [`completion_thinking_for_args`] used by the
///     中文：该注释与英文“Provider-explicit variant of [`completion_thinking_for_args`] used by the”含义一致。
/// `--agent` override path so the resolved provider drives the dispatch.
///     中文：该注释与英文“`--agent` override path so the resolved provider drives the dispatch.”含义一致。
fn completion_thinking_for_provider(
    provider: CodeProvider,
    args: &CodeArgs,
) -> Option<CompletionThinking> {
    match provider {
        CodeProvider::Ollama => args.ollama_thinking.map(CompletionThinking::from),
        CodeProvider::Deepseek => args.deepseek_thinking.map(CompletionThinking::from),
        CodeProvider::Kimi => args.kimi_thinking.map(CompletionThinking::from),
        _ => None,
    }
}

fn completion_reasoning_effort_for_args(args: &CodeArgs) -> Option<CompletionReasoningEffort> {
    completion_reasoning_effort_for_provider(args.provider, args)
}

/// Provider-explicit variant of [`completion_reasoning_effort_for_args`].
///     中文：该注释与英文“Provider-explicit variant of [`completion_reasoning_effort_for_args`].”含义一致。
fn completion_reasoning_effort_for_provider(
    provider: CodeProvider,
    args: &CodeArgs,
) -> Option<CompletionReasoningEffort> {
    match provider {
        CodeProvider::Deepseek => args
            .deepseek_reasoning_effort
            .map(CompletionReasoningEffort::from),
        _ => None,
    }
}

fn completion_stream_for_args(args: &CodeArgs) -> Option<bool> {
    completion_stream_for_provider(args.provider, args)
}

/// Provider-explicit variant of [`completion_stream_for_args`].
///     中文：该注释与英文“Provider-explicit variant of [`completion_stream_for_args`].”含义一致。
fn completion_stream_for_provider(provider: CodeProvider, args: &CodeArgs) -> Option<bool> {
    match provider {
        CodeProvider::Deepseek => args.deepseek_stream,
        CodeProvider::Kimi => Some(args.kimi_stream.unwrap_or(true)),
        _ => None,
    }
}

fn preserve_reasoning_content_for_provider(provider: CodeProvider) -> bool {
    matches!(provider, CodeProvider::Deepseek | CodeProvider::Kimi)
}

// ---------------------------------------------------------------------------
// Codex provider — managed app-server lifecycle
// 中文：该注释与英文“Codex provider — managed app-server lifecycle”含义一致。
// ---------------------------------------------------------------------------

/// Represents a managed Codex app-server child process and its WebSocket URL.
///     中文：该注释与英文“Represents a managed Codex app-server child process and its WebSocket URL.”含义一致。
///
/// The server is spawned as a child process and communicated with over WebSocket.
///     中文：该注释与英文“The server is spawned as a child process and communicated with over WebSocket.”含义一致。
/// [`ManagedCodexServer::shutdown`] sends SIGKILL and waits up to 5 seconds.
///     中文：该注释与英文“[`ManagedCodexServer::shutdown`] sends SIGKILL and waits up to 5 seconds.”含义一致。
struct ManagedCodexServer {
    ws_url: String,
    child: Child,
}

impl ManagedCodexServer {
    /// Gracefully shuts down the managed Codex app-server process.
    ///     中文：该注释与英文“Gracefully shuts down the managed Codex app-server process.”含义一致。
    ///
    /// If the child process has already exited (`id()` returns `None`), this is
    ///     中文：该注释与英文“If the child process has already exited (`id()` returns `None`), this is”含义一致。
    /// a no-op. Otherwise it sends a kill signal via `start_kill()` and waits up
    ///     中文：该注释与英文“a no-op. Otherwise it sends a kill signal via `start_kill()` and waits up”含义一致。
    /// to 5 seconds for the process to terminate. If the timeout expires the
    ///     中文：该注释与英文“to 5 seconds for the process to terminate. If the timeout expires the”含义一致。
    /// process is abandoned (the OS will reap it when the handle is dropped).
    ///     中文：该注释与英文“process is abandoned (the OS will reap it when the handle is dropped).”含义一致。
    async fn shutdown(&mut self) {
        if self.child.id().is_none() {
            return;
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

struct ControlRuntimeConfig {
    mode: ControlMode,
    paths: ControlPaths,
    token: Option<Arc<str>>,
    _lock_guard: Option<ControlLockGuard>,
    write_info: bool,
    cleanup_token: bool,
    info_written: AtomicBool,
    started_at: chrono::DateTime<Utc>,
}

impl ControlRuntimeConfig {
    fn is_write(&self) -> bool {
        self.mode == ControlMode::Write
    }

    fn mode_name(&self) -> &'static str {
        match self.mode {
            ControlMode::Observe => "observe",
            ControlMode::Write => "write",
        }
    }

    fn cleanup(&self) {
        cleanup_control_files(
            &self.paths,
            self.cleanup_token,
            self.info_written.load(Ordering::Relaxed),
        );
    }

    fn write_info_file(
        &self,
        working_dir: &Path,
        base_url: String,
        mcp_url: Option<String>,
        thread_id: Option<String>,
    ) -> CliResult<()> {
        if !self.write_info {
            return Ok(());
        }

        let info = ControlInfo {
            version: 1,
            mode: self.mode_name().to_string(),
            pid: std::process::id(),
            base_url,
            mcp_url,
            working_dir: working_dir.to_path_buf(),
            thread_id,
            started_at: self.started_at,
        };
        write_control_info(&self.paths.info, &info).map_err(|error| {
            CliError::fatal(format!(
                "failed to write local TUI control info '{}': {error}",
                self.paths.info.display()
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
        self.info_written.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for ControlRuntimeConfig {
    fn drop(&mut self) {
        self.cleanup();
    }
}

async fn prepare_control_runtime(
    args: &CodeArgs,
    working_dir: &Path,
) -> CliResult<ControlRuntimeConfig> {
    let paths = resolve_control_paths(
        working_dir,
        args.control_token_file.as_deref(),
        args.control_info_file.as_deref(),
    );
    let started_at = Utc::now();

    match args.control {
        ControlMode::Observe => Ok(ControlRuntimeConfig {
            mode: ControlMode::Observe,
            paths,
            token: None,
            _lock_guard: None,
            write_info: args.control_info_file.is_some(),
            cleanup_token: false,
            info_written: AtomicBool::new(false),
            started_at,
        }),
        ControlMode::Write => {
            let lock_guard = acquire_control_lock(&paths.lock).map_err(|error| match error {
                ControlLockError::AlreadyHeld { .. } => CliError::conflict(error.to_string()),
                ControlLockError::Io(error) => CliError::io(format!(
                    "failed to acquire local TUI control lock '{}': {error}",
                    paths.lock.display()
                )),
            })?;
            let token = ensure_control_token_file(&paths.token)
                .await
                .map_err(|error| {
                    CliError::fatal(format!(
                        "failed to prepare local TUI control token '{}': {error}",
                        paths.token.display()
                    ))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
                })?;

            Ok(ControlRuntimeConfig {
                mode: ControlMode::Write,
                paths,
                token: Some(Arc::<str>::from(token)),
                _lock_guard: Some(lock_guard),
                write_info: true,
                cleanup_token: true,
                info_written: AtomicBool::new(false),
                started_at,
            })
        }
    }
}

fn ensure_loopback_browser_control_host(host: &str) -> CliResult<()> {
    let normalized = host.trim().trim_matches('[').trim_matches(']');
    let is_loopback = matches!(normalized, "localhost" | "127.0.0.1" | "::1")
        || normalized
            .parse::<std::net::IpAddr>()
            .map(|addr| addr.is_loopback())
            .unwrap_or(false);

    if is_loopback {
        return Ok(());
    }

    Err(CliError::command_usage(
        "interactive web control is restricted to loopback hosts in v1; use --host 127.0.0.1",
    ))
}

/// Resolve the effective [`BrowserControlMode`] for this invocation.
///     中文：该注释与英文“Resolve the effective [`BrowserControlMode`] for this invocation.”含义一致。
///
/// User-supplied `--browser-control` always wins. When the flag is omitted
///     中文：该注释与英文“User-supplied `--browser-control` always wins. When the flag is omitted”含义一致。
/// the default is mode-aware:
///     中文：该注释与英文“the default is mode-aware:”含义一致。
///   - `--web-only --provider codex` → `loopback` (matches the existing
///     中文：列表项说明与英文“`--web-only --provider codex` → `loopback` (matches the existing”含义一致。
///     "browser write enabled" default for managed Codex sessions),
///     中文：该注释与英文“"browser write enabled" default for managed Codex sessions),”含义一致。
///   - all other entry points → `off` (TUI sessions and non-Codex
///     中文：列表项说明与英文“all other entry points → `off` (TUI sessions and non-Codex”含义一致。
///     `--web-only` placeholders).
///     中文：该注释与英文“`--web-only` placeholders).”含义一致。
///
/// `loopback` further requires that `--host` is a loopback address; this is
///     中文：该注释与英文“`loopback` further requires that `--host` is a loopback address; this is”含义一致。
/// validated up-front so we fail closed before any port is bound.
///     中文：该注释与英文“validated up-front so we fail closed before any port is bound.”含义一致。
pub fn resolve_browser_control_mode(args: &CodeArgs) -> CliResult<BrowserControlMode> {
    let mode = match args.browser_control {
        Some(mode) => mode,
        None => default_browser_control_mode(args),
    };
    if mode == BrowserControlMode::Loopback {
        ensure_loopback_browser_control_host(&args.host)?;
    }
    Ok(mode)
}

fn default_browser_control_mode(args: &CodeArgs) -> BrowserControlMode {
    if args.web_only && matches!(args.provider, CodeProvider::Codex) {
        BrowserControlMode::Loopback
    } else {
        BrowserControlMode::Off
    }
}

/// CLI-side wrapper around `code_ui::test_lease_duration_override` that maps
///     中文：该注释与英文“CLI-side wrapper around `code_ui::test_lease_duration_override` that maps”含义一致。
/// the helper's `String` error into `CliError::command_usage` so a bad
///     中文：该注释与英文“the helper's `String` error into `CliError::command_usage` so a bad”含义一致。
/// `LIBRA_CODE_LEASE_DURATION_MS` value fails the command at startup with
///     中文：该注释与英文“`LIBRA_CODE_LEASE_DURATION_MS` value fails the command at startup with”含义一致。
/// a stable, user-readable message.
///     中文：该注释与英文“a stable, user-readable message.”含义一致。
fn code_ui_test_lease_duration_override() -> CliResult<Option<chrono::Duration>> {
    crate::internal::ai::web::code_ui::test_lease_duration_override()
        .map_err(CliError::command_usage)
}

const HEADLESS_CODE_UI_SNAPSHOT_METADATA_KEY: &str = "code_ui_snapshot";

struct HeadlessWebSessionBootstrap {
    store: Arc<SessionStore>,
    state: SessionState,
}

struct HeadlessApprovalChannels {
    exec_approval_tx: mpsc::UnboundedSender<ExecApprovalRequest>,
    exec_approval_rx: mpsc::UnboundedReceiver<ExecApprovalRequest>,
}

fn load_or_create_headless_web_session_state(
    args: &CodeArgs,
    working_dir: &Path,
    session_store: &Arc<SessionStore>,
) -> CliResult<SessionState> {
    let working_dir_str = working_dir.to_string_lossy().to_string();
    let mut session = if let Some(thread_id) = args.resume.as_deref() {
        if thread_id.trim().is_empty() {
            return Err(CliError::command_usage(
                "--resume requires a non-empty thread_id",
            ));
        }
        match session_store.load_for_thread_id(thread_id, &working_dir_str) {
            Ok(Some(session)) => session,
            Ok(None) => {
                return Err(CliError::fatal(format!(
                    "no Libra Code session found for thread_id '{thread_id}' in working directory '{working_dir_str}'"
                )));
            }
            Err(error) => {
                return Err(CliError::io(format!(
                    "failed to load Libra Code session for thread_id '{thread_id}': {error}"
                )));
            }
        }
    } else {
        SessionState::new(&working_dir_str)
    };

    let thread_id = session_canonical_thread_id(&session).unwrap_or_else(|| session.id.clone());
    session
        .metadata
        .entry("thread_id".to_string())
        .or_insert_with(|| serde_json::json!(thread_id));
    Ok(session)
}

fn build_headless_web_code_ui_snapshot(
    working_dir: &Path,
    provider: CodeUiProviderInfo,
    capabilities: CodeUiCapabilities,
    session: &SessionState,
) -> CodeUiSessionSnapshot {
    let working_dir = working_dir.to_string_lossy().to_string();
    let mut snapshot = session
        .metadata
        .get(HEADLESS_CODE_UI_SNAPSHOT_METADATA_KEY)
        .and_then(|value| serde_json::from_value::<CodeUiSessionSnapshot>(value.clone()).ok())
        .unwrap_or_else(|| {
            initial_snapshot(working_dir.clone(), provider.clone(), capabilities.clone())
        });

    snapshot.session_id = session.id.clone();
    snapshot.thread_id =
        Some(session_canonical_thread_id(session).unwrap_or_else(|| session.id.clone()));
    snapshot.working_dir = working_dir;
    snapshot.provider = provider;
    snapshot.capabilities = capabilities;
    if snapshot.transcript.is_empty() {
        snapshot.transcript = build_tui_code_ui_transcript(session);
    }

    let now = Utc::now();
    for entry in &mut snapshot.transcript {
        if entry.streaming {
            entry.streaming = false;
            if !matches!(
                entry.status.as_deref(),
                Some("completed" | "error" | "cancelled")
            ) {
                entry.status = Some("cancelled".to_string());
            }
            entry.updated_at = now;
        }
    }
    let has_pending_interaction = snapshot
        .interactions
        .iter()
        .any(|interaction| interaction.status == CodeUiInteractionStatus::Pending);
    snapshot.status = if has_pending_interaction {
        CodeUiSessionStatus::AwaitingInteraction
    } else {
        CodeUiSessionStatus::Idle
    };
    snapshot.updated_at = now;
    snapshot
}

/// Build a headless Code UI runtime for `--web-only` non-Codex providers.
///     中文：该注释与英文“Build a headless Code UI runtime for `--web-only` non-Codex providers.”含义一致。
///
/// Constructs a minimal local-read-only [`ToolRegistry`]
///     中文：该注释与英文“Constructs a minimal local-read-only [`ToolRegistry`]”含义一致。
/// and wires it into a [`HeadlessCodeRuntime`] so the browser composer can
///     中文：该注释与英文“and wires it into a [`HeadlessCodeRuntime`] so the browser composer can”含义一致。
/// drive a real agent turn against the supplied `model`. The result is
///     中文：该注释与英文“drive a real agent turn against the supplied `model`. The result is”含义一致。
/// exposed through [`CodeUiRuntimeHandle`] just like the TUI flow, so the
///     中文：该注释与英文“exposed through [`CodeUiRuntimeHandle`] just like the TUI flow, so the”含义一致。
/// rest of `start_web_server` can use it without per-mode special cases.
///     中文：该注释与英文“rest of `start_web_server` can use it without per-mode special cases.”含义一致。
///
/// `browser_write_enabled` should mirror the resolved
///     中文：该注释与英文“`browser_write_enabled` should mirror the resolved”含义一致。
/// [`BrowserControlMode::Loopback`] so the runtime advertises browser writes
///     中文：该注释与英文“[`BrowserControlMode::Loopback`] so the runtime advertises browser writes”含义一致。
/// in the snapshot capabilities. The initial controller is `Unclaimed` —
///     中文：该注释与英文“in the snapshot capabilities. The initial controller is `Unclaimed` —”含义一致。
/// the browser is the only writer in headless mode, no TUI to hand off from.
///     中文：该注释与英文“the browser is the only writer in headless mode, no TUI to hand off from.”含义一致。
async fn build_headless_web_code_ui_runtime<M>(
    args: &CodeArgs,
    working_dir: &Path,
    session_bootstrap: HeadlessWebSessionBootstrap,
    model: M,
    model_name: String,
    approval_channels: HeadlessApprovalChannels,
    browser_write_enabled: bool,
) -> CliResult<Arc<CodeUiRuntimeHandle>>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    use crate::internal::ai::agent::runtime::tool_loop::ToolLoopConfig;

    let HeadlessWebSessionBootstrap {
        store: session_store,
        state: session_state,
    } = session_bootstrap;
    let HeadlessApprovalChannels {
        exec_approval_tx,
        exec_approval_rx,
    } = approval_channels;
    let provider_name = format!("{:?}", args.provider).to_lowercase();
    let provider = CodeUiProviderInfo {
        provider: provider_name.clone(),
        model: Some(model_name.clone()),
        mode: Some("web-headless".to_string()),
        managed: false,
    };
    let capabilities = headless_capabilities();
    let initial_history = session_state.to_history();
    let snapshot = build_headless_web_code_ui_snapshot(
        working_dir,
        provider,
        capabilities.clone(),
        &session_state,
    );
    let session = CodeUiSession::new(snapshot);
    let persistence = HeadlessSessionPersistence::new(session_store, session_state);

    let approval_config = approval_config_from_project_config(working_dir);
    let approval_ttl = args
        .approval_ttl
        .map(Duration::from_secs)
        .or(approval_config.ttl)
        .unwrap_or(DEFAULT_APPROVAL_TTL);
    let (user_input_tx, user_input_rx) = mpsc::unbounded_channel::<UserInputRequest>();
    let runtime_context = Some(default_tui_runtime_context(
        working_dir,
        args.context,
        DefaultTuiApprovalConfig {
            policy: args.approval_policy.into(),
            allow_all_commands: args.approval_policy.allows_all_commands(),
            ttl: approval_ttl,
            cache_policy: approval_config.cache_policy,
        },
        args.network_access.is_allowed(),
        exec_approval_tx,
    ));

    let registry = build_headless_tool_registry(working_dir, user_input_tx);
    let preamble = system_preamble(working_dir, args.context, args.provider, Some(&model_name));
    let preserve_reasoning_content = preserve_reasoning_content_for_provider(args.provider);
    let temperature = args.temperature;
    let thinking = completion_thinking_for_args(args);
    let reasoning_effort = completion_reasoning_effort_for_args(args);
    let stream = completion_stream_for_args(args);

    let config_factory: Arc<dyn Fn() -> ToolLoopConfig + Send + Sync> =
        Arc::new(move || ToolLoopConfig {
            preamble: Some(preamble.clone()),
            temperature,
            thinking,
            reasoning_effort,
            stream,
            preserve_reasoning_content,
            runtime_context: runtime_context.clone(),
            ..Default::default()
        });

    let adapter = HeadlessCodeRuntime::new_with_persistence(
        session,
        capabilities,
        model,
        registry,
        user_input_rx,
        exec_approval_rx,
        config_factory,
        initial_history,
        Some(persistence),
    );

    let mut runtime_options = CodeUiRuntimeOptions::new(
        browser_write_enabled,
        false,
        CodeUiInitialController::Unclaimed,
    );
    runtime_options.lease_duration = code_ui_test_lease_duration_override()?;
    Ok(CodeUiRuntimeHandle::build_with_options(adapter, runtime_options).await)
}

fn build_headless_tool_registry(
    working_dir: &Path,
    user_input_tx: mpsc::UnboundedSender<UserInputRequest>,
) -> Arc<ToolRegistry> {
    // Headless web mode now reuses the same ToolRuntimeContext path as TUI:
    // 中文：该注释与英文“Headless web mode now reuses the same ToolRuntimeContext path as TUI:”含义一致。
    // shell/apply_patch route through sandbox + exec approval, web_search sees
    // 中文：该注释与英文“shell/apply_patch route through sandbox + exec approval, web_search sees”含义一致。
    // the CLI network policy, and pending approvals surface through
    // 中文：该注释与英文“the CLI network policy, and pending approvals surface through”含义一致。
    // CodeUiInteractionRequest. `submit_plan_draft` is exposed because
    // 中文：该注释与英文“CodeUiInteractionRequest. `submit_plan_draft` is exposed because”含义一致。
    // headless projects it into plans[]; workflow tools that require a
    // 中文：该注释与英文“headless projects it into plans[]; workflow tools that require a”含义一致。
    // session driver (`task`, `submit_intent_draft`) remain gated.
    // 中文：该注释与英文“session driver (`task`, `submit_intent_draft`) remain gated.”含义一致。
    let trace_id = uuid::Uuid::new_v4();
    let builder = ToolRegistryBuilder::with_working_dir(working_dir.to_path_buf())
        .hardening(ToolBoundaryRuntime::system(
            trace_id,
            Arc::new(TracingAuditSink),
        ))
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("search_files", Arc::new(SearchFilesHandler))
        .register("web_search", Arc::new(WebSearchHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register("submit_plan_draft", Arc::new(SubmitPlanDraftHandler))
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx)),
        );
    Arc::new(register_semantic_handlers(builder).build())
}

/// Construct the appropriate provider client and wrap it in
///     中文：该注释与英文“Construct the appropriate provider client and wrap it in”含义一致。
/// [`build_headless_web_code_ui_runtime`]. Returns `None` when the requested
///     中文：该注释与英文“[`build_headless_web_code_ui_runtime`]. Returns `None` when the requested”含义一致。
/// provider is not yet wired into the headless path so the caller can fall
///     中文：该注释与英文“provider is not yet wired into the headless path so the caller can fall”含义一致。
/// back to the read-only placeholder gracefully.
///     中文：该注释与英文“back to the read-only placeholder gracefully.”含义一致。
///
/// v0 now routes several non-Codex providers through the same provider-factory
///     中文：该注释与英文“v0 now routes several non-Codex providers through the same provider-factory”含义一致。
/// bootstrap used by TUI. This keeps API-key/base-URL resolution centralized and
///     中文：该注释与英文“bootstrap used by TUI. This keeps API-key/base-URL resolution centralized and”含义一致。
/// ensures `--web-only` behavior stays aligned with existing provider construction.
///     中文：该注释与英文“ensures `--web-only` behavior stays aligned with existing provider construction.”含义一致。
///
/// The placeholder path is still available for providers that are not in this
///     中文：该注释与英文“The placeholder path is still available for providers that are not in this”含义一致。
/// dispatch arm or fail during bootstrap for other reasons.
///     中文：该注释与英文“dispatch arm or fail during bootstrap for other reasons.”含义一致。
async fn build_non_codex_headless_runtime(
    args: &CodeArgs,
    working_dir: &Path,
    session_store: Arc<SessionStore>,
    session_state: SessionState,
    browser_write_enabled: bool,
) -> CliResult<Option<Arc<CodeUiRuntimeHandle>>> {
    let (exec_approval_tx, exec_approval_rx) =
        tokio::sync::mpsc::unbounded_channel::<ExecApprovalRequest>();

    match args.provider {
        CodeProvider::Gemini
        | CodeProvider::Openai
        | CodeProvider::Anthropic
        | CodeProvider::Deepseek
        | CodeProvider::Kimi
        | CodeProvider::Zhipu
        | CodeProvider::Ollama => {
            let (model, model_name, _) =
                build_any_completion_model_for_args(args, &CodeEnvFile::default(), working_dir)?;
            Ok(Some(
                build_headless_web_code_ui_runtime(
                    args,
                    working_dir,
                    HeadlessWebSessionBootstrap {
                        store: session_store,
                        state: session_state,
                    },
                    model,
                    model_name,
                    HeadlessApprovalChannels {
                        exec_approval_tx,
                        exec_approval_rx,
                    },
                    browser_write_enabled,
                )
                .await?,
            ))
        }
        // Codex is handled by `start_codex_code_ui_runtime` in `execute_web_only`;
        // 中文：该注释与英文“Codex is handled by `start_codex_code_ui_runtime` in `execute_web_only`;”含义一致。
        // it must never enter this dispatcher.
        // 中文：该注释与英文“it must never enter this dispatcher.”含义一致。
        CodeProvider::Codex => Ok(None),
        #[cfg(feature = "test-provider")]
        CodeProvider::Fake => {
            let (model, model_name, _) =
                build_any_completion_model_for_args(args, &CodeEnvFile::default(), working_dir)?;
            Ok(Some(
                build_headless_web_code_ui_runtime(
                    args,
                    working_dir,
                    HeadlessWebSessionBootstrap {
                        store: session_store,
                        state: session_state,
                    },
                    model,
                    model_name,
                    HeadlessApprovalChannels {
                        exec_approval_tx,
                        exec_approval_rx,
                    },
                    browser_write_enabled,
                )
                .await?,
            ))
        }
    }
}

async fn build_placeholder_web_code_ui_runtime(
    args: &CodeArgs,
    working_dir: &Path,
) -> Arc<CodeUiRuntimeHandle> {
    let capabilities = CodeUiCapabilities {
        message_input: false,
        streaming_text: false,
        plan_updates: false,
        tool_calls: false,
        patchsets: false,
        interactive_approvals: false,
        structured_questions: false,
        provider_session_resume: false,
    };

    let mut snapshot = initial_snapshot(
        working_dir.to_string_lossy().to_string(),
        CodeUiProviderInfo {
            provider: format!("{:?}", args.provider).to_lowercase(),
            model: args.model.clone(),
            mode: Some("web".to_string()),
            managed: matches!(args.provider, CodeProvider::Codex),
        },
        capabilities.clone(),
    );
    let now = Utc::now();
    snapshot.status = CodeUiSessionStatus::Idle;
    snapshot.transcript.push(CodeUiTranscriptEntry {
        id: "web-ui-placeholder".to_string(),
        kind: CodeUiTranscriptEntryKind::InfoNote,
        title: Some("Web Control Unavailable".to_string()),
        content: Some(
            "Interactive browser control is fully implemented for `--provider codex`. For other providers, launch `libra code` without `--web-only` to observe the live terminal session in the browser."
                .to_string(),
        ),
        status: Some("completed".to_string()),
        streaming: false,
        metadata: serde_json::json!({ "providerAgnostic": true }),
        created_at: now,
        updated_at: now,
    });

    CodeUiRuntimeHandle::build(
        ReadOnlyCodeUiAdapter::new(CodeUiSession::new(snapshot), capabilities),
        false,
        CodeUiInitialController::Unclaimed,
    )
    .await
}

async fn start_codex_code_ui_runtime(
    args: &CodeArgs,
    working_dir: &Path,
    ws_url: &str,
    mcp_server: Arc<LibraMcpServer>,
    browser_write_enabled: bool,
    initial_controller: CodeUiInitialController,
) -> CliResult<Arc<CodeUiRuntimeHandle>> {
    let ui_mode = match &initial_controller {
        CodeUiInitialController::Fixed {
            kind: CodeUiControllerKind::Tui,
            ..
        } => Some("tui".to_string()),
        CodeUiInitialController::Fixed {
            kind: CodeUiControllerKind::Cli,
            ..
        } => Some("cli".to_string()),
        CodeUiInitialController::LocalTui { .. } => Some("managed-tui".to_string()),
        _ => Some("web".to_string()),
    };
    let plan_mode = effective_plan_mode(args);
    let approval_auto_accepts = matches!(
        args.approval_policy,
        CodeApprovalPolicy::Never | CodeApprovalPolicy::AllowAll
    );
    tracing::info!(
        target: "libra::internal::ai::codex",
        plan_mode,
        provider = "codex",
        approval_policy = ?args.approval_policy,
        "starting Codex code-ui runtime; plan_mode {} (defaults to true for codex provider)",
        if plan_mode { "enabled" } else { "disabled" }
    );
    if plan_mode && approval_auto_accepts {
        tracing::warn!(
            target: "libra::internal::ai::codex",
            approval_policy = ?args.approval_policy,
            "plan_mode is enabled but the approval policy auto-accepts every \
             request — Codex will produce a plan and then run it without an \
             explicit operator review. Use --approval-policy on-request to \
             keep the review gate active."
        );
    }
    let agent_args = agent_codex::AgentCodexArgs {
        url: ws_url.to_string(),
        cwd: working_dir.to_string_lossy().to_string(),
        approval: approval_policy_to_codex(args.approval_policy).to_string(),
        model_provider: None,
        service_tier: None,
        personality: None,
        model: args.model.clone(),
        plan_mode,
        debug: false,
        ui_mode,
    };

    agent_codex::start_code_ui_runtime(
        agent_args,
        mcp_server,
        browser_write_enabled,
        initial_controller,
    )
    .await
    .map_err(|error| CliError::fatal(error.to_string()))
}

// ---------------------------------------------------------------------------
// Approval policy mapping helpers
// 中文：该注释与英文“Approval policy mapping helpers”含义一致。
// ---------------------------------------------------------------------------

/// Maps [`CodeApprovalPolicy`] to the Codex app-server's approval string.
///     中文：该注释与英文“Maps [`CodeApprovalPolicy`] to the Codex app-server's approval string.”含义一致。
/// Codex only distinguishes between "accept" (auto-approve) and "ask" (prompt).
///     中文：该注释与英文“Codex only distinguishes between "accept" (auto-approve) and "ask" (prompt).”含义一致。
fn approval_policy_to_codex(policy: CodeApprovalPolicy) -> &'static str {
    match policy {
        CodeApprovalPolicy::Never | CodeApprovalPolicy::AllowAll => "accept",
        CodeApprovalPolicy::OnFailure
        | CodeApprovalPolicy::OnRequest
        | CodeApprovalPolicy::Untrusted => "ask",
    }
}

/// Starts the Codex app-server as a managed child process.
///     中文：该注释与英文“Starts the Codex app-server as a managed child process.”含义一致。
///
/// 1. Resolves the WebSocket URL (using the requested port or auto-selecting a free one).
///     中文：该注释与英文“1. Resolves the WebSocket URL (using the requested port or auto-selecting a free one).”含义一致。
/// 2. Spawns the Codex binary with `app-server --listen <ws_url>`.
///     中文：该注释与英文“2. Spawns the Codex binary with `app-server --listen <ws_url>`.”含义一致。
/// 3. Polls the WebSocket endpoint until it becomes reachable (or times out).
///     中文：该注释与英文“3. Polls the WebSocket endpoint until it becomes reachable (or times out).”含义一致。
///
/// On failure, the child process is killed before returning the error.
///     中文：该注释与英文“On failure, the child process is killed before returning the error.”含义一致。
async fn start_managed_codex_server(
    codex_bin: &str,
    requested_port: Option<u16>,
    working_dir: &Path,
) -> CliResult<ManagedCodexServer> {
    let ws_url = resolve_codex_ws_url(requested_port)?;
    let mut child = spawn_codex_app_server(codex_bin, &ws_url, working_dir)?;

    if let Err(err) = wait_for_codex_ready(&ws_url).await {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(err);
    }

    Ok(ManagedCodexServer { ws_url, child })
}

/// Builds a `tokio::process::Command` for the Codex app-server.
///     中文：该注释与英文“Builds a `tokio::process::Command` for the Codex app-server.”含义一致。
/// Stdin/stdout/stderr are all set to null since the server communicates
///     中文：该注释与英文“Stdin/stdout/stderr are all set to null since the server communicates”含义一致。
/// exclusively over WebSocket.
///     中文：该注释与英文“exclusively over WebSocket.”含义一致。
fn build_codex_command(program: &str, ws_url: &str, working_dir: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .arg("app-server")
        .arg("--listen")
        .arg(ws_url)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Windows fallback: wraps the Codex binary invocation in `cmd /C` to
///     中文：该注释与英文“Windows fallback: wraps the Codex binary invocation in `cmd /C` to”含义一致。
/// handle `.cmd`/`.bat` shims that are common on Windows (e.g. from npm).
///     中文：该注释与英文“handle `.cmd`/`.bat` shims that are common on Windows (e.g. from npm).”含义一致。
#[cfg(target_os = "windows")]
fn build_windows_shell_codex_command(codex_bin: &str, ws_url: &str, working_dir: &Path) -> Command {
    let mut command = Command::new("cmd");
    command
        .arg("/C")
        .arg(codex_bin)
        .arg("app-server")
        .arg("--listen")
        .arg(ws_url)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Attempts to spawn the Codex app-server process. On Windows, falls back
///     中文：该注释与英文“Attempts to spawn the Codex app-server process. On Windows, falls back”含义一致。
/// to `cmd /C` if the direct spawn fails with `NotFound` (handles `.cmd` shims).
///     中文：该注释与英文“to `cmd /C` if the direct spawn fails with `NotFound` (handles `.cmd` shims).”含义一致。
fn spawn_codex_app_server(codex_bin: &str, ws_url: &str, working_dir: &Path) -> CliResult<Child> {
    match build_codex_command(codex_bin, ws_url, working_dir).spawn() {
        Ok(child) => Ok(child),
        #[cfg(target_os = "windows")]
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            build_windows_shell_codex_command(codex_bin, ws_url, working_dir)
                .spawn()
                .map_err(|shell_err| {
                    CliError::io(format!(
                        "failed to start Codex app-server using '{}': {}. Direct spawn error: {}. Make sure the Codex CLI is installed and available in PATH.",
                        codex_bin, shell_err, err
                    ))
                })
        }
        Err(err) => Err(CliError::io(format!(
            "failed to start Codex app-server using '{}': {}. Make sure the Codex CLI is installed and available in PATH.",
            codex_bin, err
        ))),
    }
}

/// Resolves the WebSocket URL for the Codex app-server.
///     中文：该注释与英文“Resolves the WebSocket URL for the Codex app-server.”含义一致。
/// If no port is specified, auto-selects a free local port via [`pick_free_local_port`].
///     中文：该注释与英文“If no port is specified, auto-selects a free local port via [`pick_free_local_port`].”含义一致。
fn resolve_codex_ws_url(requested_port: Option<u16>) -> CliResult<String> {
    let port = match requested_port {
        Some(0) => {
            return Err(CliError::command_usage(
                "--codex-port must be a non-zero TCP port; omit it to auto-select a free port",
            ));
        }
        Some(port) => port,
        None => pick_free_local_port(DEFAULT_BIND_HOST)?,
    };
    Ok(format!("ws://{DEFAULT_BIND_HOST}:{port}"))
}

/// Binds to port 0 on the given host to let the OS assign a free ephemeral
///     中文：该注释与英文“Binds to port 0 on the given host to let the OS assign a free ephemeral”含义一致。
/// port, then returns that port number. The listener is dropped immediately,
///     中文：该注释与英文“port, then returns that port number. The listener is dropped immediately,”含义一致。
/// releasing the port for the Codex server to bind to.
///     中文：该注释与英文“releasing the port for the Codex server to bind to.”含义一致。
fn pick_free_local_port(host: &str) -> CliResult<u16> {
    let listener = std::net::TcpListener::bind((host, 0)).map_err(|e| {
        CliError::network(format!(
            "failed to reserve a local port for the Codex app-server on {}: {}",
            host, e
        ))
    })?;
    listener.local_addr().map(|addr| addr.port()).map_err(|e| {
        CliError::network(format!(
            "failed to determine the reserved Codex app-server port: {}",
            e
        ))
    })
}

/// Polls the Codex app-server WebSocket endpoint until a connection succeeds
///     中文：该注释与英文“Polls the Codex app-server WebSocket endpoint until a connection succeeds”含义一致。
/// or [`CODEX_STARTUP_TIMEOUT`] is exceeded. The probe connection is immediately
///     中文：该注释与英文“or [`CODEX_STARTUP_TIMEOUT`] is exceeded. The probe connection is immediately”含义一致。
/// dropped after a successful handshake.
///     中文：该注释与英文“dropped after a successful handshake.”含义一致。
async fn wait_for_codex_ready(ws_url: &str) -> CliResult<()> {
    let deadline = Instant::now() + CODEX_STARTUP_TIMEOUT;

    loop {
        match connect_async(ws_url).await {
            Ok((stream, _)) => {
                drop(stream);
                return Ok(());
            }
            Err(err) => {
                let detail = err.to_string();
                if Instant::now() >= deadline {
                    return Err(CliError::network(format!(
                        "timed out waiting for Codex app-server at {}: {}",
                        ws_url, detail
                    )));
                }
                sleep(CODEX_STARTUP_POLL_INTERVAL).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Working directory resolution
// 中文：该注释与英文“Working directory resolution”含义一致。
// ---------------------------------------------------------------------------

/// Resolves the effective working directory for the code session.
///     中文：该注释与英文“Resolves the effective working directory for the code session.”含义一致。
///
/// Priority: `--cwd` > `--repo` > current working directory.
///     中文：该注释与英文“Priority: `--cwd` > `--repo` > current working directory.”含义一致。
/// Validates that the resolved path exists and is a directory.
///     中文：该注释与英文“Validates that the resolved path exists and is a directory.”含义一致。
/// `--cwd` and `--repo` are mutually exclusive.
///     中文：该注释与英文“`--cwd` and `--repo` are mutually exclusive.”含义一致。
pub(crate) fn resolve_code_preflight_working_dir(args: &CodeArgs) -> CliResult<PathBuf> {
    resolve_code_working_dir(args)
}

fn resolve_code_working_dir(args: &CodeArgs) -> CliResult<PathBuf> {
    if args.cwd.is_some() && args.repo.is_some() {
        return Err(CliError::command_usage(
            "--cwd and --repo cannot be used together".to_string(),
        ));
    }

    let working_dir = args
        .cwd
        .clone()
        .or_else(|| args.repo.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let flag = if args.repo.is_some() {
        "--repo"
    } else {
        "--cwd"
    };
    validate_code_working_dir(working_dir, flag)
}

fn validate_code_working_dir(working_dir: PathBuf, flag: &str) -> CliResult<PathBuf> {
    if !working_dir.exists() {
        return Err(CliError::command_usage(format!(
            "{flag} path does not exist: {}",
            working_dir.display()
        )));
    }
    if !working_dir.is_dir() {
        return Err(CliError::command_usage(format!(
            "{flag} must point to a directory: {}",
            working_dir.display()
        )));
    }
    Ok(working_dir)
}

// ---------------------------------------------------------------------------
// TUI launch configuration and model abstraction
// 中文：该注释与英文“TUI launch configuration and model abstraction”含义一致。
// ---------------------------------------------------------------------------

/// Aggregates all parameters needed to launch the TUI application.
///     中文：该注释与英文“Aggregates all parameters needed to launch the TUI application.”含义一致。
///
/// This struct is built once in [`execute_tui`] and consumed by
///     中文：该注释与英文“This struct is built once in [`execute_tui`] and consumed by”含义一致。
/// [`run_tui_with_model`]. It bundles network config, tool registry,
///     中文：该注释与英文“[`run_tui_with_model`]. It bundles network config, tool registry,”含义一致。
/// prompt/temperature settings, session state, and inter-component channels.
///     中文：该注释与英文“prompt/temperature settings, session state, and inter-component channels.”含义一致。
struct TuiLaunchConfig {
    host: String,
    port: u16,
    mcp_port: u16,
    registry: Arc<ToolRegistry>,
    preamble: String,
    temperature: Option<f64>,
    thinking: Option<CompletionThinking>,
    reasoning_effort: Option<CompletionReasoningEffort>,
    stream: Option<bool>,
    preserve_reasoning_content: bool,
    allowed_tools: Option<Vec<String>>,
    auto_classify_first_user_message: bool,
    context: Option<CodeContext>,
    resume_thread_id: Option<String>,
    approval_policy: AskForApproval,
    allow_all_commands: bool,
    approval_ttl: Duration,
    approval_cache_policy: ApprovalCachePolicy,
    network_access: bool,
    user_input_rx: tokio::sync::mpsc::UnboundedReceiver<UserInputRequest>,
    exec_approval_rx: tokio::sync::mpsc::UnboundedReceiver<ExecApprovalRequest>,
    exec_approval_tx: tokio::sync::mpsc::UnboundedSender<ExecApprovalRequest>,
    mcp_server: Arc<LibraMcpServer>,
    control_runtime: ControlRuntimeConfig,
    browser_control: BrowserControlMode,
    /// Goal objective passed via `libra code --goal`. The TUI app
    ///     中文：该注释与英文“Goal objective passed via `libra code --goal`. The TUI app”含义一致。
    /// uses this to bootstrap a `GoalSpec` and seed
    ///     中文：该注释与英文“uses this to bootstrap a `GoalSpec` and seed”含义一致。
    /// [`AppConfig::initial_goal`] before the first turn.
    ///     中文：该注释与英文“[`AppConfig::initial_goal`] before the first turn.”含义一致。
    initial_goal: Option<String>,
}

#[derive(Clone)]
struct ManagedCodeRuntimeModel;

impl CompletionModel for ManagedCodeRuntimeModel {
    type Response = ();

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        Err(CompletionError::NotImplemented(
            "managed code runtime handles turns outside the generic completion model".to_string(),
        ))
    }
}

fn build_tui_code_ui_capabilities() -> CodeUiCapabilities {
    CodeUiCapabilities {
        message_input: true,
        streaming_text: true,
        plan_updates: true,
        tool_calls: true,
        patchsets: true,
        interactive_approvals: true,
        structured_questions: true,
        provider_session_resume: false,
    }
}

fn build_tui_code_ui_transcript(session: &SessionState) -> Vec<CodeUiTranscriptEntry> {
    session
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let kind = match message.role.as_str() {
                "user" => CodeUiTranscriptEntryKind::UserMessage,
                "assistant" => CodeUiTranscriptEntryKind::AssistantMessage,
                _ => return None,
            };
            Some(CodeUiTranscriptEntry {
                id: format!("session-message-{}", index + 1),
                kind,
                title: Some(match message.role.as_str() {
                    "user" => "Developer".to_string(),
                    _ => "Assistant".to_string(),
                }),
                content: Some(message.content.clone()),
                status: Some("completed".to_string()),
                streaming: false,
                metadata: serde_json::json!({ "restored": true }),
                created_at: message.timestamp,
                updated_at: message.timestamp,
            })
        })
        .collect()
}

fn session_canonical_thread_id(session: &SessionState) -> Option<String> {
    ["thread_id", "threadId", "canonical_thread_id"]
        .iter()
        .find_map(|key| session.metadata.get(*key).and_then(|value| value.as_str()))
        .map(str::to_string)
        .or_else(|| {
            Uuid::parse_str(&session.id)
                .ok()
                .map(|thread_id| thread_id.to_string())
        })
}

#[allow(clippy::too_many_arguments)]
async fn build_tui_code_ui_runtime(
    working_dir: &str,
    session: &SessionState,
    provider_name: &str,
    model_name: &str,
    projection_bundle: Option<&ThreadBundle>,
    code_control_tx: Option<tokio::sync::mpsc::UnboundedSender<TuiControlCommand>>,
    automation_write_enabled: bool,
    browser_write_enabled: bool,
    lease_duration_override: Option<chrono::Duration>,
) -> Arc<CodeUiRuntimeHandle> {
    let capabilities = build_tui_code_ui_capabilities();
    let provider = CodeUiProviderInfo {
        provider: provider_name.to_string(),
        model: Some(model_name.to_string()),
        mode: Some("tui".to_string()),
        managed: false,
    };
    let mut snapshot = if let Some(bundle) = projection_bundle {
        snapshot_from_thread_bundle(
            working_dir.to_string(),
            provider,
            capabilities.clone(),
            bundle,
        )
    } else {
        initial_snapshot(working_dir.to_string(), provider, capabilities.clone())
    };
    if projection_bundle.is_none() {
        snapshot.session_id = session.id.clone();
        snapshot.thread_id = session_canonical_thread_id(session);
    }
    snapshot.transcript = build_tui_code_ui_transcript(session);
    snapshot.updated_at = Utc::now();

    let code_ui_session = CodeUiSession::new(snapshot);
    let adapter: Arc<dyn CodeUiProviderAdapter> = if let Some(control_tx) = code_control_tx {
        TuiCodeUiAdapter::new(code_ui_session, capabilities, control_tx)
    } else {
        ReadOnlyCodeUiAdapter::new(code_ui_session, capabilities)
    };
    // `LocalTui` keeps the terminal as the visible owner but still lets
    // 中文：该注释与英文“`LocalTui` keeps the terminal as the visible owner but still lets”含义一致。
    // browser/automation leases attach when their write surface is enabled.
    // 中文：该注释与英文“browser/automation leases attach when their write surface is enabled.”含义一致。
    // `Fixed { Tui }` is reserved for sessions where neither writer should
    // 中文：该注释与英文“`Fixed { Tui }` is reserved for sessions where neither writer should”含义一致。
    // ever be allowed to take control (read-only browser observe).
    // 中文：该注释与英文“ever be allowed to take control (read-only browser observe).”含义一致。
    let initial_controller = if automation_write_enabled || browser_write_enabled {
        CodeUiInitialController::LocalTui {
            owner_label: "Terminal UI".to_string(),
            reason: Some("The terminal UI controls this live session".to_string()),
        }
    } else {
        CodeUiInitialController::Fixed {
            kind: CodeUiControllerKind::Tui,
            owner_label: "Terminal UI".to_string(),
            reason: Some("The terminal UI controls this live session".to_string()),
        }
    };
    let mut runtime_options = CodeUiRuntimeOptions::new(
        browser_write_enabled,
        automation_write_enabled,
        initial_controller,
    );
    runtime_options.lease_duration = lease_duration_override;
    CodeUiRuntimeHandle::build_with_options(adapter, runtime_options).await
}

async fn load_code_ui_projection_bundle(
    working_dir: &Path,
    session: &SessionState,
    thread_id: Uuid,
) -> anyhow::Result<Option<ThreadBundle>> {
    let storage_root = resolve_storage_root(working_dir);
    let session_store = SessionStore::from_storage_path(&storage_root);
    let session_jsonl_store = SessionJsonlStore::new(session_store.session_root(&session.id));
    let db_path = storage_root.join("libra.db");
    let db_path = db_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("database path is not valid UTF-8"))?;
    let db_conn = establish_connection(db_path).await?;
    let storage = Arc::new(LocalStorage::new(storage_root.join("objects")));
    let history = HistoryManager::new(storage.clone(), storage_root, Arc::new(db_conn.clone()));
    let rebuilder = ProjectionRebuilder::new(storage.as_ref(), &history);
    let resolver = ProjectionResolver::new(db_conn);
    let Some(resume_bundle) = resolver.load_for_resume(thread_id, &rebuilder).await? else {
        return Ok(None);
    };

    if let Err(error) = append_resume_audit_frame(&session_jsonl_store, session, &resume_bundle) {
        tracing::warn!(
            %thread_id,
            %error,
            "failed to append resume audit context frame"
        );
    }

    Ok(Some(ThreadBundle {
        thread: resume_bundle.thread,
        scheduler: resume_bundle.scheduler,
        freshness: resume_bundle.freshness,
    }))
}

fn append_resume_audit_frame(
    session_jsonl_store: &SessionJsonlStore,
    session: &SessionState,
    resume_bundle: &ResumeBundle,
) -> anyhow::Result<()> {
    let session_root = session_jsonl_store.session_root();
    let attachments = ContextAttachmentStore::new(session_root);
    let payload = serde_json::json!({
        "session_id": session.id,
        "thread_id": resume_bundle.thread.thread_id,
        "freshness": resume_bundle.freshness,
        "phase_at_resume": resume_bundle.phase_at_resume,
        "resume_reason": resume_bundle.resume_reason,
        "resume_actions": resume_bundle.resume_actions,
    })
    .to_string();

    let frame = ContextFrameBuilder::new(ContextFrameKind::ResumeAudit, ContextBudget::default())
        .push(
            ContextFrameCandidate::new("resume-audit", ContextSegmentKind::SourceContext, payload)
                .source(ContextFrameSource::runtime("resume"))
                .trust(ContextTrustLevel::Trusted)
                .non_compressible(true),
        )
        .build(&attachments)
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to build resume audit context frame for session {}: {error}",
                session.id
            )
        })?;

    session_jsonl_store
        .append(&SessionEvent::context_frame(frame))
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to append resume audit context frame for session {}: {error}",
                session.id
            )
        })?;

    Ok(())
}

/// Core TUI lifecycle: wires up the terminal, background servers, agent
///     中文：该注释与英文“Core TUI lifecycle: wires up the terminal, background servers, agent”含义一致。
/// configuration, session persistence, and the interactive `App` event loop.
///     中文：该注释与英文“configuration, session persistence, and the interactive `App` event loop.”含义一致。
///
/// This function is generic over the completion model `M`, allowing all
///     中文：该注释与英文“This function is generic over the completion model `M`, allowing all”含义一致。
/// providers to share the same TUI setup code. The flow is:
///     中文：该注释与英文“providers to share the same TUI setup code. The flow is:”含义一致。
///
/// 1. Load git hooks from the working directory.
///     中文：该注释与英文“1. Load git hooks from the working directory.”含义一致。
/// 2. Build the agent's `ToolLoopConfig` (preamble, temperature, sandbox policy).
///     中文：该注释与英文“2. Build the agent's `ToolLoopConfig` (preamble, temperature, sandbox policy).”含义一致。
/// 3. Initialize the terminal via `tui_init()` with a restore guard.
///     中文：该注释与英文“3. Initialize the terminal via `tui_init()` with a restore guard.”含义一致。
/// 4. Start the web server and MCP server as background tasks.
///     中文：该注释与英文“4. Start the web server and MCP server as background tasks.”含义一致。
/// 5. Load slash commands and agent profiles from disk.
///     中文：该注释与英文“5. Load slash commands and agent profiles from disk.”含义一致。
/// 6. Restore or create a new session.
///     中文：该注释与英文“6. Restore or create a new session.”含义一致。
/// 7. Run the `App` event loop until the user exits.
///     中文：该注释与英文“7. Run the `App` event loop until the user exits.”含义一致。
/// 8. Gracefully shut down all background servers.
///     中文：该注释与英文“8. Gracefully shut down all background servers.”含义一致。
///
/// # Side Effects
///     中文：标题：Side Effects。
/// - Switches the terminal into TUI mode and restores it on exit.
///     中文：列表项说明与英文“Switches the terminal into TUI mode and restores it on exit.”含义一致。
/// - Starts background web and MCP listeners when their ports are available.
///     中文：列表项说明与英文“Starts background web and MCP listeners when their ports are available.”含义一致。
/// - Reads hook, slash-command, profile, session, and projection state from the
///     中文：列表项说明与英文“Reads hook, slash-command, profile, session, and projection state from the”含义一致。
///   working directory.
///     中文：该注释与英文“working directory.”含义一致。
/// - Persists session updates and may drive tool-mediated workspace writes.
///     中文：列表项说明与英文“Persists session updates and may drive tool-mediated workspace writes.”含义一致。
///
/// # Errors
///     中文：标题：Errors。
/// Returns [`CliError`] for terminal initialization failures, invalid resume
///     中文：该注释与英文“Returns [`CliError`] for terminal initialization failures, invalid resume”含义一致。
/// thread IDs, missing sessions, session/projection load failures, or fatal app
///     中文：该注释与英文“thread IDs, missing sessions, session/projection load failures, or fatal app”含义一致。
/// exits reported by the TUI event loop.
///     中文：该注释与英文“exits reported by the TUI event loop.”含义一致。
async fn run_tui_with_model<M>(
    model: M,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
) -> CliResult<()>
where
    M: CompletionModel + Clone + 'static,
    M::Response: CompletionUsage,
{
    run_tui_with_model_inner(model, params, model_name, provider_name, None).await
}

async fn run_tui_with_managed_code_runtime(
    code_ui_runtime: Arc<CodeUiRuntimeHandle>,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
) -> CliResult<()> {
    run_tui_with_model_inner(
        ManagedCodeRuntimeModel,
        params,
        model_name,
        provider_name,
        Some(code_ui_runtime),
    )
    .await
}

async fn run_tui_with_model_inner<M>(
    model: M,
    params: TuiLaunchConfig,
    model_name: String,
    provider_name: String,
    managed_code_ui_runtime: Option<Arc<CodeUiRuntimeHandle>>,
) -> CliResult<()>
where
    M: CompletionModel + Clone + 'static,
    M::Response: CompletionUsage,
{
    let registry = params.registry;
    let control_runtime = params.control_runtime;
    let browser_control = params.browser_control;
    let hook_runner = {
        let runner = HookRunner::load(registry.working_dir());
        if runner.has_hooks() {
            Some(std::sync::Arc::new(runner))
        } else {
            None
        }
    };

    let mut config = ToolLoopConfig {
        preamble: Some(params.preamble),
        temperature: params.temperature,
        thinking: params.thinking,
        reasoning_effort: params.reasoning_effort,
        stream: params.stream,
        hook_runner,
        allowed_tools: params.allowed_tools,
        runtime_context: Some(default_tui_runtime_context(
            registry.working_dir(),
            params.context,
            DefaultTuiApprovalConfig {
                policy: params.approval_policy,
                allow_all_commands: params.allow_all_commands,
                ttl: params.approval_ttl,
                cache_policy: params.approval_cache_policy,
            },
            params.network_access,
            params.exec_approval_tx.clone(),
        )),
        max_turns: None,
        preserve_reasoning_content: params.preserve_reasoning_content,
        ..Default::default()
    };

    // Initialize terminal.
    // 中文：该注释与英文“Initialize terminal.”含义一致。
    let terminal = match tui_init() {
        Ok(t) => t,
        Err(e) => return Err(CliError::io(format!("failed to initialize terminal: {e}"))),
    };

    // INVARIANT: every successful `tui_init` must install this guard before any
    // 中文：该注释与英文“INVARIANT: every successful `tui_init` must install this guard before any”含义一致。
    // await point that can fail, otherwise a later error could leave the user's
    // 中文：该注释与英文“await point that can fail, otherwise a later error could leave the user's”含义一致。
    // terminal in raw/alternate-screen mode.
    // 中文：该注释与英文“terminal in raw/alternate-screen mode.”含义一致。
    let _guard = scopeguard::guard((), |_| {
        let _ = tui_restore();
    });

    let tui = Tui::new(terminal);

    // Set up session persistence
    // 中文：该注释与英文“Set up session persistence”含义一致。
    let working_dir_str = registry.working_dir().to_string_lossy().to_string();
    let storage_root = resolve_storage_root(registry.working_dir());
    let session_store = SessionStore::from_storage_path(&storage_root);
    let session = if let Some(thread_id) = params.resume_thread_id.as_deref() {
        // The resume identifier may be either a canonical UUID (planning-bound
        // 中文：该注释与英文“The resume identifier may be either a canonical UUID (planning-bound”含义一致。
        // thread) or a chat-flow session id from `generate_session_id`
        // 中文：该注释与英文“thread) or a chat-flow session id from `generate_session_id`”含义一致。
        // (millisecond-hex / pid-hex / counter-hex). The store accepts either
        // 中文：该注释与英文“(millisecond-hex / pid-hex / counter-hex). The store accepts either”含义一致。
        // shape — reject empty input here and let `load_for_thread_id` surface
        // 中文：该注释与英文“shape — reject empty input here and let `load_for_thread_id` surface”含义一致。
        // a unified "no session found" error for any unknown identifier.
        // 中文：该注释与英文“a unified "no session found" error for any unknown identifier.”含义一致。
        if thread_id.trim().is_empty() {
            return Err(CliError::command_usage(
                "--resume requires a non-empty thread_id",
            ));
        }
        match session_store.load_for_thread_id(thread_id, &working_dir_str) {
            Ok(Some(session)) => session,
            Ok(None) => {
                return Err(CliError::fatal(format!(
                    "no Libra Code session found for thread_id '{thread_id}' in working directory '{working_dir_str}'"
                )));
            }
            Err(error) => {
                return Err(CliError::io(format!(
                    "failed to load Libra Code session for thread_id '{thread_id}': {error}"
                )));
            }
        }
    } else {
        SessionState::new(&working_dir_str)
    };
    // v0.17.791 session-bootstrap usage auto-prune: if the
    // 中文：该注释与英文“v0.17.791 session-bootstrap usage auto-prune: if the”含义一致。
    // operator configured `[usage] retention_days = N` in
    // 中文：该注释与英文“operator configured `[usage] retention_days = N` in”含义一致。
    // `config.toml`, drop usage rows older than N days at session
    // 中文：该注释与英文“`config.toml`, drop usage rows older than N days at session”含义一致。
    // start. Soft-failure (logs warn + continues) so a malformed
    // 中文：该注释与英文“start. Soft-failure (logs warn + continues) so a malformed”含义一致。
    // config or DB error doesn't block startup.
    // 中文：该注释与英文“config or DB error doesn't block startup.”含义一致。
    crate::command::usage::auto_prune_at_session_start(&storage_root).await;

    if let Some(usage_recorder) = build_usage_recorder(&storage_root).await {
        config.usage_recorder = Some(usage_recorder);
        config.usage_context = Some(UsageContext {
            session_id: Some(session.id.clone()),
            thread_id: session_canonical_thread_id(&session),
            agent_run_id: None,
            run_id: None,
            provider: provider_name.clone(),
            model: model_name.clone(),
            request_kind: "completion".to_string(),
            intent: None,
            // OC-Phase 5 P5.2: single-agent legacy path. The
            // 中文：该注释与英文“OC-Phase 5 P5.2: single-agent legacy path. The”含义一致。
            // dispatcher (P5.3) sets this to the active profile name
            // 中文：该注释与英文“dispatcher (P5.3) sets this to the active profile name”含义一致。
            // when multi-agent is enabled.
            // 中文：该注释与英文“when multi-agent is enabled.”含义一致。
            agent_name: None,
        });
    }

    let automation_write_enabled = control_runtime.is_write();
    let browser_write_enabled = browser_control == BrowserControlMode::Loopback;
    // The TUI control command channel is created whenever any writer
    // 中文：该注释与英文“The TUI control command channel is created whenever any writer”含义一致。
    // (automation or browser) is enabled, so the runtime adapter can route
    // 中文：该注释与英文“(automation or browser) is enabled, so the runtime adapter can route”含义一致。
    // submit/respond/cancel into the TUI app loop. Selecting the adapter
    // 中文：该注释与英文“submit/respond/cancel into the TUI app loop. Selecting the adapter”含义一致。
    // based on `code_control_tx.is_some()` would gate browser writes behind
    // 中文：该注释与英文“based on `code_control_tx.is_some()` would gate browser writes behind”含义一致。
    // `--control write`; gating on the explicit booleans avoids that.
    // 中文：该注释与英文“`--control write`; gating on the explicit booleans avoids that.”含义一致。
    let (code_control_tx, code_control_rx) = if automation_write_enabled || browser_write_enabled {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<TuiControlCommand>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let code_ui_runtime = if let Some(runtime) = managed_code_ui_runtime.clone() {
        if let Some(control_tx) = code_control_tx {
            let adapter = runtime.adapter();
            let code_ui_session = adapter.session();
            let capabilities = adapter.capabilities();
            let tui_adapter: Arc<dyn CodeUiProviderAdapter> =
                TuiCodeUiAdapter::new(code_ui_session, capabilities, control_tx);
            let mut runtime_options = CodeUiRuntimeOptions::new(
                browser_write_enabled,
                automation_write_enabled,
                CodeUiInitialController::LocalTui {
                    owner_label: "Terminal UI".to_string(),
                    reason: Some("The terminal UI controls this live managed session".to_string()),
                },
            );
            runtime_options.lease_duration = code_ui_test_lease_duration_override()?;
            CodeUiRuntimeHandle::build_with_options(tui_adapter, runtime_options).await
        } else {
            runtime
        }
    } else {
        let projection_bundle = session_canonical_thread_id(&session)
            .and_then(|thread_id| Uuid::parse_str(&thread_id).ok());
        let projection_bundle = match projection_bundle {
            Some(thread_id) => {
                match load_code_ui_projection_bundle(registry.working_dir(), &session, thread_id)
                    .await
                {
                    Ok(bundle) => bundle,
                    Err(error) => {
                        tracing::warn!(%thread_id, error = %error, "failed to load projection-backed code ui snapshot; falling back to session state");
                        None
                    }
                }
            }
            None => None,
        };
        build_tui_code_ui_runtime(
            &working_dir_str,
            &session,
            &provider_name,
            &model_name,
            projection_bundle.as_ref(),
            code_control_tx,
            automation_write_enabled,
            browser_write_enabled,
            code_ui_test_lease_duration_override()?,
        )
        .await
    };
    let code_ui_session = code_ui_runtime.adapter().session();
    params
        .mcp_server
        .set_code_ui_session(code_ui_session.clone());
    let code_ui_runtime_for_app = code_ui_runtime.clone();

    let control_thread_id = session_canonical_thread_id(&session);
    let (mut web_handle, web_line) = match start_web_server(
        &params.host,
        params.port,
        registry.working_dir().to_path_buf(),
        WebServerOptions {
            code_ui: Some(code_ui_runtime),
            automation_control_token: control_runtime.token.clone(),
            audit_sink: None,
        },
    )
    .await
    {
        Ok(handle) => {
            let base_url = format!("http://{}", handle.addr);
            if let Err(error) = control_runtime.write_info_file(
                registry.working_dir(),
                base_url.clone(),
                None,
                control_thread_id.clone(),
            ) {
                handle.shutdown().await;
                if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                    let _ = runtime.shutdown().await;
                }
                return Err(error);
            }
            let line = format!("Web: {base_url}");
            (Some(handle), line)
        }
        Err(err) if control_runtime.is_write() => {
            if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                let _ = runtime.shutdown().await;
            }
            return Err(
                CliError::network(format!("failed to start web server: {err}"))
                    .with_detail("component", "web_server"),
            );
        }
        Err(err) => (
            None::<WebServerHandle>,
            format!("Web: failed to start ({err})"),
        ),
    };
    let control_base_url = web_handle
        .as_ref()
        .map(|handle| format!("http://{}", handle.addr));

    // Start MCP Server
    // 中文：该注释与英文“Start MCP Server”含义一致。
    let (mcp_handle, mcp_line) =
        match start_mcp_server(&params.host, params.mcp_port, params.mcp_server.clone()).await {
            Ok(handle) => {
                let mcp_url = format!("http://{}", handle.addr);
                if let Some(base_url) = control_base_url.as_ref()
                    && let Err(error) = control_runtime.write_info_file(
                        registry.working_dir(),
                        base_url.clone(),
                        Some(mcp_url.clone()),
                        control_thread_id.clone(),
                    )
                {
                    if let Some(handle) = web_handle.take() {
                        handle.shutdown().await;
                    }
                    handle.shutdown().await;
                    if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                        let _ = runtime.shutdown().await;
                    }
                    return Err(error);
                }
                let line = format!("MCP: {mcp_url}");
                (Some(handle), line)
            }
            Err(err) if control_runtime.is_write() => {
                if let Some(handle) = web_handle.take() {
                    handle.shutdown().await;
                }
                if let Some(runtime) = managed_code_ui_runtime.as_ref() {
                    let _ = runtime.shutdown().await;
                }
                return Err(
                    CliError::network(format!("failed to start MCP server: {err}"))
                        .with_detail("component", "mcp_server"),
                );
            }
            Err(err) => (None, format!("MCP: failed to start ({err})")),
        };

    let input_guidance = if managed_code_ui_runtime.is_some() {
        "Type your message and press Enter to work with the managed provider."
    } else {
        "Type a development request and press Enter to generate a reviewable plan before execution."
    };
    let welcome = format!("Welcome to Libra Code! {input_guidance}\n{web_line}\n{mcp_line}");

    // Load slash commands
    // 中文：该注释与英文“Load slash commands”含义一致。
    let commands = load_commands(registry.working_dir());
    let command_dispatcher = CommandDispatcher::new(commands);
    let skills = load_skills(registry.working_dir());
    let skill_dispatcher = SkillDispatcher::new(skills);

    // Load agent profiles
    // 中文：该注释与英文“Load agent profiles”含义一致。
    let profiles = load_profiles(registry.working_dir());
    let agent_router = AgentProfileRouter::new(profiles);
    // OC-Phase 5 P5.1 session bootstrap (v0.17.775): read the
    // 中文：该注释与英文“OC-Phase 5 P5.1 session bootstrap (v0.17.775): read the”含义一致。
    // operator's `.libra/agents.toml` if present so
    // 中文：该注释与英文“operator's `.libra/agents.toml` if present so”含义一致。
    // `code.sub_agents.enabled` / `code.multi_agent.enabled` /
    // 中文：该注释与英文“`code.sub_agents.enabled` / `code.multi_agent.enabled` /”含义一致。
    // `[code.budget]` / `[code.agents.*]` etc. actually take
    // 中文：该注释与英文“`[code.budget]` / `[code.agents.*]` etc. actually take”含义一致。
    // effect. Missing file degrades to `AgentsConfig::default()`
    // 中文：该注释与英文“effect. Missing file degrades to `AgentsConfig::default()`”含义一致。
    // (the previous hardcoded behavior) per `load_or_default`'s
    // 中文：该注释与英文“(the previous hardcoded behavior) per `load_or_default`'s”含义一致。
    // contract. Parse errors are surfaced as a warning rather than
    // 中文：该注释与英文“contract. Parse errors are surfaced as a warning rather than”含义一致。
    // failing the session — a malformed config should not block an
    // 中文：该注释与英文“failing the session — a malformed config should not block an”含义一致。
    // operator from starting `libra code` to fix it.
    // 中文：该注释与英文“operator from starting `libra code` to fix it.”含义一致。
    let agents_config_path = registry.working_dir().join(".libra").join("agents.toml");
    let agents_config = AgentsConfig::load_or_default(&agents_config_path).unwrap_or_else(|err| {
        tracing::warn!(
            error = %err,
            path = %agents_config_path.display(),
            "failed to load agents.toml; falling back to AgentsConfig::default()",
        );
        AgentsConfig::default()
    });
    // v0.17.804 source_call_log persistence wire-up: build the
    // 中文：该注释与英文“v0.17.804 source_call_log persistence wire-up: build the”含义一致。
    // pool with the per-session SeaORM connection so every
    // 中文：该注释与英文“pool with the per-session SeaORM connection so every”含义一致。
    // SourcePool tool call lands a `source_call_log` row. Soft
    // 中文：该注释与英文“SourcePool tool call lands a `source_call_log` row. Soft”含义一致。
    // fallback to `SourcePool::new()` (in-memory only) if the DB
    // 中文：该注释与英文“fallback to `SourcePool::new()` (in-memory only) if the DB”含义一致。
    // path can't be resolved or the connection fails — same
    // 中文：该注释与英文“path can't be resolved or the connection fails — same”含义一致。
    // posture as `build_usage_recorder` further down so session
    // 中文：该注释与英文“posture as `build_usage_recorder` further down so session”含义一致。
    // bootstrap never blocks on a telemetry-layer issue.
    // 中文：该注释与英文“bootstrap never blocks on a telemetry-layer issue.”含义一致。
    let source_pool = {
        let db_path = storage_root.join(DATABASE);
        let db_path_str = db_path.to_string_lossy();
        match establish_connection(&db_path_str).await {
            Ok(conn) => SourcePool::with_persistence(Arc::new(conn)),
            Err(err) => {
                tracing::warn!(
                    %err,
                    path = %db_path.display(),
                    "failed to open repo DB for SourcePool persistence; \
                     falling back to in-memory-only source call log",
                );
                SourcePool::new()
            }
        }
    };
    if let Err(error) = register_builtin_mcp_source_from_project_config(
        &source_pool,
        params.mcp_server.clone(),
        registry.working_dir(),
    ) {
        tracing::warn!("failed to register built-in MCP source: {error}");
    }
    config.source_pool = Some(source_pool.clone());
    config.source_session_id = Some(session.id.clone());

    // OC-Phase 3 P3.4 session bootstrap (v0.17.776): when the
    // 中文：该注释与英文“OC-Phase 3 P3.4 session bootstrap (v0.17.776): when the”含义一致。
    // operator's agents.toml flips `code.sub_agents.enabled =
    // 中文：该注释与英文“operator's agents.toml flips `code.sub_agents.enabled =”含义一致。
    // true`, build the full `SubAgentToolLoopRuntime` so the
    // 中文：该注释与英文“true`, build the full `SubAgentToolLoopRuntime` so the”含义一致。
    // `task` tool actually routes through the dispatcher.
    // 中文：该注释与英文“`task` tool actually routes through the dispatcher.”含义一致。
    //
    // Required parent context fields are sourced as:
    // 中文：该注释与英文“Required parent context fields are sourced as:”含义一致。
    //   - dispatcher: DefaultSubAgentDispatcher::new(registry, cfg)
    // 中文：列表项说明与英文“dispatcher: DefaultSubAgentDispatcher::new(registry, cfg)”含义一致。
    //     .with_default_child_runner()
    // 中文：该注释与英文“.with_default_child_runner()”含义一致。
    //   - permission_service: a `DenyByDefaultPermissionAsker`
    // 中文：列表项说明与英文“permission_service: a `DenyByDefaultPermissionAsker`”含义一致。
    //     fallback (interactive prompt wiring is a follow-up).
    // 中文：该注释与英文“fallback (interactive prompt wiring is a follow-up).”含义一致。
    //     `UserInitiated{bypass_permission_ask:true}` /task
    // 中文：该注释与英文“`UserInitiated{bypass_permission_ask:true}` /task”含义一致。
    //     paths work; LlmInitiated paths that need escalation
    // 中文：该注释与英文“paths work; LlmInitiated paths that need escalation”含义一致。
    //     get rejected with an actionable feedback message.
    // 中文：该注释与英文“get rejected with an actionable feedback message.”含义一致。
    //   - parent_model_binding: ModelBinding from CLI flags.
    // 中文：列表项说明与英文“parent_model_binding: ModelBinding from CLI flags.”含义一致。
    //   - parent_agent: minimal `AgentExecutionSpec` with the
    // 中文：列表项说明与英文“parent_agent: minimal `AgentExecutionSpec` with the”含义一致。
    //     CLI-resolved model — enough for dispatcher gates
    // 中文：该注释与英文“CLI-resolved model — enough for dispatcher gates”含义一致。
    //     (depth/concurrency/feature flag) which never reach
    // 中文：该注释与英文“(depth/concurrency/feature flag) which never reach”含义一致。
    //     into the parent_agent's tool/permission spec.
    // 中文：该注释与英文“into the parent_agent's tool/permission spec.”含义一致。
    //   - All other Arc'd state is sourced from values already
    // 中文：列表项说明与英文“All other Arc'd state is sourced from values already”含义一致。
    //     constructed earlier in this function.
    // 中文：该注释与英文“constructed earlier in this function.”含义一致。
    //
    // Failure-to-build is logged and the runtime stays None —
    // 中文：该注释与英文“Failure-to-build is logged and the runtime stays None —”含义一致。
    // `code.sub_agents.enabled = true` with a malformed agents
    // 中文：该注释与英文“`code.sub_agents.enabled = true` with a malformed agents”含义一致。
    // block degrades to the same "task tool not available" UX
    // 中文：该注释与英文“block degrades to the same "task tool not available" UX”含义一致。
    // an operator sees with the flag off.
    // 中文：该注释与英文“an operator sees with the flag off.”含义一致。
    //
    // OC-Phase 4 P4.4 diagnostic (v0.17.783): if the operator
    // 中文：该注释与英文“OC-Phase 4 P4.4 diagnostic (v0.17.783): if the operator”含义一致。
    // configured `[code.compaction]`, log the resolved model
    // 中文：该注释与英文“configured `[code.compaction]`, log the resolved model”含义一致。
    // binding so an operator can confirm the binding round-trip
    // 中文：该注释与英文“binding so an operator can confirm the binding round-trip”含义一致。
    // works before the dispatcher-side integration lands. A
    // 中文：该注释与英文“works before the dispatcher-side integration lands. A”含义一致。
    // future commit consumes this binding in
    // 中文：该注释与英文“future commit consumes this binding in”含义一致。
    // `build_subagent_runtime_for_session` to route parent
    // 中文：该注释与英文“`build_subagent_runtime_for_session` to route parent”含义一致。
    // frames through `run_compaction(...)` before feeding the
    // 中文：该注释与英文“frames through `run_compaction(...)` before feeding the”含义一致。
    // child via `ContextHandoff::to_handoff_messages`.
    // 中文：该注释与英文“child via `ContextHandoff::to_handoff_messages`.”含义一致。
    if let Some(binding) = agents_config.compaction_model_binding() {
        tracing::info!(
            provider = %binding.provider_id,
            model = %binding.model_id,
            "compaction model binding resolved from [code.compaction]; \
             dispatcher integration is a v0.17.783+ follow-up",
        );
    }
    if agents_config.sub_agents.enabled {
        match build_subagent_runtime_for_session(
            &agents_config,
            registry.clone(),
            &session,
            &session_store,
            &storage_root,
            &model_name,
            &provider_name,
            &agent_router,
            config.hook_runner.clone(),
            // Hand the dispatcher the parent tool loop's resolved
            // 中文：该注释与英文“Hand the dispatcher the parent tool loop's resolved”含义一致。
            // runtime context (sandbox / approval / file-history)
            // 中文：该注释与英文“runtime context (sandbox / approval / file-history)”含义一致。
            // so dispatched sub-agents inherit the parent's authority
            // 中文：该注释与英文“so dispatched sub-agents inherit the parent's authority”含义一致。
            // rather than running unsandboxed (S2-INV-06).
            // 中文：该注释与英文“rather than running unsandboxed (S2-INV-06).”含义一致。
            config.runtime_context.clone(),
        )
        .await
        {
            Ok(runtime) => {
                tracing::info!(
                    enabled = true,
                    max_depth = agents_config.multi_agent.max_subagent_depth,
                    // Log the EFFECTIVE concurrency (CEX-S2-12 caps it to
                    // 中文：该注释与英文“Log the EFFECTIVE concurrency (CEX-S2-12 caps it to”含义一致。
                    // 1), not the configured value, so the diagnostic
                    // 中文：该注释与英文“1), not the configured value, so the diagnostic”含义一致。
                    // matches what the dispatcher actually enforces.
                    // 中文：该注释与英文“matches what the dispatcher actually enforces.”含义一致。
                    max_concurrent = cex_s2_12_subagent_concurrency_cap(
                        agents_config.multi_agent.max_concurrent_subagents,
                    ),
                    "sub-agent dispatcher attached to tool_loop config",
                );
                config.subagent_runtime = Some(runtime);
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "failed to build SubAgentToolLoopRuntime; the `task` tool will surface \
                     'sub_agents.enabled = true required' until this is resolved",
                );
            }
        }
    }

    let managed_runtime_for_shutdown = managed_code_ui_runtime.clone();
    let auto_classify_first_user_message =
        params.auto_classify_first_user_message && managed_code_ui_runtime.is_none();

    // Create and run app
    // 中文：该注释与英文“Create and run app”含义一致。
    let mut app = App::new(
        tui,
        model,
        registry,
        config,
        AppConfig {
            welcome_message: welcome,
            command_dispatcher,
            skill_dispatcher,
            agent_router,
            agents_config,
            session,
            session_store,
            user_input_rx: params.user_input_rx,
            exec_approval_rx: params.exec_approval_rx,
            model_name,
            provider_name,
            mcp_server: Some(params.mcp_server),
            code_ui_session: Some(code_ui_session),
            code_ui_runtime: Some(code_ui_runtime_for_app),
            code_control_rx,
            managed_code_ui_runtime,
            default_network_access: params.network_access,
            auto_classify_first_user_message,
            initial_goal: params.initial_goal.clone(),
            source_pool,
        },
    );

    let graph_thread_hint = match app.run().await {
        Ok(exit_info) => {
            if let ExitReason::Fatal(msg) = exit_info.reason {
                return Err(
                    CliError::fatal(msg).with_stable_code(StableErrorCode::InternalInvariant)
                );
            }
            exit_info.thread_id
        }
        Err(e) => return Err(CliError::internal(format!("TUI exited unexpectedly: {e}"))),
    };

    if let Some(handle) = web_handle {
        handle.shutdown().await;
    }
    if let Some(handle) = mcp_handle {
        handle.shutdown().await;
    }
    if let Some(runtime) = managed_runtime_for_shutdown {
        let _ = runtime.shutdown().await;
    }
    if let Some(thread_id) = graph_thread_hint {
        println!("Inspect this thread graph with: libra graph {thread_id}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// MCP server — Streamable HTTP transport via Hyper
// 中文：该注释与英文“MCP server — Streamable HTTP transport via Hyper”含义一致。
// ---------------------------------------------------------------------------

/// Starts the MCP server using `rmcp`'s Streamable HTTP transport.
///     中文：该注释与英文“Starts the MCP server using `rmcp`'s Streamable HTTP transport.”含义一致。
///
/// Each incoming TCP connection is handled by a Hyper service that wraps the
///     中文：该注释与英文“Each incoming TCP connection is handled by a Hyper service that wraps the”含义一致。
/// `StreamableHttpService`. Per-connection tasks are tracked in `connection_tasks`
///     中文：该注释与英文“`StreamableHttpService`. Per-connection tasks are tracked in `connection_tasks`”含义一致。
/// so they can be aborted during shutdown, preventing task leaks.
///     中文：该注释与英文“so they can be aborted during shutdown, preventing task leaks.”含义一致。
///
/// Uses `LocalSessionManager` for session management (single-node, in-memory).
///     中文：该注释与英文“Uses `LocalSessionManager` for session management (single-node, in-memory).”含义一致。
async fn start_mcp_server(
    host: &str,
    port: u16,
    mcp_server: Arc<LibraMcpServer>,
) -> anyhow::Result<McpServerHandle> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;

    // Use rmcp's Streamable HTTP transport via Hyper directly
    // 中文：该注释与英文“Use rmcp's Streamable HTTP transport via Hyper directly”含义一致。
    let service = TowerToHyperService::new(StreamableHttpService::new(
        move || Ok(mcp_server.clone()),
        LocalSessionManager::default().into(),
        Default::default(),
    ));

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let connection_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let tracked_connections = connection_tasks.clone();

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    break;
                }
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _)) => {
                            let io = TokioIo::new(stream);
                            let service = service.clone();
                            let conn_task = tokio::spawn(async move {
                                if let Err(e) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::default())
                                    .serve_connection(io, service)
                                    .await
                                {
                                    cli_error!(e, "warning: MCP connection error");
                                }
                            });
                            match tracked_connections.lock() {
                                Ok(mut tasks) => {
                                    tasks.retain(|task| !task.is_finished());
                                    tasks.push(conn_task);
                                }
                                Err(_) => conn_task.abort(),
                            }
                        }
                        Err(e) => {
                            cli_error!(e, "warning: MCP accept error");
                        }
                    }
                }
            }
        }
        Ok(())
    });

    Ok(McpServerHandle {
        addr: bound_addr,
        shutdown_tx,
        join,
        connection_tasks,
    })
}

// ---------------------------------------------------------------------------
// System prompt and runtime context construction
// 中文：该注释与英文“System prompt and runtime context construction”含义一致。
// ---------------------------------------------------------------------------

/// Builds the system prompt (preamble) for the AI agent, incorporating the
///     中文：该注释与英文“Builds the system prompt (preamble) for the AI agent, incorporating the”含义一致。
/// working directory context and optional operating mode (dev/review/research).
///     中文：该注释与英文“working directory context and optional operating mode (dev/review/research).”含义一致。
fn system_preamble(
    working_dir: &std::path::Path,
    context: Option<CodeContext>,
    provider: CodeProvider,
    model: Option<&str>,
) -> String {
    let intent = task_intent_for_context(context);
    let budget = ContextBudget::for_provider_model(
        context_budget_provider_name(provider),
        model.unwrap_or_else(|| default_context_budget_model(provider)),
    );
    let mut builder = SystemPromptBuilder::new(working_dir)
        .with_intent(intent)
        .with_dynamic_context()
        .with_context_budget(budget);
    if let Some(ctx) = context {
        let mode = match ctx {
            CodeContext::Dev => ContextMode::Dev,
            CodeContext::Review => ContextMode::Review,
            CodeContext::Research => ContextMode::Research,
        };
        builder = builder.with_context(mode);
    }
    builder.build()
}

fn context_budget_provider_name(provider: CodeProvider) -> &'static str {
    match provider {
        CodeProvider::Gemini => "gemini",
        CodeProvider::Openai => "openai",
        CodeProvider::Anthropic => "anthropic",
        CodeProvider::Deepseek => "deepseek",
        CodeProvider::Kimi => "kimi",
        CodeProvider::Zhipu => "zhipu",
        CodeProvider::Ollama => "ollama",
        CodeProvider::Codex => "codex",
        #[cfg(feature = "test-provider")]
        CodeProvider::Fake => "fake",
    }
}

fn default_context_budget_model(provider: CodeProvider) -> &'static str {
    match provider {
        CodeProvider::Gemini => GEMINI_2_5_FLASH,
        CodeProvider::Openai => GPT_4O_MINI,
        CodeProvider::Anthropic => CLAUDE_3_5_SONNET,
        CodeProvider::Deepseek => "deepseek-chat",
        CodeProvider::Kimi => KIMI_K2_6,
        CodeProvider::Zhipu => GLM_5,
        CodeProvider::Ollama => "ollama-default",
        CodeProvider::Codex => "codex",
        #[cfg(feature = "test-provider")]
        CodeProvider::Fake => FAKE_DEFAULT_MODEL,
    }
}

fn task_intent_for_context(context: Option<CodeContext>) -> TaskIntent {
    match context {
        Some(CodeContext::Dev) => TaskIntent::Feature,
        Some(CodeContext::Review) => TaskIntent::Review,
        Some(CodeContext::Research) => TaskIntent::Question,
        None => TaskIntent::Unknown,
    }
}

/// Constructs the default [`ToolRuntimeContext`] for TUI mode, configuring
///     中文：该注释与英文“Constructs the default [`ToolRuntimeContext`] for TUI mode, configuring”含义一致。
/// the sandbox policy based on the operating context:
///     中文：该注释与英文“the sandbox policy based on the operating context:”含义一致。
///
/// - **Dev mode (or no context)**: Workspace-write sandbox allowing modifications
///     中文：列表项说明与英文“**Dev mode (or no context)**: Workspace-write sandbox allowing modifications”含义一致。
///   within the working directory; network access follows the developer's
///     中文：该注释与英文“within the working directory; network access follows the developer's”含义一致。
///   selected policy.
///     中文：该注释与英文“selected policy.”含义一致。
/// - **Review / Research mode**: Read-only sandbox; no writes or network access.
///     中文：列表项说明与英文“**Review / Research mode**: Read-only sandbox; no writes or network access.”含义一致。
///
/// The approval policy and its communication channel are also wired in here.
///     中文：该注释与英文“The approval policy and its communication channel are also wired in here.”含义一致。
#[derive(Clone, Debug, PartialEq, Eq)]
struct DefaultTuiApprovalConfig {
    policy: AskForApproval,
    allow_all_commands: bool,
    ttl: Duration,
    cache_policy: ApprovalCachePolicy,
}

fn default_tui_runtime_context(
    working_dir: &std::path::Path,
    context: Option<CodeContext>,
    approval: DefaultTuiApprovalConfig,
    network_access: bool,
    exec_approval_tx: tokio::sync::mpsc::UnboundedSender<ExecApprovalRequest>,
) -> ToolRuntimeContext {
    let policy = match context {
        Some(CodeContext::Review | CodeContext::Research) => SandboxPolicy::ReadOnly,
        Some(CodeContext::Dev) | None => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![working_dir.to_path_buf()],
            network_access: NetworkAccess::from_legacy_bool(network_access),
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    };

    let mut approval_store = ApprovalStore::default();
    if approval.allow_all_commands {
        approval_store.approve_all_commands();
    }

    ToolRuntimeContext {
        sandbox: Some(ToolSandboxContext {
            policy,
            permissions: SandboxPermissions::UseDefault,
        }),
        sandbox_runtime: None,
        approval: Some(ToolApprovalContext {
            policy: approval.policy,
            request_tx: exec_approval_tx,
            store: Arc::new(tokio::sync::Mutex::new(approval_store)),
            scope_key_prefix: None,
            approval_ttl: approval.ttl,
            cache_policy: approval.cache_policy,
        }),
        file_history: None,
        max_output_bytes: None,
    }
}

#[derive(Debug, Deserialize)]
struct ApprovalProjectConfig {
    approval: Option<ApprovalSectionConfig>,
}

#[derive(Debug, Deserialize)]
struct ApprovalSectionConfig {
    ttl_seconds: Option<u64>,
    #[serde(default)]
    protected_branches: Option<Vec<String>>,
    #[serde(default)]
    allowed_network_domains: Option<Vec<String>>,
    #[serde(default)]
    no_cache_unknown_network: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ApprovalRuntimeConfig {
    ttl: Option<Duration>,
    cache_policy: ApprovalCachePolicy,
}

fn approval_config_from_project_config(working_dir: &Path) -> ApprovalRuntimeConfig {
    let path = working_dir.join(".libra").join("config.toml");
    let Some(contents) = fs::read_to_string(&path).ok() else {
        return ApprovalRuntimeConfig::default();
    };
    let Ok(config) = toml::from_str::<ApprovalProjectConfig>(&contents).map_err(|err| {
        tracing::warn!(
            target: "libra::command::code",
            path = %path.display(),
            error = %err,
            "failed to parse approval config"
        );
        err
    }) else {
        return ApprovalRuntimeConfig::default();
    };
    let Some(approval) = config.approval else {
        return ApprovalRuntimeConfig::default();
    };
    let ttl = approval.ttl_seconds.and_then(|ttl_seconds| {
        if ttl_seconds == 0 {
            tracing::warn!(
                target: "libra::command::code",
                path = %path.display(),
                "ignoring approval ttl_seconds=0"
            );
            None
        } else {
            Some(Duration::from_secs(ttl_seconds))
        }
    });

    let default_cache_policy = ApprovalCachePolicy::default();
    ApprovalRuntimeConfig {
        ttl,
        cache_policy: ApprovalCachePolicy {
            protected_branches: approval
                .protected_branches
                .unwrap_or(default_cache_policy.protected_branches),
            allowed_network_domains: approval.allowed_network_domains.unwrap_or_default(),
            no_cache_unknown_network: approval.no_cache_unknown_network,
            // OC-Phase 2 P2.5: the persistent ruleset is loaded lazily by
            // 中文：该注释与英文“OC-Phase 2 P2.5: the persistent ruleset is loaded lazily by”含义一致。
            // the runtime once it has a `DatabaseConnection`; the project-
            // 中文：该注释与英文“the runtime once it has a `DatabaseConnection`; the project-”含义一致。
            // config-derived policy starts with no projection attached.
            // 中文：该注释与英文“config-derived policy starts with no projection attached.”含义一致。
            approved_ruleset: None,
        },
    }
}

#[cfg(test)]
fn approval_ttl_from_project_config(working_dir: &Path) -> Option<Duration> {
    approval_config_from_project_config(working_dir).ttl
}

#[cfg(test)]
fn approval_cache_policy_from_project_config(working_dir: &Path) -> ApprovalCachePolicy {
    approval_config_from_project_config(working_dir).cache_policy
}

// ---------------------------------------------------------------------------
// MCP server initialization — storage and database setup
// 中文：该注释与英文“MCP server initialization — storage and database setup”含义一致。
// ---------------------------------------------------------------------------

/// Initializes the [`LibraMcpServer`] instance with optional history persistence.
///     中文：该注释与英文“Initializes the [`LibraMcpServer`] instance with optional history persistence.”含义一致。
///
/// Sets up the local object storage directory and SQLite database under the
///     中文：该注释与英文“Sets up the local object storage directory and SQLite database under the”含义一致。
/// `.libra/` storage root. If any step fails (directory creation, DB connection),
///     中文：该注释与英文“`.libra/` storage root. If any step fails (directory creation, DB connection),”含义一致。
/// falls back to a read-only MCP server with history disabled, printing a warning.
///     中文：该注释与英文“falls back to a read-only MCP server with history disabled, printing a warning.”含义一致。
///
/// # Side Effects
///     中文：标题：Side Effects。
/// - Creates the local object storage directory when possible.
///     中文：列表项说明与英文“Creates the local object storage directory when possible.”含义一致。
/// - Opens a SQLite connection for intent/run history when the DB path is usable.
///     中文：列表项说明与英文“Opens a SQLite connection for intent/run history when the DB path is usable.”含义一致。
/// - Prints warnings to stderr before falling back to history-disabled mode.
///     中文：列表项说明与英文“Prints warnings to stderr before falling back to history-disabled mode.”含义一致。
///
/// # Errors
///     中文：标题：Errors。
/// This helper intentionally does not return errors. It converts storage/DB
///     中文：该注释与英文“This helper intentionally does not return errors. It converts storage/DB”含义一致。
/// setup failures into a read-only MCP server so AI clients can still inspect
///     中文：该注释与英文“setup failures into a read-only MCP server so AI clients can still inspect”含义一致。
/// files and continue a degraded session.
///     中文：该注释与英文“files and continue a degraded session.”含义一致。
async fn init_mcp_server(working_dir: &std::path::Path) -> Arc<LibraMcpServer> {
    let storage_dir = resolve_storage_root(working_dir);
    let objects_dir = storage_dir.join("objects");
    let dot_libra = storage_dir;

    // Try to create the directory. If it fails, we assume read-only or permission issues.
    // 中文：该注释与英文“Try to create the directory. If it fails, we assume read-only or permission issues.”含义一致。
    if let Err(e) = std::fs::create_dir_all(&objects_dir) {
        eprintln!(
            "Warning: Failed to create storage directory: {}. Running in read-only mode (history/context disabled). Error: {}",
            objects_dir.display(),
            e
        );
        return Arc::new(LibraMcpServer::new_with_working_dir(
            None,
            None,
            working_dir.to_path_buf(),
        ));
    }

    // Connect to DB
    // 中文：该注释与英文“Connect to DB”含义一致。
    let db_path = dot_libra.join("libra.db");
    let Some(db_path_str) = db_path.to_str() else {
        eprintln!(
            "Warning: Database path is not valid UTF-8: {}. History disabled.",
            db_path.display()
        );
        return Arc::new(LibraMcpServer::new_with_working_dir(
            None,
            None,
            working_dir.to_path_buf(),
        ));
    };

    #[cfg(target_os = "windows")]
    let db_path_string = db_path_str.replace("\\", "/");
    #[cfg(target_os = "windows")]
    let db_path_str = &db_path_string;

    let db_conn = match establish_connection(db_path_str).await {
        Ok(conn) => Arc::new(conn),
        Err(e) => {
            eprintln!(
                "Warning: Failed to connect to database: {}. History disabled.",
                e
            );
            return Arc::new(LibraMcpServer::new_with_working_dir(
                None,
                None,
                working_dir.to_path_buf(),
            ));
        }
    };

    let storage = Arc::new(ClientStorage::init(objects_dir));
    let intent_history_manager = Arc::new(HistoryManager::new(storage.clone(), dot_libra, db_conn));
    Arc::new(LibraMcpServer::new_with_working_dir(
        Some(intent_history_manager),
        Some(storage),
        working_dir.to_path_buf(),
    ))
}

/// Resolves the `.libra/` storage root for the given working directory.
///     中文：该注释与英文“Resolves the `.libra/` storage root for the given working directory.”含义一致。
///
/// Supports linked worktrees by delegating to `try_get_storage_path`, which
///     中文：该注释与英文“Supports linked worktrees by delegating to `try_get_storage_path`, which”含义一致。
/// follows `.libra` symlinks to the main repository's storage. Falls back to
///     中文：该注释与英文“follows `.libra` symlinks to the main repository's storage. Falls back to”含义一致。
/// `<working_dir>/.libra` if resolution fails.
///     中文：该注释与英文“`<working_dir>/.libra` if resolution fails.”含义一致。
pub(crate) fn resolve_storage_root(working_dir: &std::path::Path) -> std::path::PathBuf {
    try_get_storage_path(Some(working_dir.to_path_buf()))
        .unwrap_or_else(|_| working_dir.join(".libra"))
}

/// CEX-S2-12 "single sub-agent behind flag" concurrency cap.
///     中文：该注释与英文“CEX-S2-12 "single sub-agent behind flag" concurrency cap.”含义一致。
///
/// While the `code.sub_agents.enabled` gate is the only path that
///     中文：该注释与英文“While the `code.sub_agents.enabled` gate is the only path that”含义一致。
/// builds a [`SubAgentToolLoopRuntime`], CEX-S2-12 must run at most one
///     中文：该注释与英文“builds a [`SubAgentToolLoopRuntime`], CEX-S2-12 must run at most one”含义一致。
/// concurrent sub-agent regardless of the operator-configured
///     中文：该注释与英文“concurrent sub-agent regardless of the operator-configured”含义一致。
/// `code.multi_agent.max_concurrent_subagents` (and the
///     中文：该注释与英文“`code.multi_agent.max_concurrent_subagents` (and the”含义一致。
/// `code.sub_agents.max_parallel` schema default of `2`). Real
///     中文：该注释与英文“`code.sub_agents.max_parallel` schema default of `2`). Real”含义一致。
/// parallelism stays locked until CEX-S2-14 wires the scheduler-side
///     中文：该注释与英文“parallelism stays locked until CEX-S2-14 wires the scheduler-side”含义一致。
/// observer budget — at which point this returns `configured` instead
///     中文：该注释与英文“observer budget — at which point this returns `configured` instead”含义一致。
/// of the forced `1`.
///     中文：该注释与英文“of the forced `1`.”含义一致。
///
/// Kept as a named pure function (rather than a literal `1` at the call
///     中文：该注释与英文“Kept as a named pure function (rather than a literal `1` at the call”含义一致。
/// site) so the cap is documented, greppable, and pinned by a unit test
///     中文：该注释与英文“site) so the cap is documented, greppable, and pinned by a unit test”含义一致。
/// against a silent regression to passing the operator value through.
///     中文：该注释与英文“against a silent regression to passing the operator value through.”含义一致。
const fn cex_s2_12_subagent_concurrency_cap(_configured: u32) -> u32 {
    1
}

/// Construct a [`SubAgentToolLoopRuntime`] from the libra-code
///     中文：该注释与英文“Construct a [`SubAgentToolLoopRuntime`] from the libra-code”含义一致。
/// session's resolved state. Called from the session bootstrap
///     中文：该注释与英文“session's resolved state. Called from the session bootstrap”含义一致。
/// when `agents_config.sub_agents.enabled = true`; failures
///     中文：该注释与英文“when `agents_config.sub_agents.enabled = true`; failures”含义一致。
/// degrade to "task tool unavailable" rather than blocking
///     中文：该注释与英文“degrade to "task tool unavailable" rather than blocking”含义一致。
/// session startup.
///     中文：该注释与英文“session startup.”含义一致。
///
/// The runtime is shared (cloned by `Option<...>::clone()` since
///     中文：该注释与英文“The runtime is shared (cloned by `Option<...>::clone()` since”含义一致。
/// every field is `Arc`-wrapped or trivially copyable inside its
///     中文：该注释与英文“every field is `Arc`-wrapped or trivially copyable inside its”含义一致。
/// own owning newtype). Per-call `dispatch_context(call_id)`
///     中文：该注释与英文“own owning newtype). Per-call `dispatch_context(call_id)`”含义一致。
/// captures a fresh `parent_message_id` for each `task` tool
///     中文：该注释与英文“captures a fresh `parent_message_id` for each `task` tool”含义一致。
/// invocation; the rest of the parent context is stable for the
///     中文：该注释与英文“invocation; the rest of the parent context is stable for the”含义一致。
/// session.
///     中文：该注释与英文“session.”含义一致。
#[allow(clippy::too_many_arguments)]
async fn build_subagent_runtime_for_session(
    agents_config: &AgentsConfig,
    registry: std::sync::Arc<ToolRegistry>,
    session: &SessionState,
    session_store: &SessionStore,
    storage_root: &Path,
    model_name: &str,
    provider_name: &str,
    agent_router: &AgentProfileRouter,
    hook_runner: Option<std::sync::Arc<crate::internal::ai::hooks::HookRunner>>,
    runtime_context: Option<ToolRuntimeContext>,
) -> anyhow::Result<crate::internal::ai::agent::runtime::SubAgentToolLoopRuntime> {
    use crate::internal::ai::{
        agent::{
            profile::{AgentExecutionSpec, AgentMode, ModelBinding},
            runtime::{
                AbortToken, ChannelPermissionAsker, ContextFrameLoader, DefaultSubAgentDispatcher,
                MultiAgentConfig, PermissionAsker, PermissionReply, PermissionService,
                SubAgentToolLoopRuntime,
            },
        },
        providers::{ProviderBuildOptions, ProviderFactory},
        session::jsonl::SessionJsonlStore,
    };

    let agent_spec_registry = agents_config
        .build_agent_registry()
        .map_err(|err| anyhow::anyhow!("agents.toml validation failed: {err}"))?;

    let dispatcher = DefaultSubAgentDispatcher::new(
        agent_spec_registry,
        MultiAgentConfig {
            enabled: agents_config.multi_agent.enabled,
            // `agents_config.multi_agent` carries u32 for both
            // 中文：该注释与英文“`agents_config.multi_agent` carries u32 for both”含义一致。
            // limits to preserve TOML round-trip; the runtime's
            // 中文：该注释与英文“limits to preserve TOML round-trip; the runtime's”含义一致。
            // `MultiAgentConfig` narrows depth to u8 (a depth of
            // 中文：该注释与英文“`MultiAgentConfig` narrows depth to u8 (a depth of”含义一致。
            // 256+ is meaningless — that's a recursion bug not a
            // 中文：该注释与英文“256+ is meaningless — that's a recursion bug not a”含义一致。
            // legitimate config). Saturating cast keeps the
            // 中文：该注释与英文“legitimate config). Saturating cast keeps the”含义一致。
            // semantics safe when an operator sets a huge u32.
            // 中文：该注释与英文“semantics safe when an operator sets a huge u32.”含义一致。
            max_subagent_depth: agents_config
                .multi_agent
                .max_subagent_depth
                .min(u8::MAX as u32) as u8,
            // CEX-S2-12 "single sub-agent behind flag": force the
            // 中文：该注释与英文“CEX-S2-12 "single sub-agent behind flag": force the”含义一致。
            // dispatcher concurrency to 1 regardless of the configured
            // 中文：该注释与英文“dispatcher concurrency to 1 regardless of the configured”含义一致。
            // value; CEX-S2-14 unlocks the operator's real budget.
            // 中文：该注释与英文“value; CEX-S2-14 unlocks the operator's real budget.”含义一致。
            max_concurrent_subagents: cex_s2_12_subagent_concurrency_cap(
                agents_config.multi_agent.max_concurrent_subagents,
            ),
        },
    )
    .with_default_child_runner()
    // CEX-S2-12 / S2-INV-03: confine each dispatched sub-agent to a
    // 中文：该注释与英文“CEX-S2-12 / S2-INV-03: confine each dispatched sub-agent to a”含义一致。
    // materialized per-run workspace so its writes never touch the main
    // 中文：该注释与英文“materialized per-run workspace so its writes never touch the main”含义一致。
    // worktree. `sessions_root` = the `.libra/sessions` dir the per-run
    // 中文：该注释与英文“worktree. `sessions_root` = the `.libra/sessions` dir the per-run”含义一致。
    // `AgentRunEventStore` records the `WorkspaceMaterialized` event
    // 中文：该注释与英文“`AgentRunEventStore` records the `WorkspaceMaterialized` event”含义一致。
    // under (transcript path `sessions_root/{thread}/agents/{run}.jsonl`).
    // 中文：该注释与英文“under (transcript path `sessions_root/{thread}/agents/{run}.jsonl`).”含义一致。
    .with_workspace_isolation(
        crate::internal::ai::agent::runtime::WorkspaceIsolationConfig {
            fuse_state: crate::internal::ai::orchestrator::workspace::FuseProvisionState::default(),
            sessions_root: storage_root.join("sessions"),
            allow_full_copy: agents_config.multi_agent.allow_full_copy,
        },
    );

    // OC-Phase 3 P3.4 / P3.7 interactive permission asker (v0.17.788):
    // 中文：该注释与英文“OC-Phase 3 P3.4 / P3.7 interactive permission asker (v0.17.788):”含义一致。
    // construct a ChannelPermissionAsker + spawn a background
    // 中文：该注释与英文“construct a ChannelPermissionAsker + spawn a background”含义一致。
    // consumer task that auto-rejects each ask while emitting a
    // 中文：该注释与英文“consumer task that auto-rejects each ask while emitting a”含义一致。
    // structured tracing event with the full ask context. This is
    // 中文：该注释与英文“structured tracing event with the full ask context. This is”含义一致。
    // the channel-plumbing wire-up that proves the path end-to-end;
    // 中文：该注释与英文“the channel-plumbing wire-up that proves the path end-to-end;”含义一致。
    // the follow-up replaces the auto-reject consumer with a real
    // 中文：该注释与英文“the follow-up replaces the auto-reject consumer with a real”含义一致。
    // TUI prompt widget that surfaces each ask interactively.
    // 中文：该注释与英文“TUI prompt widget that surfaces each ask interactively.”含义一致。
    //
    // The consumer task lives for the entire session — when the
    // 中文：该注释与英文“The consumer task lives for the entire session — when the”含义一致。
    // session exits, the sender drops, the receiver's `recv()`
    // 中文：该注释与英文“session exits, the sender drops, the receiver's `recv()`”含义一致。
    // returns None, and the task ends cleanly.
    // 中文：该注释与英文“returns None, and the task ends cleanly.”含义一致。
    let (permission_ask_tx, mut permission_ask_rx) = tokio::sync::mpsc::unbounded_channel::<
        crate::internal::ai::agent::runtime::ChannelPermissionAsk,
    >();
    tokio::spawn(async move {
        while let Some(ask) = permission_ask_rx.recv().await {
            tracing::warn!(
                permission = %ask.permission,
                patterns = ?ask.patterns,
                thread_id = %ask.thread_id,
                session_id = %ask.session_id,
                source = ?ask.source,
                "permission ask received via ChannelPermissionAsker; \
                 auto-rejecting until interactive TUI prompt widget lands",
            );
            // Send may fail if the dispatcher dropped its
            // 中文：该注释与英文“Send may fail if the dispatcher dropped its”含义一致。
            // oneshot receiver (e.g. cancelled mid-await). Ignore
            // 中文：该注释与英文“oneshot receiver (e.g. cancelled mid-await). Ignore”含义一致。
            // the send error — the dispatcher already handles a
            // 中文：该注释与英文“the send error — the dispatcher already handles a”含义一致。
            // closed reply channel by surfacing Reject.
            // 中文：该注释与英文“closed reply channel by surfacing Reject.”含义一致。
            let _ = ask.reply_tx.send(PermissionReply::Reject {
                feedback: Some(
                    "permission ask auto-rejected by the v0.17.788 channel consumer; \
                     pre-grant the permission via [code.agents.<name>.permission] in \
                     .libra/agents.toml or wait for the interactive TUI widget"
                        .to_string(),
                ),
            });
        }
    });
    let permission_service = PermissionService::new(std::sync::Arc::new(
        ChannelPermissionAsker::new(permission_ask_tx),
    ) as std::sync::Arc<dyn PermissionAsker>);

    let parent_model_binding = ModelBinding::parse(&format!("{provider_name}/{model_name}"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "failed to parse parent ModelBinding from provider={provider_name} model={model_name}"
            )
        })?;

    // OC-Phase 3 P3.4 router-resolved parent_agent (v0.17.780):
    // 中文：该注释与英文“OC-Phase 3 P3.4 router-resolved parent_agent (v0.17.780):”含义一致。
    // if the operator has authored a `.libra/agents/primary.md`
    // 中文：该注释与英文“if the operator has authored a `.libra/agents/primary.md`”含义一致。
    // (or any `.md` profile named "primary"), use it as the
    // 中文：该注释与英文“(or any `.md` profile named "primary"), use it as the”含义一致。
    // sub-agent dispatcher's parent_agent. The CLI flags still
    // 中文：该注释与英文“sub-agent dispatcher's parent_agent. The CLI flags still”含义一致。
    // win for the model binding because the operator's `libra
    // 中文：该注释与英文“win for the model binding because the operator's `libra”含义一致。
    // code --model <X>` should override the profile's default
    // 中文：该注释与英文“code --model <X>` should override the profile's default”含义一致。
    // model — sub-agents inherit the session's actual model, not
    // 中文：该注释与英文“model — sub-agents inherit the session's actual model, not”含义一致。
    // the profile's static one. Falls back to the v0.17.776
    // 中文：该注释与英文“the profile's static one. Falls back to the v0.17.776”含义一致。
    // placeholder when no profile is found.
    // 中文：该注释与英文“placeholder when no profile is found.”含义一致。
    let parent_agent = match agent_router.execution_spec("primary") {
        Some(mut spec) => {
            // The router-supplied spec carries the profile's
            // 中文：该注释与英文“The router-supplied spec carries the profile's”含义一致。
            // declared model binding, but the session's actual
            // 中文：该注释与英文“declared model binding, but the session's actual”含义一致。
            // model is what the CLI resolved — sub-agents should
            // 中文：该注释与英文“model is what the CLI resolved — sub-agents should”含义一致。
            // see the same model the parent is talking to, not
            // 中文：该注释与英文“see the same model the parent is talking to, not”含义一致。
            // the profile's default.
            // 中文：该注释与英文“the profile's default.”含义一致。
            spec.model = Some(parent_model_binding.clone());
            spec
        }
        None => AgentExecutionSpec {
            name: "parent".to_string(),
            description: "libra-code primary agent (session bootstrap default)".to_string(),
            mode: AgentMode::Primary,
            model: Some(parent_model_binding.clone()),
            ..AgentExecutionSpec::default()
        },
    };

    let session_jsonl_store = SessionJsonlStore::new(session_store.session_root(&session.id));
    let usage_recorder =
        std::sync::Arc::new(build_usage_recorder(storage_root).await.ok_or_else(|| {
            anyhow::anyhow!(
                "usage recorder unavailable; sub-agent dispatcher requires the SQLite DB \
                 — check storage_root permissions"
            )
        })?);
    let context_frame_loader = std::sync::Arc::new(ContextFrameLoader::default());

    // OC-Phase 4 P4.4 compaction model (v0.17.784): when the
    // 中文：该注释与英文“OC-Phase 4 P4.4 compaction model (v0.17.784): when the”含义一致。
    // operator configured `[code.compaction]`, build a
    // 中文：该注释与英文“operator configured `[code.compaction]`, build a”含义一致。
    // `CompletionModel` for it so the dispatcher tail can route
    // 中文：该注释与英文“`CompletionModel` for it so the dispatcher tail can route”含义一致。
    // parent frames through `run_compaction(...)`. Failures
    // 中文：该注释与英文“parent frames through `run_compaction(...)`. Failures”含义一致。
    // here degrade to None — the v0.17.773 raw-segment handoff
    // 中文：该注释与英文“here degrade to None — the v0.17.773 raw-segment handoff”含义一致。
    // path stays operational. We log + warn on failure rather
    // 中文：该注释与英文“path stays operational. We log + warn on failure rather”含义一致。
    // than aborting the whole runtime construction so a
    // 中文：该注释与英文“than aborting the whole runtime construction so a”含义一致。
    // misconfigured compaction model doesn't break operators
    // 中文：该注释与英文“misconfigured compaction model doesn't break operators”含义一致。
    // who have correctly configured sub-agents.
    // 中文：该注释与英文“who have correctly configured sub-agents.”含义一致。
    let compaction_model = match agents_config.compaction_model_binding() {
        Some(binding) => match ProviderFactory.build(&binding, ProviderBuildOptions::default()) {
            Ok(model) => Some(std::sync::Arc::new(model)),
            Err(err) => {
                tracing::warn!(
                    %err,
                    provider = %binding.provider_id,
                    model = %binding.model_id,
                    "failed to build compaction model from [code.compaction]; \
                     falling back to raw-segment handoff",
                );
                None
            }
        },
        None => None,
    };

    Ok(SubAgentToolLoopRuntime {
        dispatcher: std::sync::Arc::new(dispatcher),
        parent_thread_id: session_canonical_thread_id(session)
            .unwrap_or_else(|| session.id.clone()),
        parent_session_id: session.id.clone(),
        parent_agent,
        parent_ruleset: Vec::new(),
        parent_model_binding,
        permission_service: std::sync::Arc::new(permission_service),
        session_store: session_jsonl_store,
        provider_factory: std::sync::Arc::new(ProviderFactory),
        provider_build_options: ProviderBuildOptions::default(),
        provider_build_options_resolver: None,
        tool_registry: (*registry).clone(),
        // S2-INV-06: hand the child the parent session's resolved
        // 中文：该注释与英文“S2-INV-06: hand the child the parent session's resolved”含义一致。
        // runtime sandbox / approval / file-history authority so its
        // 中文：该注释与英文“runtime sandbox / approval / file-history authority so its”含义一致。
        // tool invocations run under the same gates the parent does.
        // 中文：该注释与英文“tool invocations run under the same gates the parent does.”含义一致。
        // `DefaultSubAgentChildRunner::run` forwards this into the
        // 中文：该注释与英文“`DefaultSubAgentChildRunner::run` forwards this into the”含义一致。
        // child's `ToolLoopConfig.runtime_context`; before it was
        // 中文：该注释与英文“child's `ToolLoopConfig.runtime_context`; before it was”含义一致。
        // populated here the child ran every tool call with `None`
        // 中文：该注释与英文“populated here the child ran every tool call with `None`”含义一致。
        // (no sandbox, approval defaulting to `Skip`) — strictly more
        // 中文：该注释与英文“(no sandbox, approval defaulting to `Skip`) — strictly more”含义一致。
        // permissive than the parent. This is authority *inheritance*,
        // 中文：该注释与英文“permissive than the parent. This is authority *inheritance*,”含义一致。
        // not workspace *isolation* (S2-INV-03): the child still shares
        // 中文：该注释与英文“not workspace *isolation* (S2-INV-03): the child still shares”含义一致。
        // the parent's `writable_roots`; rebasing those onto a
        // 中文：该注释与英文“the parent's `writable_roots`; rebasing those onto a”含义一致。
        // materialized per-run workspace is a separate follow-on.
        // 中文：该注释与英文“materialized per-run workspace is a separate follow-on.”含义一致。
        runtime_context,
        compaction_model,
        usage_recorder,
        context_frame_loader,
        abort_token: AbortToken::new(),
        depth: 0,
        // v0.17.807 S2-INV-13 hook dispatch: the parent's
        // 中文：该注释与英文“v0.17.807 S2-INV-13 hook dispatch: the parent's”含义一致。
        // `HookRunner` (loaded at `code.rs:2554` via
        // 中文：该注释与英文“`HookRunner` (loaded at `code.rs:2554` via”含义一致。
        // `HookRunner::load(...)`) is now threaded through here
        // 中文：该注释与英文“`HookRunner::load(...)`) is now threaded through here”含义一致。
        // so child sub-agents inherit the same PreToolUse /
        // 中文：该注释与英文“so child sub-agents inherit the same PreToolUse /”含义一致。
        // PostToolUse hook surface as the parent. Sub-agents
        // 中文：该注释与英文“PostToolUse hook surface as the parent. Sub-agents”含义一致。
        // cannot disable or supersede the parent's runner.
        // 中文：该注释与英文“cannot disable or supersede the parent's runner.”含义一致。
        hook_runner,
    })
}

async fn build_usage_recorder(storage_root: &Path) -> Option<UsageRecorder> {
    let db_path = storage_root.join(DATABASE);
    let Some(db_path) = db_path.to_str() else {
        tracing::warn!(
            path = %storage_root.display(),
            "usage stats disabled because the repository database path is not valid UTF-8"
        );
        return None;
    };
    match establish_connection(db_path).await {
        Ok(conn) => {
            let pricing = usage_price_table_from_project_config(storage_root);
            Some(UsageRecorder::with_pricing(conn, pricing))
        }
        Err(error) => {
            tracing::warn!("usage stats disabled because database open failed: {error}");
            None
        }
    }
}

fn usage_price_table_from_project_config(storage_root: &Path) -> UsagePriceTable {
    let path = storage_root.join("config.toml");
    let Ok(contents) = fs::read_to_string(&path) else {
        return UsagePriceTable::new();
    };
    match UsagePriceTable::from_project_config_toml(&contents) {
        Ok(pricing) => pricing,
        Err(error) => {
            tracing::warn!(
                target: "libra::command::code",
                path = %path.display(),
                error = %error,
                "failed to parse usage pricing config; using built-in pricing table"
            );
            UsagePriceTable::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Mode: Stdio — MCP server over stdin/stdout
// 中文：该注释与英文“Mode: Stdio — MCP server over stdin/stdout”含义一致。
// ---------------------------------------------------------------------------

/// Runs the MCP server over stdin/stdout using `rmcp`'s async read/write
///     中文：该注释与英文“Runs the MCP server over stdin/stdout using `rmcp`'s async read/write”含义一致。
/// transport. This mode is designed for integration with AI clients (e.g.
///     中文：该注释与英文“transport. This mode is designed for integration with AI clients (e.g.”含义一致。
/// Claude Desktop) that communicate via the Model Context Protocol over pipes.
///     中文：该注释与英文“Claude Desktop) that communicate via the Model Context Protocol over pipes.”含义一致。
///
/// Blocks until the MCP session ends (client disconnects or EOF on stdin).
///     中文：该注释与英文“Blocks until the MCP session ends (client disconnects or EOF on stdin).”含义一致。
///
/// # Side Effects
///     中文：标题：Side Effects。
/// - Takes ownership of process stdin/stdout for the MCP transport.
///     中文：列表项说明与英文“Takes ownership of process stdin/stdout for the MCP transport.”含义一致。
/// - Initializes the same history/object-backed MCP server used by other modes.
///     中文：列表项说明与英文“Initializes the same history/object-backed MCP server used by other modes.”含义一致。
///
/// # Errors
///     中文：标题：Errors。
/// Returns [`CliError`] when working-dir resolution fails, the MCP server cannot
///     中文：该注释与英文“Returns [`CliError`] when working-dir resolution fails, the MCP server cannot”含义一致。
/// start on stdio, or the running MCP session reports an unrecoverable error.
///     中文：该注释与英文“start on stdio, or the running MCP session reports an unrecoverable error.”含义一致。
async fn execute_stdio(args: &CodeArgs) -> CliResult<()> {
    let working_dir = resolve_code_working_dir(args)?;

    let mcp_server = init_mcp_server(&working_dir).await;

    use rmcp::{
        service::serve_server,
        transport::{async_rw::AsyncRwTransport, io::stdio},
    };

    let (stdin, stdout) = stdio();
    let transport = AsyncRwTransport::new_server(stdin, stdout);

    match serve_server(mcp_server, transport).await {
        Ok(running) => {
            if let Err(e) = running.waiting().await {
                return Err(CliError::internal(format!("MCP Stdio server error: {}", e)));
            }
        }
        Err(e) => {
            return Err(CliError::network(format!(
                "failed to start MCP Stdio server: {e}"
            )));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI argument validation
// 中文：该注释与英文“CLI argument validation”含义一致。
// ---------------------------------------------------------------------------

/// Validates CLI flag combinations across all three operating modes.
///     中文：该注释与英文“Validates CLI flag combinations across all three operating modes.”含义一致。
///
/// Enforces constraints such as:
///     中文：该注释与英文“Enforces constraints such as:”含义一致。
/// - Web and MCP ports must differ (except in stdio mode).
///     中文：列表项说明与英文“Web and MCP ports must differ (except in stdio mode).”含义一致。
/// - TUI-specific flags (--model, --temperature, --resume, etc.) are rejected
///     中文：列表项说明与英文“TUI-specific flags (--model, --temperature, --resume, etc.) are rejected”含义一致。
///   in web-only and stdio modes.
///     中文：该注释与英文“in web-only and stdio modes.”含义一致。
/// - Provider-specific flags are only accepted for their respective providers.
///     中文：列表项说明与英文“Provider-specific flags are only accepted for their respective providers.”含义一致。
fn validate_mode_args(args: &CodeArgs, _output: &OutputConfig) -> Result<(), String> {
    if !args.stdio && args.port == args.mcp_port && args.port != 0 {
        return Err(format!(
            "--port ({}) and --mcp-port ({}) must be different",
            args.port, args.mcp_port
        ));
    }

    // OC-Phase 6 P6.5: validate `--goal "<objective>"` against the
    // 中文：该注释与英文“OC-Phase 6 P6.5: validate `--goal "<objective>"` against the”含义一致。
    // same shape rules `GoalSpec::new` enforces (opencode.md
    // 中文：该注释与英文“same shape rules `GoalSpec::new` enforces (opencode.md”含义一致。
    // lines 538-556). Surfacing the failure at CLI parse keeps the
    // 中文：该注释与英文“lines 538-556). Surfacing the failure at CLI parse keeps the”含义一致。
    // supervisor (P6.3) from booting against a malformed objective
    // 中文：该注释与英文“supervisor (P6.3) from booting against a malformed objective”含义一致。
    // and gives the user a precise error string instead of a panic
    // 中文：该注释与英文“and gives the user a precise error string instead of a panic”含义一致。
    // at session-start.
    // 中文：该注释与英文“at session-start.”含义一致。
    if let Some(objective) = args.goal.as_deref() {
        use crate::internal::ai::goal::MAX_OBJECTIVE_LEN;
        if objective.trim().is_empty() {
            return Err("--goal requires a non-empty objective string (e.g. \
                 `--goal \"ship feature X\"`)"
                .to_string());
        }
        if objective.len() > MAX_OBJECTIVE_LEN {
            return Err(format!(
                "--goal objective is {} bytes which exceeds the {}-byte cap; \
                 shorten the objective and add detail through the model's \
                 first turn or `/goal criteria add <text>`",
                objective.len(),
                MAX_OBJECTIVE_LEN,
            ));
        }
    }

    if args.web_only {
        reject_non_tui_flags(args, "--web")?;
    }

    if args.stdio {
        if args.control == ControlMode::Write {
            return Err(
                "--control write is not supported with `libra code --stdio` because --stdio is the MCP stdio transport; use `libra code-control --stdio` for local TUI automation"
                    .to_string(),
            );
        }
        reject_non_tui_flags(args, "--stdio")?;
        reject_mode_flag(args.host != DEFAULT_BIND_HOST, "--host", "--stdio")?;
        reject_mode_flag(args.port != DEFAULT_WEB_PORT, "--port", "--stdio")?;
        reject_mode_flag(args.mcp_port != DEFAULT_MCP_PORT, "--mcp-port", "--stdio")?;
    }

    if args.control == ControlMode::Write {
        ensure_loopback_control_host_for_validation(&args.host)?;
    }

    if args.provider != CodeProvider::Codex {
        if args.codex_port.is_some() {
            return Err("--codex-port is only supported with --provider=codex".to_string());
        }
        if args.codex_bin != DEFAULT_CODEX_BIN {
            return Err("--codex-bin is only supported with --provider=codex".to_string());
        }
        if matches!(args.plan_mode, Some(true)) {
            return Err("--plan-mode is only supported with --provider=codex".to_string());
        }
    }

    if args.provider == CodeProvider::Codex && args.api_base.is_some() {
        return Err("--api-base is not supported with --provider=codex".to_string());
    }
    if let Some(base_url) = args.api_base.as_deref() {
        match Url::parse(base_url) {
            Ok(u) if u.scheme() == "http" || u.scheme() == "https" => {}
            Ok(u) => {
                return Err(format!(
                    "--api-base must use http or https (got {})",
                    u.scheme()
                ));
            }
            Err(e) => {
                return Err(format!("--api-base is not a valid URL: {e}"));
            }
        }
    }

    if args.provider != CodeProvider::Ollama && args.ollama_thinking.is_some() {
        return Err(
            "--ollama-thinking/--thinking is only supported with --provider=ollama".to_string(),
        );
    }

    if args.provider != CodeProvider::Ollama && args.ollama_compact_tools {
        return Err("--ollama-compact-tools is only supported with --provider=ollama".to_string());
    }

    if args.provider != CodeProvider::Deepseek && args.deepseek_thinking.is_some() {
        return Err("--deepseek-thinking is only supported with --provider=deepseek".to_string());
    }

    if args.provider != CodeProvider::Deepseek && args.deepseek_reasoning_effort.is_some() {
        return Err(
            "--deepseek-reasoning-effort is only supported with --provider=deepseek".to_string(),
        );
    }

    if args.provider != CodeProvider::Deepseek && args.deepseek_stream.is_some() {
        return Err(
            "--deepseek-stream/--stream is only supported with --provider=deepseek".to_string(),
        );
    }

    if args.provider != CodeProvider::Kimi && args.kimi_thinking.is_some() {
        return Err("--kimi-thinking is only supported with --provider=kimi".to_string());
    }

    if args.provider != CodeProvider::Kimi && args.kimi_stream.is_some() {
        return Err("--kimi-stream is only supported with --provider=kimi".to_string());
    }

    #[cfg(feature = "test-provider")]
    {
        if args.provider == CodeProvider::Fake {
            if std::env::var_os("LIBRA_ENABLE_TEST_PROVIDER").is_none() {
                return Err(
                    "--provider=fake is test-only; set LIBRA_ENABLE_TEST_PROVIDER=1 to use it"
                        .to_string(),
                );
            }
            if args.fake_fixture.is_none() {
                return Err("--fake-fixture is required with --provider=fake".to_string());
            }
        } else if args.fake_fixture.is_some() {
            return Err("--fake-fixture is only supported with --provider=fake".to_string());
        }
    }

    Ok(())
}

/// Helper: rejects a flag if it was set (`is_invalid == true`) with a
///     中文：该注释与英文“Helper: rejects a flag if it was set (`is_invalid == true`) with a”含义一致。
/// standardized error message indicating the flag is not supported in the given mode.
///     中文：该注释与英文“standardized error message indicating the flag is not supported in the given mode.”含义一致。
fn reject_mode_flag(is_invalid: bool, flag: &str, mode: &str) -> Result<(), String> {
    if is_invalid {
        return Err(format!("{flag} is not supported in {mode} mode"));
    }
    Ok(())
}

fn ensure_loopback_control_host_for_validation(host: &str) -> Result<(), String> {
    let normalized = host.trim().trim_matches('[').trim_matches(']');
    let is_loopback = matches!(normalized, "localhost" | "127.0.0.1" | "::1")
        || normalized
            .parse::<std::net::IpAddr>()
            .map(|addr| addr.is_loopback())
            .unwrap_or(false);

    if is_loopback {
        Ok(())
    } else {
        Err("--control write requires a loopback --host such as 127.0.0.1 or ::1".to_string())
    }
}

/// Rejects all TUI-specific flags when running in a non-TUI mode (web-only or stdio).
///     中文：该注释与英文“Rejects all TUI-specific flags when running in a non-TUI mode (web-only or stdio).”含义一致。
/// This ensures users get clear errors instead of silently ignored flags.
///     中文：该注释与英文“This ensures users get clear errors instead of silently ignored flags.”含义一致。
fn reject_non_tui_flags(args: &CodeArgs, mode: &str) -> Result<(), String> {
    reject_mode_flag(args.provider != CodeProvider::Gemini, "--provider", mode)?;
    reject_mode_flag(args.model.is_some(), "--model", mode)?;
    reject_mode_flag(args.temperature.is_some(), "--temperature", mode)?;
    reject_mode_flag(args.env_file.is_some(), "--env-file", mode)?;
    reject_mode_flag(args.ollama_thinking.is_some(), "--ollama-thinking", mode)?;
    reject_mode_flag(args.ollama_compact_tools, "--ollama-compact-tools", mode)?;
    reject_mode_flag(
        args.deepseek_thinking.is_some(),
        "--deepseek-thinking",
        mode,
    )?;
    reject_mode_flag(
        args.deepseek_reasoning_effort.is_some(),
        "--deepseek-reasoning-effort",
        mode,
    )?;
    reject_mode_flag(args.deepseek_stream.is_some(), "--deepseek-stream", mode)?;
    reject_mode_flag(args.kimi_thinking.is_some(), "--kimi-thinking", mode)?;
    reject_mode_flag(args.kimi_stream.is_some(), "--kimi-stream", mode)?;
    reject_mode_flag(args.context.is_some(), "--context", mode)?;
    reject_mode_flag(args.resume.is_some(), "--resume", mode)?;
    reject_mode_flag(
        args.approval_policy != CodeApprovalPolicy::OnRequest,
        "--approval-policy",
        mode,
    )?;
    reject_mode_flag(args.approval_ttl.is_some(), "--approval-ttl", mode)?;
    reject_mode_flag(
        args.network_access != CodeNetworkAccess::Deny,
        "--network-access",
        mode,
    )?;
    reject_mode_flag(args.api_base.is_some(), "--api-base", mode)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// 中文：该注释与英文“Tests”含义一致。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};
    use tokio::{
        net::TcpListener,
        sync::{Mutex as AsyncMutex, mpsc::unbounded_channel},
    };

    use super::*;

    /// CEX-S2-12 "single sub-agent behind flag": the dispatcher
    ///     中文：该注释与英文“CEX-S2-12 "single sub-agent behind flag": the dispatcher”含义一致。
    /// concurrency cap is forced to 1 for every configured value —
    ///     中文：该注释与英文“concurrency cap is forced to 1 for every configured value —”含义一致。
    /// including the `sub_agents.max_parallel` schema default of 2 and
    ///     中文：该注释与英文“including the `sub_agents.max_parallel` schema default of 2 and”含义一致。
    /// larger operator settings — until CEX-S2-14 unlocks real
    ///     中文：该注释与英文“larger operator settings — until CEX-S2-14 unlocks real”含义一致。
    /// parallelism. Pins the cap against a silent regression to passing
    ///     中文：该注释与英文“parallelism. Pins the cap against a silent regression to passing”含义一致。
    /// the operator value through.
    ///     中文：该注释与英文“the operator value through.”含义一致。
    // Test scenario: verifies `s2_12_concurrency_cap_forces_single_sub_agent` covers the s2 12 concurrency cap forces single sub agent behavior.
    // 测试场景：验证 `s2_12_concurrency_cap_forces_single_sub_agent` 覆盖 s2 12 concurrency cap forces single sub agent 对应的行为。
    #[test]
    fn s2_12_concurrency_cap_forces_single_sub_agent() {
        for configured in [0_u32, 1, 2, 4, 16, u32::MAX] {
            assert_eq!(
                cex_s2_12_subagent_concurrency_cap(configured),
                1,
                "CEX-S2-12 must cap concurrency to 1, not {configured}",
            );
        }
    }

    fn base_args() -> CodeArgs {
        CodeArgs {
            web_only: false,
            port: DEFAULT_WEB_PORT,
            host: DEFAULT_BIND_HOST.to_string(),
            cwd: None,
            repo: None,
            env_file: None,
            control: ControlMode::Observe,
            browser_control: None,
            control_token_file: None,
            control_info_file: None,
            provider: CodeProvider::Gemini,
            model: None,
            temperature: None,
            ollama_thinking: None,
            ollama_compact_tools: false,
            deepseek_thinking: None,
            deepseek_reasoning_effort: None,
            deepseek_stream: None,
            kimi_thinking: None,
            kimi_stream: None,
            agent: None,
            #[cfg(feature = "test-provider")]
            fake_fixture: None,
            context: None,
            resume: None,
            approval_policy: CodeApprovalPolicy::OnRequest,
            approval_ttl: None,
            network_access: CodeNetworkAccess::Deny,
            mcp_port: DEFAULT_MCP_PORT,
            stdio: false,
            api_base: None,
            codex_bin: DEFAULT_CODEX_BIN.to_string(),
            codex_port: None,
            plan_mode: None,
            goal: None,
        }
    }

    fn canned_openai_compat_response() -> Value {
        json!({
            "id": "test-completion",
            "object": "chat.completion",
            "created": 0,
            "model": "test-model",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "ok"
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 1,
                "total_tokens": 2
            }
        })
    }

    async fn start_chat_completions_stub() -> (
        String,
        Arc<AsyncMutex<Vec<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let captured = Arc::new(AsyncMutex::new(Vec::new()));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let captured = captured.clone();
                move |Json(body): Json<Value>| {
                    let captured = captured.clone();
                    async move {
                        captured.lock().await.push(body);
                        Json(canned_openai_compat_response())
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock provider listener");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock provider server runs");
        });
        (base_url, captured, handle)
    }

    // Test scenario: verifies `rejects_same_web_and_mcp_ports` covers the rejects same web and mcp ports behavior.
    // 测试场景：验证 `rejects_same_web_and_mcp_ports` 覆盖 rejects same web and mcp ports 对应的行为。
    #[test]
    fn rejects_same_web_and_mcp_ports() {
        let mut args = base_args();
        args.mcp_port = args.port;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    /// OC-Phase 6 P6.5: `--goal` runs the same shape rules
    ///     中文：该注释与英文“OC-Phase 6 P6.5: `--goal` runs the same shape rules”含义一致。
    /// `GoalSpec::new` does so a malformed objective fails CLI
    ///     中文：该注释与英文“`GoalSpec::new` does so a malformed objective fails CLI”含义一致。
    /// parsing instead of crashing the supervisor at session start.
    ///     中文：该注释与英文“parsing instead of crashing the supervisor at session start.”含义一致。
    // Test scenario: verifies `accepts_well_formed_goal_objective` covers the accepts well formed goal objective behavior.
    // 测试场景：验证 `accepts_well_formed_goal_objective` 覆盖 accepts well formed goal objective 对应的行为。
    #[test]
    fn accepts_well_formed_goal_objective() {
        let mut args = base_args();
        args.goal = Some("ship feature X".to_string());
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_blank_goal_objective() {
        let mut args = base_args();
        args.goal = Some("   ".to_string());
        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("non-empty objective"));
    }

    // Test scenario: verifies `rejects_oversized_goal_objective` covers the rejects oversized goal objective behavior.
    // 测试场景：验证 `rejects_oversized_goal_objective` 覆盖 rejects oversized goal objective 对应的行为。
    #[test]
    fn rejects_oversized_goal_objective() {
        use crate::internal::ai::goal::MAX_OBJECTIVE_LEN;
        let mut args = base_args();
        args.goal = Some("z".repeat(MAX_OBJECTIVE_LEN + 1));
        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("exceeds the"));
    }

    // Test scenario: verifies `rejects_tui_flags_in_web_mode` covers the rejects tui flags in web mode behavior.
    // 测试场景：验证 `rejects_tui_flags_in_web_mode` 覆盖 rejects tui flags in web mode 对应的行为。
    #[test]
    fn rejects_tui_flags_in_web_mode() {
        let mut args = base_args();
        args.web_only = true;
        args.model = Some("foo".to_string());
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    // Test scenario: verifies `rejects_web_flags_in_stdio_mode` covers the rejects web flags in stdio mode behavior.
    // 测试场景：验证 `rejects_web_flags_in_stdio_mode` 覆盖 rejects web flags in stdio mode 对应的行为。
    #[test]
    fn rejects_web_flags_in_stdio_mode() {
        let mut args = base_args();
        args.stdio = true;
        args.host = "0.0.0.0".to_string();
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    // Test scenario: verifies `accepts_default_tui_mode` covers the accepts default tui mode behavior.
    // 测试场景：验证 `accepts_default_tui_mode` 覆盖 accepts default tui mode 对应的行为。
    #[test]
    fn accepts_default_tui_mode() {
        let args = base_args();
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn accepts_control_write_in_default_tui_mode() {
        let mut args = base_args();
        args.control = ControlMode::Write;

        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `accepts_control_write_in_default_web_mode` covers the accepts control write in default web mode behavior.
    // 测试场景：验证 `accepts_control_write_in_default_web_mode` 覆盖 accepts control write in default web mode 对应的行为。
    #[test]
    fn accepts_control_write_in_default_web_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--web", "--control", "write"]).unwrap();

        assert!(args.web_only);
        assert_eq!(args.control, ControlMode::Write);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `browser_control_resolution_matrix_pins_mode_provider_and_host_contract` covers the browser control resolution matrix pins mode provider and host contract behavior.
    // 测试场景：验证 `browser_control_resolution_matrix_pins_mode_provider_and_host_contract` 覆盖 browser control resolution matrix pins mode provider and host contract 对应的行为。
    #[test]
    fn browser_control_resolution_matrix_pins_mode_provider_and_host_contract() {
        #[derive(Copy, Clone)]
        struct BrowserControlCase {
            name: &'static str,
            web_only: bool,
            provider: CodeProvider,
            explicit: Option<BrowserControlMode>,
            host: &'static str,
            expected: Result<BrowserControlMode, &'static str>,
        }

        let cases = [
            BrowserControlCase {
                name: "tui default stays off even on non-loopback host",
                web_only: false,
                provider: CodeProvider::Gemini,
                explicit: None,
                host: "0.0.0.0",
                expected: Ok(BrowserControlMode::Off),
            },
            BrowserControlCase {
                name: "tui explicit off allows non-loopback host",
                web_only: false,
                provider: CodeProvider::Gemini,
                explicit: Some(BrowserControlMode::Off),
                host: "0.0.0.0",
                expected: Ok(BrowserControlMode::Off),
            },
            BrowserControlCase {
                name: "tui explicit loopback allows loopback host",
                web_only: false,
                provider: CodeProvider::Gemini,
                explicit: Some(BrowserControlMode::Loopback),
                host: "127.0.0.1",
                expected: Ok(BrowserControlMode::Loopback),
            },
            BrowserControlCase {
                name: "tui explicit loopback rejects non-loopback host",
                web_only: false,
                provider: CodeProvider::Gemini,
                explicit: Some(BrowserControlMode::Loopback),
                host: "0.0.0.0",
                expected: Err("loopback"),
            },
            BrowserControlCase {
                name: "non-codex web-only default stays off on non-loopback host",
                web_only: true,
                provider: CodeProvider::Ollama,
                explicit: None,
                host: "0.0.0.0",
                expected: Ok(BrowserControlMode::Off),
            },
            BrowserControlCase {
                name: "non-codex web-only explicit loopback rejects non-loopback host",
                web_only: true,
                provider: CodeProvider::Ollama,
                explicit: Some(BrowserControlMode::Loopback),
                host: "0.0.0.0",
                expected: Err("loopback"),
            },
            BrowserControlCase {
                name: "codex web-only defaults to loopback on loopback host",
                web_only: true,
                provider: CodeProvider::Codex,
                explicit: None,
                host: "localhost",
                expected: Ok(BrowserControlMode::Loopback),
            },
            BrowserControlCase {
                name: "codex web-only default loopback rejects non-loopback host",
                web_only: true,
                provider: CodeProvider::Codex,
                explicit: None,
                host: "0.0.0.0",
                expected: Err("loopback"),
            },
            BrowserControlCase {
                name: "codex web-only explicit off allows non-loopback host",
                web_only: true,
                provider: CodeProvider::Codex,
                explicit: Some(BrowserControlMode::Off),
                host: "0.0.0.0",
                expected: Ok(BrowserControlMode::Off),
            },
            BrowserControlCase {
                name: "codex web-only explicit loopback allows ipv6 loopback host",
                web_only: true,
                provider: CodeProvider::Codex,
                explicit: Some(BrowserControlMode::Loopback),
                host: "::1",
                expected: Ok(BrowserControlMode::Loopback),
            },
        ];

        for case in cases {
            let mut args = base_args();
            args.web_only = case.web_only;
            args.provider = case.provider;
            args.browser_control = case.explicit;
            args.host = case.host.to_string();

            match (resolve_browser_control_mode(&args), case.expected) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected, "case: {}", case.name);
                }
                (Err(error), Err(expected_text)) => {
                    let rendered = error.to_string();
                    assert!(
                        rendered.contains(expected_text),
                        "case: {}; expected error containing {expected_text:?}, got {rendered}",
                        case.name
                    );
                }
                (actual, expected) => {
                    panic!(
                        "case: {}; browser-control resolution mismatch; actual={actual:?}, expected={expected:?}",
                        case.name
                    );
                }
            }
        }
    }

    // Test scenario: verifies `rejects_control_write_in_stdio_mode` covers the rejects control write in stdio mode behavior.
    // 测试场景：验证 `rejects_control_write_in_stdio_mode` 覆盖 rejects control write in stdio mode 对应的行为。
    #[test]
    fn rejects_control_write_in_stdio_mode() {
        let mut args = base_args();
        args.stdio = true;
        args.control = ControlMode::Write;

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("code-control --stdio"));
    }

    // Test scenario: verifies `rejects_control_write_with_non_loopback_host` covers the rejects control write with non loopback host behavior.
    // 测试场景：验证 `rejects_control_write_with_non_loopback_host` 覆盖 rejects control write with non loopback host 对应的行为。
    #[test]
    fn rejects_control_write_with_non_loopback_host() {
        let mut args = base_args();
        args.control = ControlMode::Write;
        args.host = "0.0.0.0".to_string();

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("loopback"));
    }

    // Test scenario: verifies `accepts_env_file_cli_arg_in_tui_mode` covers the accepts env file cli arg in tui mode behavior.
    // 测试场景：验证 `accepts_env_file_cli_arg_in_tui_mode` 覆盖 accepts env file cli arg in tui mode 对应的行为。
    #[test]
    fn accepts_env_file_cli_arg_in_tui_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--env-file", ".env.test"]).unwrap();

        assert_eq!(args.env_file.as_deref(), Some(Path::new(".env.test")));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `rejects_env_file_in_web_mode` covers the rejects env file in web mode behavior.
    // 测试场景：验证 `rejects_env_file_in_web_mode` 覆盖 rejects env file in web mode 对应的行为。
    #[test]
    fn rejects_env_file_in_web_mode() {
        let mut args = base_args();
        args.web_only = true;
        args.env_file = Some(PathBuf::from(".env.test"));

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--env-file"));
    }

    // Test scenario: verifies `parses_dotenv_style_env_file` covers the parses dotenv style env file behavior.
    // 测试场景：验证 `parses_dotenv_style_env_file` 覆盖 parses dotenv style env file 对应的行为。
    #[test]
    fn parses_dotenv_style_env_file() {
        let env_file = parse_code_env_file(
            r#"
            # comments and blank lines are ignored
            export DEEPSEEK_API_KEY='deepseek-key'
            OPENAI_BASE_URL="https://example.test/v1"
            UNQUOTED=value # inline comment
            "#,
            Path::new(".env.test"),
        )
        .unwrap();

        assert_eq!(env_file.get("DEEPSEEK_API_KEY"), Some("deepseek-key"));
        assert_eq!(
            env_file.get("OPENAI_BASE_URL"),
            Some("https://example.test/v1")
        );
        assert_eq!(env_file.get("UNQUOTED"), Some("value"));
    }

    // Test scenario: verifies `provider_env_file_value_overrides_process_lookup` covers the provider env file value overrides process lookup behavior.
    // 测试场景：验证 `provider_env_file_value_overrides_process_lookup` 覆盖 provider env file value overrides process lookup 对应的行为。
    #[test]
    fn provider_env_file_value_overrides_process_lookup() {
        let env_file =
            parse_code_env_file("DEEPSEEK_API_KEY=file-key", Path::new(".env.test")).unwrap();

        let value = provider_env_value_with_lookup(&env_file, "DEEPSEEK_API_KEY", |_| {
            Some("old-key".into())
        });

        assert_eq!(value.as_deref(), Some("file-key"));
    }

    // Test scenario: verifies `accepts_network_access_cli_arg_in_tui_mode` covers the accepts network access cli arg in tui mode behavior.
    // 测试场景：验证 `accepts_network_access_cli_arg_in_tui_mode` 覆盖 accepts network access cli arg in tui mode 对应的行为。
    #[test]
    fn accepts_network_access_cli_arg_in_tui_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--network-access", "allow"]).unwrap();

        assert_eq!(args.network_access, CodeNetworkAccess::Allow);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `accepts_allow_all_approval_policy_in_tui_mode` covers the accepts allow all approval policy in tui mode behavior.
    // 测试场景：验证 `accepts_allow_all_approval_policy_in_tui_mode` 覆盖 accepts allow all approval policy in tui mode 对应的行为。
    #[test]
    fn accepts_allow_all_approval_policy_in_tui_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--approval-policy", "allow-all"]).unwrap();

        assert_eq!(args.approval_policy, CodeApprovalPolicy::AllowAll);
        assert!(args.approval_policy.allows_all_commands());
        assert_eq!(
            AskForApproval::from(args.approval_policy),
            AskForApproval::OnRequest
        );
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `accepts_approval_ttl_cli_arg_in_tui_mode` covers the accepts approval ttl cli arg in tui mode behavior.
    // 测试场景：验证 `accepts_approval_ttl_cli_arg_in_tui_mode` 覆盖 accepts approval ttl cli arg in tui mode 对应的行为。
    #[test]
    fn accepts_approval_ttl_cli_arg_in_tui_mode() {
        let args = CodeArgs::try_parse_from(["libra", "--approval-ttl", "42"]).unwrap();

        assert_eq!(args.approval_ttl, Some(42));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `loads_approval_ttl_from_project_config` covers the loads approval ttl from project config behavior.
    // 测试场景：验证 `loads_approval_ttl_from_project_config` 覆盖 loads approval ttl from project config 对应的行为。
    #[test]
    fn loads_approval_ttl_from_project_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let libra_dir = temp_dir.path().join(".libra");
        fs::create_dir_all(&libra_dir).unwrap();
        fs::write(
            libra_dir.join("config.toml"),
            "[approval]\nttl_seconds = 123\n",
        )
        .unwrap();

        assert_eq!(
            approval_ttl_from_project_config(temp_dir.path()),
            Some(Duration::from_secs(123))
        );
    }

    // Test scenario: verifies `loads_approval_cache_policy_from_project_config` covers the loads approval cache policy from project config behavior.
    // 测试场景：验证 `loads_approval_cache_policy_from_project_config` 覆盖 loads approval cache policy from project config 对应的行为。
    #[test]
    fn loads_approval_cache_policy_from_project_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let libra_dir = temp_dir.path().join(".libra");
        fs::create_dir_all(&libra_dir).unwrap();
        fs::write(
            libra_dir.join("config.toml"),
            r#"[approval]
protected_branches = ["main", "release"]
allowed_network_domains = ["github.com"]
no_cache_unknown_network = true
"#,
        )
        .unwrap();

        assert_eq!(
            approval_cache_policy_from_project_config(temp_dir.path()),
            ApprovalCachePolicy {
                protected_branches: vec!["main".to_string(), "release".to_string()],
                allowed_network_domains: vec!["github.com".to_string()],
                no_cache_unknown_network: true,
                approved_ruleset: None,
            }
        );
    }

    // Test scenario: verifies `plan_mode_defaults_to_none_when_omitted` covers the plan mode defaults to none when omitted behavior.
    // 测试场景：验证 `plan_mode_defaults_to_none_when_omitted` 覆盖 plan mode defaults to none when omitted 对应的行为。
    #[test]
    fn plan_mode_defaults_to_none_when_omitted() {
        let args = CodeArgs::try_parse_from(["libra"]).unwrap();
        assert_eq!(args.plan_mode, None);
    }

    #[test]
    fn plan_mode_bare_flag_is_true() {
        let args = CodeArgs::try_parse_from(["libra", "--plan-mode"]).unwrap();
        assert_eq!(args.plan_mode, Some(true));
    }

    // Test scenario: verifies `plan_mode_explicit_true_is_true` covers the plan mode explicit true is true behavior.
    // 测试场景：验证 `plan_mode_explicit_true_is_true` 覆盖 plan mode explicit true is true 对应的行为。
    #[test]
    fn plan_mode_explicit_true_is_true() {
        let args = CodeArgs::try_parse_from(["libra", "--plan-mode=true"]).unwrap();
        assert_eq!(args.plan_mode, Some(true));
    }

    #[test]
    fn plan_mode_explicit_false_is_false() {
        let args = CodeArgs::try_parse_from(["libra", "--plan-mode=false"]).unwrap();
        assert_eq!(args.plan_mode, Some(false));
    }

    // Test scenario: verifies `effective_plan_mode_defaults_to_true_for_codex` covers the effective plan mode defaults to true for codex behavior.
    // 测试场景：验证 `effective_plan_mode_defaults_to_true_for_codex` 覆盖 effective plan mode defaults to true for codex 对应的行为。
    #[test]
    fn effective_plan_mode_defaults_to_true_for_codex() {
        let mut args = base_args();
        args.provider = CodeProvider::Codex;
        assert!(effective_plan_mode(&args));
    }

    #[test]
    fn effective_plan_mode_defaults_to_false_for_non_codex_providers() {
        let providers = [
            CodeProvider::Gemini,
            CodeProvider::Openai,
            CodeProvider::Anthropic,
            CodeProvider::Deepseek,
            CodeProvider::Kimi,
            CodeProvider::Zhipu,
            CodeProvider::Ollama,
        ];
        for provider in providers {
            let mut args = base_args();
            args.provider = provider;
            assert!(
                !effective_plan_mode(&args),
                "expected plan_mode=false default for provider {provider:?}"
            );
        }
    }

    // Test scenario: verifies `effective_plan_mode_respects_explicit_user_value` covers the effective plan mode respects explicit user value behavior.
    // 测试场景：验证 `effective_plan_mode_respects_explicit_user_value` 覆盖 effective plan mode respects explicit user value 对应的行为。
    #[test]
    fn effective_plan_mode_respects_explicit_user_value() {
        let mut args = base_args();
        args.provider = CodeProvider::Codex;
        args.plan_mode = Some(false);
        assert!(
            !effective_plan_mode(&args),
            "explicit --plan-mode=false must override the codex default"
        );

        args.provider = CodeProvider::Gemini;
        args.plan_mode = Some(true);
        assert!(
            effective_plan_mode(&args),
            "explicit --plan-mode=true must take effect even for non-codex providers \
             at the resolution layer (validate_mode_args is responsible for rejecting \
             that combination separately)"
        );
    }

    // Test scenario: verifies `rejects_explicit_plan_mode_true_for_non_codex_provider` covers the rejects explicit plan mode true for non codex provider behavior.
    // 测试场景：验证 `rejects_explicit_plan_mode_true_for_non_codex_provider` 覆盖 rejects explicit plan mode true for non codex provider 对应的行为。
    #[test]
    fn rejects_explicit_plan_mode_true_for_non_codex_provider() {
        let mut args = base_args();
        args.provider = CodeProvider::Gemini;
        args.plan_mode = Some(true);
        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--plan-mode"));
    }

    // Test scenario: verifies `accepts_explicit_plan_mode_false_for_non_codex_provider` covers the accepts explicit plan mode false for non codex provider behavior.
    // 测试场景：验证 `accepts_explicit_plan_mode_false_for_non_codex_provider` 覆盖 accepts explicit plan mode false for non codex provider 对应的行为。
    #[test]
    fn accepts_explicit_plan_mode_false_for_non_codex_provider() {
        let mut args = base_args();
        args.provider = CodeProvider::Gemini;
        args.plan_mode = Some(false);
        validate_mode_args(&args, &OutputConfig::default()).unwrap();
    }

    // Test scenario: verifies `rejects_network_access_cli_arg_with_invalid_value` covers the rejects network access cli arg with invalid value behavior.
    // 测试场景：验证 `rejects_network_access_cli_arg_with_invalid_value` 覆盖 rejects network access cli arg with invalid value 对应的行为。
    #[test]
    fn rejects_network_access_cli_arg_with_invalid_value() {
        let result = CodeArgs::try_parse_from(["libra", "--network-access", "sometimes"]);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_network_access_flag_in_web_mode() {
        let mut args = base_args();
        args.web_only = true;
        args.network_access = CodeNetworkAccess::Allow;

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--network-access"));
    }

    // Test scenario: verifies `accepts_anthropic_provider_in_tui_mode` covers the accepts anthropic provider in tui mode behavior.
    // 测试场景：验证 `accepts_anthropic_provider_in_tui_mode` 覆盖 accepts anthropic provider in tui mode 对应的行为。
    #[test]
    fn accepts_anthropic_provider_in_tui_mode() {
        let mut args = base_args();
        args.provider = CodeProvider::Anthropic;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    #[test]
    fn rejects_ollama_thinking_for_non_ollama_provider() {
        let mut args = base_args();
        args.ollama_thinking = Some(OllamaThinkingArg::High);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    // Test scenario: verifies `accepts_ollama_thinking_for_ollama_provider` covers the accepts ollama thinking for ollama provider behavior.
    // 测试场景：验证 `accepts_ollama_thinking_for_ollama_provider` 覆盖 accepts ollama thinking for ollama provider 对应的行为。
    #[test]
    fn accepts_ollama_thinking_for_ollama_provider() {
        let mut args = base_args();
        args.provider = CodeProvider::Ollama;
        args.ollama_thinking = Some(OllamaThinkingArg::High);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `rejects_ollama_compact_tools_for_non_ollama_provider` covers the rejects ollama compact tools for non ollama provider behavior.
    // 测试场景：验证 `rejects_ollama_compact_tools_for_non_ollama_provider` 覆盖 rejects ollama compact tools for non ollama provider 对应的行为。
    #[test]
    fn rejects_ollama_compact_tools_for_non_ollama_provider() {
        let mut args = base_args();
        args.ollama_compact_tools = true;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    #[test]
    fn accepts_ollama_compact_tools_for_ollama_provider() {
        let mut args = base_args();
        args.provider = CodeProvider::Ollama;
        args.ollama_compact_tools = true;
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `accepts_deepseek_reasoning_flags_for_deepseek_provider` covers the accepts deepseek reasoning flags for deepseek provider behavior.
    // 测试场景：验证 `accepts_deepseek_reasoning_flags_for_deepseek_provider` 覆盖 accepts deepseek reasoning flags for deepseek provider 对应的行为。
    #[test]
    fn accepts_deepseek_reasoning_flags_for_deepseek_provider() {
        let args = CodeArgs::try_parse_from([
            "libra",
            "--provider",
            "deepseek",
            "--model",
            "deepseek-v4-pro",
            "--deepseek-thinking",
            "enabled",
            "--deepseek-reasoning-effort",
            "high",
            "--deepseek-stream",
            "true",
        ])
        .unwrap();

        assert_eq!(args.provider, CodeProvider::Deepseek);
        assert_eq!(args.deepseek_thinking, Some(DeepSeekThinkingArg::Enabled));
        assert_eq!(
            args.deepseek_reasoning_effort,
            Some(DeepSeekReasoningEffortArg::High)
        );
        assert_eq!(
            completion_thinking_for_args(&args),
            Some(CompletionThinking::Enabled)
        );
        assert_eq!(
            completion_reasoning_effort_for_args(&args),
            Some(CompletionReasoningEffort::High)
        );
        assert_eq!(completion_stream_for_args(&args), Some(true));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `accepts_deepseek_max_reasoning_alias` covers the accepts deepseek max reasoning alias behavior.
    // 测试场景：验证 `accepts_deepseek_max_reasoning_alias` 覆盖 accepts deepseek max reasoning alias 对应的行为。
    #[test]
    fn accepts_deepseek_max_reasoning_alias() {
        let args = CodeArgs::try_parse_from([
            "libra",
            "--provider",
            "deepseek",
            "--deepseek-reasoning-effort",
            "xhigh",
        ])
        .unwrap();

        assert_eq!(
            args.deepseek_reasoning_effort,
            Some(DeepSeekReasoningEffortArg::Max)
        );
        assert_eq!(
            completion_reasoning_effort_for_args(&args),
            Some(CompletionReasoningEffort::Max)
        );
    }

    // Test scenario: verifies `rejects_deepseek_reasoning_flags_for_non_deepseek_provider` covers the rejects deepseek reasoning flags for non deepseek provider behavior.
    // 测试场景：验证 `rejects_deepseek_reasoning_flags_for_non_deepseek_provider` 覆盖 rejects deepseek reasoning flags for non deepseek provider 对应的行为。
    #[test]
    fn rejects_deepseek_reasoning_flags_for_non_deepseek_provider() {
        let mut args = base_args();
        args.deepseek_thinking = Some(DeepSeekThinkingArg::Enabled);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());

        let mut args = base_args();
        args.deepseek_reasoning_effort = Some(DeepSeekReasoningEffortArg::High);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());

        let mut args = base_args();
        args.deepseek_stream = Some(true);
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_err());
    }

    // Test scenario: verifies `accepts_kimi_thinking_for_kimi_provider` covers the accepts kimi thinking for kimi provider behavior.
    // 测试场景：验证 `accepts_kimi_thinking_for_kimi_provider` 覆盖 accepts kimi thinking for kimi provider 对应的行为。
    #[test]
    fn accepts_kimi_thinking_for_kimi_provider() {
        let args = CodeArgs::try_parse_from([
            "libra",
            "--provider",
            "kimi",
            "--model",
            "kimi-k2.6",
            "--kimi-thinking",
            "disabled",
        ])
        .unwrap();

        assert_eq!(args.provider, CodeProvider::Kimi);
        assert_eq!(args.kimi_thinking, Some(KimiThinkingArg::Disabled));
        assert_eq!(
            completion_thinking_for_args(&args),
            Some(CompletionThinking::Disabled)
        );
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `defaults_kimi_stream_for_kimi_provider` covers the defaults kimi stream for kimi provider behavior.
    // 测试场景：验证 `defaults_kimi_stream_for_kimi_provider` 覆盖 defaults kimi stream for kimi provider 对应的行为。
    #[test]
    fn defaults_kimi_stream_for_kimi_provider() {
        let args = CodeArgs::try_parse_from(["libra", "--provider", "kimi"]).unwrap();

        assert_eq!(args.provider, CodeProvider::Kimi);
        assert_eq!(args.kimi_stream, None);
        assert_eq!(completion_stream_for_args(&args), Some(true));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `accepts_kimi_stream_override_for_kimi_provider` covers the accepts kimi stream override for kimi provider behavior.
    // 测试场景：验证 `accepts_kimi_stream_override_for_kimi_provider` 覆盖 accepts kimi stream override for kimi provider 对应的行为。
    #[test]
    fn accepts_kimi_stream_override_for_kimi_provider() {
        let args =
            CodeArgs::try_parse_from(["libra", "--provider", "kimi", "--kimi-stream", "false"])
                .unwrap();

        assert_eq!(args.provider, CodeProvider::Kimi);
        assert_eq!(args.kimi_stream, Some(false));
        assert_eq!(completion_stream_for_args(&args), Some(false));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `rejects_kimi_thinking_for_non_kimi_provider` covers the rejects kimi thinking for non kimi provider behavior.
    // 测试场景：验证 `rejects_kimi_thinking_for_non_kimi_provider` 覆盖 rejects kimi thinking for non kimi provider 对应的行为。
    #[test]
    fn rejects_kimi_thinking_for_non_kimi_provider() {
        let mut args = base_args();
        args.kimi_thinking = Some(KimiThinkingArg::Enabled);

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--kimi-thinking"));
    }

    // Test scenario: verifies `rejects_kimi_stream_for_non_kimi_provider` covers the rejects kimi stream for non kimi provider behavior.
    // 测试场景：验证 `rejects_kimi_stream_for_non_kimi_provider` 覆盖 rejects kimi stream for non kimi provider 对应的行为。
    #[test]
    fn rejects_kimi_stream_for_non_kimi_provider() {
        let mut args = base_args();
        args.kimi_stream = Some(true);

        let err = validate_mode_args(&args, &OutputConfig::default()).unwrap_err();
        assert!(err.contains("--kimi-stream"));
    }

    // Test scenario: verifies `accepts_deepseek_stream_alias_for_deepseek_provider` covers the accepts deepseek stream alias for deepseek provider behavior.
    // 测试场景：验证 `accepts_deepseek_stream_alias_for_deepseek_provider` 覆盖 accepts deepseek stream alias for deepseek provider 对应的行为。
    #[test]
    fn accepts_deepseek_stream_alias_for_deepseek_provider() {
        let args =
            CodeArgs::try_parse_from(["libra", "--provider", "deepseek", "--stream", "false"])
                .unwrap();

        assert_eq!(args.deepseek_stream, Some(false));
        assert_eq!(completion_stream_for_args(&args), Some(false));
        assert!(validate_mode_args(&args, &OutputConfig::default()).is_ok());
    }

    // Test scenario: verifies `tui_preserves_reasoning_content_for_reasoning_providers` covers the tui preserves reasoning content for reasoning providers behavior.
    // 测试场景：验证 `tui_preserves_reasoning_content_for_reasoning_providers` 覆盖 tui preserves reasoning content for reasoning providers 对应的行为。
    #[test]
    fn tui_preserves_reasoning_content_for_reasoning_providers() {
        assert!(preserve_reasoning_content_for_provider(
            CodeProvider::Deepseek
        ));
        assert!(!preserve_reasoning_content_for_provider(
            CodeProvider::Gemini
        ));
        assert!(!preserve_reasoning_content_for_provider(
            CodeProvider::Ollama
        ));
        assert!(preserve_reasoning_content_for_provider(CodeProvider::Kimi));
    }

    // Test scenario: verifies `codex_preflight_rejects_file_cwd` covers the codex preflight rejects file cwd behavior.
    // 测试场景：验证 `codex_preflight_rejects_file_cwd` 覆盖 codex preflight rejects file cwd 对应的行为。
    #[test]
    fn codex_preflight_rejects_file_cwd() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cwd_file = temp_dir.path().join("README.md");
        std::fs::write(&cwd_file, "not a directory").unwrap();

        let mut args = base_args();
        args.provider = CodeProvider::Codex;
        args.cwd = Some(cwd_file.clone());

        let err = resolve_code_preflight_working_dir(&args).unwrap_err();
        assert!(
            err.to_string().contains("--cwd must point to a directory"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains(&cwd_file.display().to_string()),
            "error should identify the invalid --cwd path: {err}"
        );
    }

    // Test scenario: verifies `code_ui_runtime_uses_canonical_thread_id_metadata` covers the code ui runtime uses canonical thread id metadata behavior.
    // 测试场景：验证 `code_ui_runtime_uses_canonical_thread_id_metadata` 覆盖 code ui runtime uses canonical thread id metadata 对应的行为。
    #[test]
    fn code_ui_runtime_uses_canonical_thread_id_metadata() {
        let mut session = SessionState::new("/tmp/workspace");
        session.id = "legacy-session".to_string();
        session.metadata.insert(
            "thread_id".to_string(),
            serde_json::json!("11111111-1111-4111-8111-111111111111"),
        );

        assert_eq!(
            session_canonical_thread_id(&session).as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    // Test scenario: verifies `tui_code_ui_runtime_prefers_projection_bundle_identity` covers the tui code ui runtime prefers projection bundle identity behavior.
    // 测试场景：验证 `tui_code_ui_runtime_prefers_projection_bundle_identity` 覆盖 tui code ui runtime prefers projection bundle identity 对应的行为。
    #[tokio::test]
    async fn tui_code_ui_runtime_prefers_projection_bundle_identity() {
        let thread_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let actor = git_internal::internal::object::types::ActorRef::human("tester").unwrap();
        let bundle = ThreadBundle {
            thread: crate::internal::ai::projection::ThreadProjection {
                thread_id,
                title: Some("projection thread".to_string()),
                owner: actor.clone(),
                participants: vec![crate::internal::ai::projection::ThreadParticipant {
                    actor,
                    role: crate::internal::ai::projection::ThreadParticipantRole::Owner,
                    joined_at: Utc::now(),
                }],
                current_intent_id: None,
                latest_intent_id: None,
                intents: Vec::new(),
                metadata: None,
                archived: false,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                version: 1,
            },
            scheduler: crate::internal::ai::projection::SchedulerState {
                thread_id,
                selected_plan_id: None,
                selected_plan_ids: Vec::new(),
                current_plan_heads: Vec::new(),
                active_task_id: None,
                active_run_id: None,
                live_context_window: Vec::new(),
                metadata: None,
                updated_at: Utc::now(),
                version: 1,
            },
            freshness: crate::internal::ai::runtime::contracts::ProjectionFreshness::Fresh,
        };
        let mut session = SessionState::new("/tmp/workspace");
        session.id = "legacy-session".to_string();

        let runtime = build_tui_code_ui_runtime(
            "/tmp/workspace",
            &session,
            "ollama",
            "gemma4:31b",
            Some(&bundle),
            None,
            false,
            false,
            None,
        )
        .await;
        let snapshot = runtime.snapshot().await;

        assert_eq!(snapshot.session_id, thread_id.to_string());
        assert_eq!(snapshot.thread_id, Some(thread_id.to_string()));
    }

    // Test scenario: verifies `append_resume_audit_frame_records_resume_state` covers the append resume audit frame records resume state behavior.
    // 测试场景：验证 `append_resume_audit_frame_records_resume_state` 覆盖 append resume audit frame records resume state 对应的行为。
    #[test]
    fn append_resume_audit_frame_records_resume_state() {
        let temp_dir = tempfile::tempdir().unwrap();
        let working_dir = temp_dir.path();
        let mut session = SessionState::new(working_dir.to_string_lossy().as_ref());
        session.id = "11111111-1111-4111-8111-111111111111".to_string();
        session.metadata.insert(
            "thread_id".to_string(),
            serde_json::json!("11111111-1111-4111-8111-111111111111"),
        );

        let thread_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let resume_bundle = ResumeBundle::from_thread_bundle(ThreadBundle {
            thread: crate::internal::ai::projection::ThreadProjection {
                thread_id,
                title: Some("projection thread".to_string()),
                owner: git_internal::internal::object::types::ActorRef::human("tester").unwrap(),
                participants: Vec::new(),
                current_intent_id: Some(
                    Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
                ),
                latest_intent_id: Some(
                    Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
                ),
                intents: Vec::new(),
                metadata: None,
                archived: false,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                version: 1,
            },
            scheduler: crate::internal::ai::projection::SchedulerState {
                thread_id,
                selected_plan_id: None,
                selected_plan_ids: Vec::new(),
                current_plan_heads: Vec::new(),
                active_task_id: Some(
                    Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
                ),
                active_run_id: Some(
                    Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
                ),
                live_context_window: Vec::new(),
                metadata: Some(serde_json::json!({"ready_queue": []})),
                updated_at: Utc::now(),
                version: 1,
            },
            freshness: crate::internal::ai::runtime::contracts::ProjectionFreshness::Fresh,
        });

        let session_store = SessionStore::from_storage_path(&working_dir.join(".libra"));
        let session_jsonl_store = SessionJsonlStore::new(session_store.session_root(&session.id));

        append_resume_audit_frame(&session_jsonl_store, &session, &resume_bundle).unwrap();

        let events = session_jsonl_store.load_events().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionEvent::ContextFrame(frame) => {
                assert_eq!(frame.kind, ContextFrameKind::ResumeAudit);
                assert_eq!(frame.segments.len(), 1);
                assert_eq!(frame.segments[0].segment, ContextSegmentKind::SourceContext);
                assert_eq!(
                    frame.segments[0].source.kind,
                    crate::internal::ai::context_budget::ContextFrameSourceKind::Runtime
                );
                assert_eq!(frame.segments[0].source.label, "resume");
                assert!(frame.segments[0].content.as_ref().is_some_and(|content| {
                    content.contains("\"resume_reason\":\"interrupted_run\"")
                }));
                assert!(frame.segments[0].non_compressible);
            }
            other => panic!("expected resume audit context frame, got {other:?}"),
        }
    }

    // Test scenario: verifies `code_context_maps_to_task_intent_for_prompt_and_tool_policy` covers the code context maps to task intent for prompt and tool policy behavior.
    // 测试场景：验证 `code_context_maps_to_task_intent_for_prompt_and_tool_policy` 覆盖 code context maps to task intent for prompt and tool policy 对应的行为。
    #[test]
    fn code_context_maps_to_task_intent_for_prompt_and_tool_policy() {
        assert_eq!(
            task_intent_for_context(Some(CodeContext::Dev)),
            TaskIntent::Feature
        );
        assert_eq!(
            task_intent_for_context(Some(CodeContext::Review)),
            TaskIntent::Review
        );
        assert_eq!(
            task_intent_for_context(Some(CodeContext::Research)),
            TaskIntent::Question
        );
        assert_eq!(task_intent_for_context(None), TaskIntent::Unknown);
    }

    // Test scenario: verifies `system_preamble_includes_explicit_context_intent_and_dynamic_context` covers the system preamble includes explicit context intent and dynamic context behavior.
    // 测试场景：验证 `system_preamble_includes_explicit_context_intent_and_dynamic_context` 覆盖 system preamble includes explicit context intent and dynamic context 对应的行为。
    #[test]
    fn system_preamble_includes_explicit_context_intent_and_dynamic_context() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prompt = system_preamble(
            temp_dir.path(),
            Some(CodeContext::Review),
            CodeProvider::Openai,
            Some("gpt-test"),
        );

        assert!(prompt.contains("Code Review Mode"));
        assert!(prompt.contains("## Task Intent"));
        assert!(prompt.contains("intent=review"));
        assert!(prompt.contains("## Dynamic Workspace Context"));
        assert!(prompt.contains("source=libra status --short"));
        assert!(prompt.contains("## Context Budget Plan"));
    }

    // Test scenario: verifies `default_tui_runtime_context_denies_network_in_dev_mode` covers the default tui runtime context denies network in dev mode behavior.
    // 测试场景：验证 `default_tui_runtime_context_denies_network_in_dev_mode` 覆盖 default tui runtime context denies network in dev mode 对应的行为。
    #[test]
    fn default_tui_runtime_context_denies_network_in_dev_mode() {
        let (tx, _rx) = unbounded_channel();
        let runtime = default_tui_runtime_context(
            Path::new("/tmp/workspace"),
            Some(CodeContext::Dev),
            DefaultTuiApprovalConfig {
                policy: AskForApproval::OnRequest,
                allow_all_commands: false,
                ttl: DEFAULT_APPROVAL_TTL,
                cache_policy: ApprovalCachePolicy::default(),
            },
            false,
            tx,
        );

        let sandbox = runtime.sandbox.expect("sandbox context should be present");
        assert!(matches!(
            sandbox.policy,
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                ..
            } if writable_roots == vec![PathBuf::from("/tmp/workspace")] && network_access.is_denied()
        ));
    }

    // Test scenario: verifies `default_tui_runtime_context_allows_network_when_requested_in_dev_mode` covers the default tui runtime context allows network when requested in dev mode behavior.
    // 测试场景：验证 `default_tui_runtime_context_allows_network_when_requested_in_dev_mode` 覆盖 default tui runtime context allows network when requested in dev mode 对应的行为。
    #[test]
    fn default_tui_runtime_context_allows_network_when_requested_in_dev_mode() {
        let (tx, _rx) = unbounded_channel();
        let runtime = default_tui_runtime_context(
            Path::new("/tmp/workspace"),
            Some(CodeContext::Dev),
            DefaultTuiApprovalConfig {
                policy: AskForApproval::OnRequest,
                allow_all_commands: false,
                ttl: DEFAULT_APPROVAL_TTL,
                cache_policy: ApprovalCachePolicy::default(),
            },
            true,
            tx,
        );

        let sandbox = runtime.sandbox.expect("sandbox context should be present");
        assert!(matches!(
            sandbox.policy,
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                ..
            } if writable_roots == vec![PathBuf::from("/tmp/workspace")] && network_access.is_full()
        ));
    }

    // Test scenario: verifies `default_tui_runtime_context_can_allow_all_commands` covers the default tui runtime context can allow all commands behavior.
    // 测试场景：验证 `default_tui_runtime_context_can_allow_all_commands` 覆盖 default tui runtime context can allow all commands 对应的行为。
    #[tokio::test]
    async fn default_tui_runtime_context_can_allow_all_commands() {
        let (tx, _rx) = unbounded_channel();
        let runtime = default_tui_runtime_context(
            Path::new("/tmp/workspace"),
            Some(CodeContext::Dev),
            DefaultTuiApprovalConfig {
                policy: AskForApproval::OnRequest,
                allow_all_commands: true,
                ttl: DEFAULT_APPROVAL_TTL,
                cache_policy: ApprovalCachePolicy::default(),
            },
            true,
            tx,
        );

        let approval = runtime
            .approval
            .expect("approval context should be present");
        assert!(approval.store.lock().await.allow_all_commands());
    }

    // Test scenario: verifies `default_tui_runtime_context_is_read_only_for_review_and_research` covers the default tui runtime context is read only for review and research behavior.
    // 测试场景：验证 `default_tui_runtime_context_is_read_only_for_review_and_research` 覆盖 default tui runtime context is read only for review and research 对应的行为。
    #[test]
    fn default_tui_runtime_context_is_read_only_for_review_and_research() {
        for context in [CodeContext::Review, CodeContext::Research] {
            let (tx, _rx) = unbounded_channel();
            let runtime = default_tui_runtime_context(
                Path::new("/tmp/workspace"),
                Some(context),
                DefaultTuiApprovalConfig {
                    policy: AskForApproval::OnRequest,
                    allow_all_commands: false,
                    ttl: DEFAULT_APPROVAL_TTL,
                    cache_policy: ApprovalCachePolicy::default(),
                },
                true,
                tx,
            );

            let sandbox = runtime.sandbox.expect("sandbox context should be present");
            assert!(matches!(sandbox.policy, SandboxPolicy::ReadOnly));
        }
    }

    // ─── OC-Phase 2 P2.4: --agent override ────────────────────────────────
    // 中文：该注释与英文“─── OC-Phase 2 P2.4: --agent override ────────────────────────────────”含义一致。

    /// Build a working directory with a `.libra/agents/` profile that pins a
    ///     中文：该注释与英文“Build a working directory with a `.libra/agents/` profile that pins a”含义一致。
    /// structured `provider/model` binding so the override path has
    ///     中文：该注释与英文“structured `provider/model` binding so the override path has”含义一致。
    /// something to lift.
    ///     中文：该注释与英文“something to lift.”含义一致。
    fn write_agent_profile(working_dir: &Path, name: &str, body: &str) {
        let agents_dir = working_dir.join(".libra").join("agents");
        std::fs::create_dir_all(&agents_dir).expect("create agents dir");
        std::fs::write(agents_dir.join(format!("{name}.md")), body).expect("write profile");
    }

    /// Scenario: `--agent` is unset → helper is a no-op and returns `None`.
    ///     中文：该注释与英文“Scenario: `--agent` is unset → helper is a no-op and returns `None`.”含义一致。
    /// This is the flag-off baseline OC-Phase 2 P2.4 must preserve.
    ///     中文：该注释与英文“This is the flag-off baseline OC-Phase 2 P2.4 must preserve.”含义一致。
    // Test scenario: verifies `resolve_agent_override_noop_when_flag_absent` covers the resolve agent override noop when flag absent behavior.
    // 测试场景：验证 `resolve_agent_override_noop_when_flag_absent` 覆盖 resolve agent override noop when flag absent 对应的行为。
    #[test]
    fn resolve_agent_override_noop_when_flag_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let args = base_args();
        let result = resolve_agent_binding_override(&args, tmp.path()).unwrap();
        assert!(result.is_none());
    }

    /// Scenario: `--agent <name>` lifts a profile that carries
    ///     中文：该注释与英文“Scenario: `--agent <name>` lifts a profile that carries”含义一致。
    /// `model: anthropic/claude-3-5-sonnet-latest` into a structured
    ///     中文：该注释与英文“`model: anthropic/claude-3-5-sonnet-latest` into a structured”含义一致。
    /// `ModelBinding`. The legacy `model_preference` form is irrelevant
    ///     中文：该注释与英文“`ModelBinding`. The legacy `model_preference` form is irrelevant”含义一致。
    /// here; only the binding goes through.
    ///     中文：该注释与英文“here; only the binding goes through.”含义一致。
    // Test scenario: verifies `resolve_agent_override_lifts_provider_slash_model_binding` covers the resolve agent override lifts provider slash model binding behavior.
    // 测试场景：验证 `resolve_agent_override_lifts_provider_slash_model_binding` 覆盖 resolve agent override lifts provider slash model binding 对应的行为。
    #[test]
    fn resolve_agent_override_lifts_provider_slash_model_binding() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent_profile(
            tmp.path(),
            "planner",
            "---\n\
             name: planner\n\
             description: Implementation planner\n\
             tools: []\n\
             model: anthropic/claude-3-5-sonnet-latest\n\
             ---\n\
             You plan.",
        );
        let mut args = base_args();
        args.agent = Some("planner".to_string());

        let binding = resolve_agent_binding_override(&args, tmp.path())
            .unwrap()
            .expect("binding lifts");
        assert_eq!(binding.provider_id, "anthropic");
        assert_eq!(binding.model_id, "claude-3-5-sonnet-latest");
        assert!(binding.variant.is_none());
    }

    /// Scenario: an `--agent` profile that carries only a legacy alias
    ///     中文：该注释与英文“Scenario: an `--agent` profile that carries only a legacy alias”含义一致。
    /// (`model: default`) yields `Ok(None)` — there is no structured
    ///     中文：该注释与英文“(`model: default`) yields `Ok(None)` — there is no structured”含义一致。
    /// binding to override the CLI defaults with, so the rest of
    ///     中文：该注释与英文“binding to override the CLI defaults with, so the rest of”含义一致。
    /// `build_any_completion_model_for_args` falls through to the CLI
    ///     中文：该注释与英文“`build_any_completion_model_for_args` falls through to the CLI”含义一致。
    /// provider/model defaults.
    ///     中文：该注释与英文“provider/model defaults.”含义一致。
    // Test scenario: verifies `resolve_agent_override_returns_none_for_legacy_model_alias` covers the resolve agent override returns none for legacy model alias behavior.
    // 测试场景：验证 `resolve_agent_override_returns_none_for_legacy_model_alias` 覆盖 resolve agent override returns none for legacy model alias 对应的行为。
    #[test]
    fn resolve_agent_override_returns_none_for_legacy_model_alias() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent_profile(
            tmp.path(),
            "planner",
            "---\nname: planner\nmodel: default\n---\nbody",
        );
        let mut args = base_args();
        args.agent = Some("planner".to_string());

        let result = resolve_agent_binding_override(&args, tmp.path()).unwrap();
        assert!(result.is_none());
    }

    /// Scenario: an unknown agent name surfaces a `command_usage` error
    ///     中文：该注释与英文“Scenario: an unknown agent name surfaces a `command_usage` error”含义一致。
    /// listing the known profiles. Embedded defaults always load, so the
    ///     中文：该注释与英文“listing the known profiles. Embedded defaults always load, so the”含义一致。
    /// suggestion list is never empty.
    ///     中文：该注释与英文“suggestion list is never empty.”含义一致。
    // Test scenario: verifies `resolve_agent_override_unknown_name_lists_known_profiles` covers the resolve agent override unknown name lists known profiles behavior.
    // 测试场景：验证 `resolve_agent_override_unknown_name_lists_known_profiles` 覆盖 resolve agent override unknown name lists known profiles 对应的行为。
    #[test]
    fn resolve_agent_override_unknown_name_lists_known_profiles() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut args = base_args();
        args.agent = Some("does-not-exist".to_string());

        let err = resolve_agent_binding_override(&args, tmp.path())
            .expect_err("unknown agent must error");
        let msg = err.to_string();
        assert!(
            msg.contains("does-not-exist"),
            "error must mention the bad name: {msg}"
        );
        // Embedded `planner` is one of the catalogued profiles, so the
        // 中文：该注释与英文“Embedded `planner` is one of the catalogued profiles, so the”含义一致。
        // suggestion list must include it.
        // 中文：该注释与英文“suggestion list must include it.”含义一致。
        assert!(
            msg.contains("planner"),
            "error must list known profiles: {msg}"
        );
    }

    /// Scenario: a profile whose `mode: subagent` is selected by `--agent`
    ///     中文：该注释与英文“Scenario: a profile whose `mode: subagent` is selected by `--agent`”含义一致。
    /// is rejected. Sub-agents are dispatched via the `task` tool in
    ///     中文：该注释与英文“is rejected. Sub-agents are dispatched via the `task` tool in”含义一致。
    /// OC-Phase 3, not as the session driver.
    ///     中文：该注释与英文“OC-Phase 3, not as the session driver.”含义一致。
    // Test scenario: verifies `resolve_agent_override_rejects_non_primary_eligible_mode` covers the resolve agent override rejects non primary eligible mode behavior.
    // 测试场景：验证 `resolve_agent_override_rejects_non_primary_eligible_mode` 覆盖 resolve agent override rejects non primary eligible mode 对应的行为。
    #[test]
    fn resolve_agent_override_rejects_non_primary_eligible_mode() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent_profile(
            tmp.path(),
            "explorer",
            "---\n\
             name: explorer\n\
             mode: subagent\n\
             model: anthropic/claude-3-5-haiku-latest\n\
             ---\n\
             body",
        );
        let mut args = base_args();
        args.agent = Some("explorer".to_string());

        let err = resolve_agent_binding_override(&args, tmp.path())
            .expect_err("subagent-only profile must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("explorer"),
            "error must mention agent name: {msg}"
        );
        assert!(
            msg.contains("Subagent") || msg.contains("subagent"),
            "error must mention the offending mode: {msg}"
        );
    }

    /// Scenario: a `mode: all` profile IS primary-eligible, so the override
    ///     中文：该注释与英文“Scenario: a `mode: all` profile IS primary-eligible, so the override”含义一致。
    /// surfaces the binding rather than erroring. This pins the doc rule
    ///     中文：该注释与英文“surfaces the binding rather than erroring. This pins the doc rule”含义一致。
    /// "Primary | All" → primary-eligible.
    ///     中文：该注释与英文“"Primary | All" → primary-eligible.”含义一致。
    // Test scenario: verifies `resolve_agent_override_accepts_mode_all` covers the resolve agent override accepts mode all behavior.
    // 测试场景：验证 `resolve_agent_override_accepts_mode_all` 覆盖 resolve agent override accepts mode all 对应的行为。
    #[test]
    fn resolve_agent_override_accepts_mode_all() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent_profile(
            tmp.path(),
            "swiss",
            "---\n\
             name: swiss\n\
             mode: all\n\
             model: openai/gpt-4o-mini\n\
             ---\n\
             body",
        );
        let mut args = base_args();
        args.agent = Some("swiss".to_string());

        let binding = resolve_agent_binding_override(&args, tmp.path())
            .unwrap()
            .expect("binding lifts");
        assert_eq!(binding.provider_id, "openai");
        assert_eq!(binding.model_id, "gpt-4o-mini");
    }

    /// Scenario (OC-Phase 3 P3.1 flag-off invariant — production path):
    ///     中文：该注释与英文“Scenario (OC-Phase 3 P3.1 flag-off invariant — production path):”含义一致。
    /// the headless tool registry built by [`build_headless_tool_registry`]
    ///     中文：该注释与英文“the headless tool registry built by [`build_headless_tool_registry`]”含义一致。
    /// MUST NOT register a `task` tool. P3.1 only ships the schema
    ///     中文：该注释与英文“MUST NOT register a `task` tool. P3.1 only ships the schema”含义一致。
    /// constructor; runtime wiring lives in P3.2+ behind
    ///     中文：该注释与英文“constructor; runtime wiring lives in P3.2+ behind”含义一致。
    /// `code.multi_agent.enabled` (OC-Phase 5). A regression that wires
    ///     中文：该注释与英文“`code.multi_agent.enabled` (OC-Phase 5). A regression that wires”含义一致。
    /// the dispatcher unconditionally would fail this test by surfacing
    ///     中文：该注释与英文“the dispatcher unconditionally would fail this test by surfacing”含义一致。
    /// `task` in the registry's `tool_names()`.
    ///     中文：该注释与英文“`task` in the registry's `tool_names()`.”含义一致。
    ///
    /// The TUI path inlines its registry construction inside
    ///     中文：该注释与英文“The TUI path inlines its registry construction inside”含义一致。
    /// `execute_tui` and is not testable in isolation; the unit-level
    ///     中文：该注释与英文“`execute_tui` and is not testable in isolation; the unit-level”含义一致。
    /// guard at
    ///     中文：该注释与英文“guard at”含义一致。
    /// `internal::ai::tools::registry::tests::registry_does_not_expose_task_tool_in_flag_off_default`
    ///     中文：该注释与英文“`internal::ai::tools::registry::tests::registry_does_not_expose_task_tool_in_flag_off_default`”含义一致。
    /// covers the fixture-level invariant for that path.
    ///     中文：该注释与英文“covers the fixture-level invariant for that path.”含义一致。
    // Test scenario: verifies `build_headless_tool_registry_omits_task_tool_in_flag_off_default` covers the build headless tool registry omits task tool in flag off default behavior.
    // 测试场景：验证 `build_headless_tool_registry_omits_task_tool_in_flag_off_default` 覆盖 build headless tool registry omits task tool in flag off default 对应的行为。
    #[test]
    fn build_headless_tool_registry_omits_task_tool_in_flag_off_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = build_headless_tool_registry(tmp.path(), tx);
        let names = registry.tool_names();
        assert!(
            !names.contains(&"task".to_string()),
            "OC-Phase 3 P3.1 invariant: `task` must not be registered in the \
             headless registry until the dispatcher lands and is gated; \
             got tool_names = {names:?}"
        );
    }

    /// Scenario: headless web mode now has a browser approval channel, a
    ///     中文：该注释与英文“Scenario: headless web mode now has a browser approval channel, a”含义一致。
    /// ToolRuntimeContext, and snapshot projection for direct plan updates, so
    ///     中文：该注释与英文“ToolRuntimeContext, and snapshot projection for direct plan updates, so”含义一致。
    /// the registry may expose the same guarded network/mutating/basic plan
    ///     中文：该注释与英文“the registry may expose the same guarded network/mutating/basic plan”含义一致。
    /// tools as TUI without bypassing sandbox, approval, or `--network-access
    ///     中文：该注释与英文“tools as TUI without bypassing sandbox, approval, or `--network-access”含义一致。
    /// deny`.
    ///     中文：该注释与英文“deny`.”含义一致。
    // Test scenario: verifies `build_headless_tool_registry_exposes_runtime_guarded_tools` covers the build headless tool registry exposes runtime guarded tools behavior.
    // 测试场景：验证 `build_headless_tool_registry_exposes_runtime_guarded_tools` 覆盖 build headless tool registry exposes runtime guarded tools 对应的行为。
    #[test]
    fn build_headless_tool_registry_exposes_runtime_guarded_tools() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = build_headless_tool_registry(tmp.path(), tx);
        let names = registry.tool_names();

        for tool in [
            "web_search",
            "apply_patch",
            "shell",
            "update_plan",
            "submit_plan_draft",
        ] {
            assert!(
                names.iter().any(|name| name == tool),
                "headless registry must expose guarded tool `{tool}` after runtime context wiring; got {names:?}"
            );
        }
    }

    /// Scenario: an agent binding whose `provider_id` does NOT match any
    ///     中文：该注释与英文“Scenario: an agent binding whose `provider_id` does NOT match any”含义一致。
    /// `CodeProvider` variant must be rejected at
    ///     中文：该注释与英文“`CodeProvider` variant must be rejected at”含义一致。
    /// `effective_code_provider_for_args` with a clear, actionable error.
    ///     中文：该注释与英文“`effective_code_provider_for_args` with a clear, actionable error.”含义一致。
    /// Silent fallback to `args.provider` would leave system prompt and
    ///     中文：该注释与英文“Silent fallback to `args.provider` would leave system prompt and”含义一致。
    /// context-budget computations pointed at the CLI provider while the
    ///     中文：该注释与英文“context-budget computations pointed at the CLI provider while the”含义一致。
    /// model itself was built (or refused) for a different provider —
    ///     中文：该注释与英文“model itself was built (or refused) for a different provider —”含义一致。
    /// a partial-misconfiguration trap. Pinning this gate prevents the
    ///     中文：该注释与英文“a partial-misconfiguration trap. Pinning this gate prevents the”含义一致。
    /// regression Codex flagged on the OC-Phase 2 P2.4 review.
    ///     中文：该注释与英文“regression Codex flagged on the OC-Phase 2 P2.4 review.”含义一致。
    // Test scenario: verifies `effective_provider_rejects_unknown_binding_provider_id` covers the effective provider rejects unknown binding provider id behavior.
    // 测试场景：验证 `effective_provider_rejects_unknown_binding_provider_id` 覆盖 effective provider rejects unknown binding provider id 对应的行为。
    #[test]
    fn effective_provider_rejects_unknown_binding_provider_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent_profile(
            tmp.path(),
            "alien",
            "---\n\
             name: alien\n\
             model: aleph-omega/some-model\n\
             ---\n\
             body",
        );
        let mut args = base_args();
        args.agent = Some("alien".to_string());

        let err = effective_code_provider_for_args(&args, tmp.path())
            .expect_err("unknown binding provider must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("alien"),
            "error must mention the agent name: {msg}"
        );
        assert!(
            msg.contains("aleph-omega"),
            "error must echo the offending provider id: {msg}"
        );
        assert!(
            msg.contains("anthropic"),
            "error must list the known provider ids: {msg}"
        );
    }

    // Test scenario: verifies `build_helper_missing_api_key_errors_name_canonical_env_vars` covers the build helper missing api key errors name canonical env vars behavior.
    // 测试场景：验证 `build_helper_missing_api_key_errors_name_canonical_env_vars` 覆盖 build helper missing api key errors name canonical env vars 对应的行为。
    #[test]
    fn build_helper_missing_api_key_errors_name_canonical_env_vars() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cases: &[(CodeProvider, Option<&str>, Option<&str>, &str)] = &[
            (CodeProvider::Gemini, None, None, "GEMINI_API_KEY"),
            (CodeProvider::Openai, None, None, "OPENAI_API_KEY"),
            (CodeProvider::Anthropic, None, None, "ANTHROPIC_API_KEY"),
            (CodeProvider::Deepseek, None, None, "DEEPSEEK_API_KEY"),
            (CodeProvider::Kimi, None, None, "MOONSHOT_API_KEY"),
            (CodeProvider::Zhipu, None, None, "ZHIPU_API_KEY"),
            (
                CodeProvider::Ollama,
                Some("llama3.2"),
                Some("https://ollama.com"),
                "OLLAMA_API_KEY",
            ),
        ];

        for (provider, model, api_base, expected_env) in cases {
            let mut args = base_args();
            args.provider = *provider;
            args.model = model.map(str::to_string);
            args.api_base = api_base.map(str::to_string);
            let err = build_any_completion_model_for_args_with_lookup(
                &args,
                &CodeEnvFile::default(),
                tmp.path(),
                |_| None,
            )
            .expect_err("missing api key path must fire");
            let msg = err.to_string();
            assert!(
                msg.contains(expected_env),
                "expected {expected_env} in missing-key error for {provider:?}, got: {msg}"
            );
            assert!(
                msg.contains("is not set") || msg.contains("is required"),
                "missing-key error should be readable and actionable for {provider:?}, got: {msg}"
            );
        }
    }

    // Test scenario: verifies `build_helper_honors_cli_api_base_for_deepseek` covers the build helper honors cli api base for deepseek behavior.
    // 测试场景：验证 `build_helper_honors_cli_api_base_for_deepseek` 覆盖 build helper honors cli api base for deepseek 对应的行为。
    #[tokio::test]
    async fn build_helper_honors_cli_api_base_for_deepseek() {
        let (base_url, captured, server) = start_chat_completions_stub().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let mut args = base_args();
        args.provider = CodeProvider::Deepseek;
        args.model = Some("deepseek-chat".to_string());
        args.api_base = Some(base_url);
        let mut env_file = CodeEnvFile::default();
        env_file
            .values
            .insert("DEEPSEEK_API_KEY".to_string(), "test-key".to_string());

        let (model, model_name, provider_id) =
            build_any_completion_model_for_args(&args, &env_file, tmp.path())
                .expect("DeepSeek model builds with API key and custom base URL");
        assert_eq!(provider_id, "deepseek");
        assert_eq!(model_name, "deepseek-chat");

        let request = CompletionRequest::new(vec![crate::internal::ai::completion::Message::user(
            "hello",
        )]);
        let _response = model
            .completion(request)
            .await
            .expect("custom --api-base endpoint should receive the request");

        let bodies = captured.lock().await;
        assert_eq!(bodies.len(), 1, "expected exactly one provider POST");
        assert_eq!(
            bodies[0].get("model").and_then(|value| value.as_str()),
            Some("deepseek-chat"),
            "DeepSeek request should reach the CLI-provided --api-base endpoint"
        );
        server.abort();
    }

    // Test scenario: verifies `headless_ollama_reuses_provider_factory_bootstrap` covers the headless ollama reuses provider factory bootstrap behavior.
    // 测试场景：验证 `headless_ollama_reuses_provider_factory_bootstrap` 覆盖 headless ollama reuses provider factory bootstrap 对应的行为。
    #[tokio::test]
    async fn headless_ollama_reuses_provider_factory_bootstrap() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut args = base_args();
        args.provider = CodeProvider::Ollama;
        args.model = Some("llama3.2".to_string());
        let session_store = Arc::new(SessionStore::from_storage_path(&tmp.path().join(".libra")));
        let session_state = SessionState::new(&tmp.path().to_string_lossy());

        let runtime = build_non_codex_headless_runtime(
            &args,
            tmp.path(),
            session_store,
            session_state,
            false,
        )
        .await
        .expect("headless Ollama should build through ProviderFactory")
        .expect("Ollama is the supported non-Codex headless provider");
        let snapshot = runtime.snapshot().await;

        assert_eq!(snapshot.provider.provider, "ollama");
        assert_eq!(snapshot.provider.mode.as_deref(), Some("web-headless"));
        assert_eq!(snapshot.provider.model.as_deref(), Some("llama3.2"));
    }

    #[cfg(feature = "test-provider")]
    // Test scenario: verifies `headless_non_ollama_provider_reuses_provider_factory_bootstrap` covers the headless non ollama provider reuses provider factory bootstrap behavior.
    // 测试场景：验证 `headless_non_ollama_provider_reuses_provider_factory_bootstrap` 覆盖 headless non ollama provider reuses provider factory bootstrap 对应的行为。
    #[tokio::test]
    async fn headless_non_ollama_provider_reuses_provider_factory_bootstrap() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut args = base_args();
        args.provider = CodeProvider::Fake;
        let fixture_path = tmp.path().join("fake-fixture.json");
        args.fake_fixture = Some({
            std::fs::write(
                &fixture_path,
                r#"{"responses":[],"fallback":{"type":"text","text":"ok"}}"#,
            )
            .expect("fixture payload should be written");
            fixture_path
        });
        let session_store = Arc::new(SessionStore::from_storage_path(&tmp.path().join(".libra")));
        let session_state = SessionState::new(&tmp.path().to_string_lossy());

        let runtime = build_non_codex_headless_runtime(
            &args,
            tmp.path(),
            session_store,
            session_state,
            false,
        )
        .await
        .expect("headless Fake should build through ProviderFactory")
        .expect("Fake provider is now supported in headless provider factory path");
        let snapshot = runtime.snapshot().await;

        assert_eq!(snapshot.provider.provider, "fake");
        assert_eq!(snapshot.provider.mode.as_deref(), Some("web-headless"));
        assert_eq!(snapshot.provider.model.as_deref(), Some("fake-local"));
    }

    /// Scenario: `--provider gemini --model gpt-foo --agent planner`
    ///     中文：该注释与英文“Scenario: `--provider gemini --model gpt-foo --agent planner`”含义一致。
    /// (where `planner` carries `model: anthropic/claude-3-5-sonnet-latest`)
    ///     中文：该注释与英文“(where `planner` carries `model: anthropic/claude-3-5-sonnet-latest`)”含义一致。
    /// — the agent's binding wins **atomically**. The CLI `--model gpt-foo`
    ///     中文：该注释与英文“— the agent's binding wins **atomically**. The CLI `--model gpt-foo`”含义一致。
    /// is dropped because it would otherwise pair an OpenAI-style model id
    ///     中文：该注释与英文“is dropped because it would otherwise pair an OpenAI-style model id”含义一致。
    /// with the agent's anthropic provider. Smoke tests the integration of
    ///     中文：该注释与英文“with the agent's anthropic provider. Smoke tests the integration of”含义一致。
    /// `resolve_agent_binding_override` with the rest of
    ///     中文：该注释与英文“`resolve_agent_binding_override` with the rest of”含义一致。
    /// `build_any_completion_model_for_args`.
    ///     中文：该注释与英文“`build_any_completion_model_for_args`.”含义一致。
    #[cfg(feature = "test-provider")]
    // Test scenario: verifies `build_helper_treats_agent_binding_atomically` covers the build helper treats agent binding atomically behavior.
    // 测试场景：验证 `build_helper_treats_agent_binding_atomically` 覆盖 build helper treats agent binding atomically 对应的行为。
    #[test]
    fn build_helper_treats_agent_binding_atomically() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent_profile(
            tmp.path(),
            "planner",
            "---\n\
             name: planner\n\
             model: anthropic/claude-3-5-sonnet-latest\n\
             ---\n\
             body",
        );
        let mut args = base_args();
        args.provider = CodeProvider::Gemini;
        args.model = Some("gemini-2.0-flash".to_string()); // would-be hybrid
        args.agent = Some("planner".to_string());
        let env_file = CodeEnvFile::default();

        // The build call would fail (no API key in CodeEnvFile), but the
        // 中文：该注释与英文“The build call would fail (no API key in CodeEnvFile), but the”含义一致。
        // failure path tells us which provider we ended up dispatching to:
        // 中文：该注释与英文“failure path tells us which provider we ended up dispatching to:”含义一致。
        // an Anthropic build complains about ANTHROPIC_API_KEY, NOT
        // 中文：该注释与英文“an Anthropic build complains about ANTHROPIC_API_KEY, NOT”含义一致。
        // GEMINI_API_KEY.
        // 中文：该注释与英文“GEMINI_API_KEY.”含义一致。
        let err = build_any_completion_model_for_args(&args, &env_file, tmp.path())
            .expect_err("missing api key path must fire");
        let msg = err.to_string();
        assert!(
            msg.contains("ANTHROPIC_API_KEY"),
            "agent override must point env-var lookup at anthropic, got: {msg}"
        );
        assert!(
            !msg.contains("GEMINI_API_KEY"),
            "CLI --provider gemini must NOT win after agent override, got: {msg}"
        );
    }
}
