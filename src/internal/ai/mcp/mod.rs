//! Libra MCP (Model Context Protocol) implementation.
//!
//! This module builds an MCP server on top of `rmcp` by implementing `ServerHandler`, exposing:
//! - **Resources** via `libra://` URIs for read-only access (object fetch, list-by-type, etc.).
//! - **Tools** for creating and listing AI workflow process objects (Task/Run/Plan/...).
//!
//! # Quick start (conceptual)
//!
//! 1. Construct `LibraMcpServer` in your host process (inject `Storage` and `HistoryManager`).
//! 2. Attach the server to your chosen MCP transport (stdio/http/websocket; handled by the host).
//! 3. Clients call tools to create objects, and read resources to fetch object JSON.
//!
//! # Resources (URI)
//!
//! - `libra://object/{object_id}`: read JSON by UUID (resolve id -> hash in history, then read blob from storage).
//! - `libra://objects/{object_type}`: list objects by type (one line per entry: `{object_id} {object_hash}`).
//! - `libra://history/latest`: returns the current history orphan-branch HEAD commit hash.
//! - `libra://context/active`: returns the active Run/Task/ContextSnapshot context as JSON.
//!
//! The supported `object_type` values match the history directory naming:
//! `task`, `run`, `snapshot`, `plan`, `patchset`, `evidence`, `invocation`, `provenance`, `decision`.
//!
//! # Tools
//!
//! Tools are mostly "create" and "list" pairs:
//! - Task: `create_task` / `list_tasks`
//! - Run: `create_run` / `list_runs`
//! - ContextSnapshot: `create_context_snapshot` / `list_context_snapshots`
//! - Plan: `create_plan` / `list_plans`
//! - PatchSet: `create_patchset` / `list_patchsets`
//! - Evidence: `create_evidence` / `list_evidences`
//! - ToolInvocation: `create_tool_invocation` / `list_tool_invocations`
//! - Provenance: `create_provenance` / `list_provenances`
//! - Decision: `create_decision` / `list_decisions`
//!
//! ## base_commit_sha (strong anchor)
//!
//! `base_commit_sha` is the commit anchor used by `create_run` / `create_context_snapshot` /
//! `create_patchset` to indicate which repository commit the workflow is based on.
//! The object model expects a 64-hex string:
//! - If the repo uses SHA-256: pass the 64-hex commit id as-is.
//! - If the repo uses SHA-1: you may pass a 40-hex commit id; the server will right-pad with `0`
//!   to 64 hex (reversible).
//!   See `crate::internal::ai::util::normalize_commit_anchor`.
//!
//! # Error conventions
//!
//! - If `Storage` is not injected: create tools return `"Storage not available"`.
//! - If `HistoryManager` is not injected: list tools and object reads return `"History not available"`.
pub mod server;
#[cfg(test)]
mod tests;
pub mod tools;
