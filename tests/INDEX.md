# tests/ INDEX

> One-line index of every integration test target in `tests/`.
> Format: `target | wave | one-line purpose | relevant src paths`
>
> - `target` is the cargo `--test` name (matches `tests/<target>.rs`).
> - `wave` references `docs/development/integration-test-plan.md §4`.
> - Use the three-part form `<target>::<test_fn>` whenever you reference a
>   specific test in PRs, reviews, or issue trackers (see §9.1 of the plan).
>
> Rows marked `TODO` need an owner pass; do not delete them — the file is the
> contract that AI reviewers reason against.

---

## Wave 1 — command layer & compat

| target | wave | one-line purpose | relevant src |
|---|---|---|---|
| `command_test` | 1 | Top-level dispatcher covering most `libra <subcmd>` integration paths | `src/command/`, `src/cli.rs` |
| `compat_stash_subcommand_surface` | 1 | Guards `libra stash` subcommand surface vs. git CLI | `src/command/stash.rs` |
| `compat_bisect_subcommand_surface` | 1 | Guards `libra bisect` subcommand surface | `src/command/bisect.rs` |
| `compat_worktree_delete_dir` | 1 | Guards worktree delete semantics on dir removal | `src/command/worktree.rs` |
| `compat_checkout_alias_help` | 1 | Guards `--help` text for checkout aliases | `src/command/checkout.rs` |
| `compat_matrix_alignment` | 1 | Guards public docs/release matrices vs. real CLI/API surfaces | `COMPATIBILITY.md`, `docs/automation/local-tui-control.md`, `.github/workflows/base.yml`, `src/cli.rs`, `src/internal/ai/web/mod.rs` |
| `compat_live_compat_workflow` | 1 | Guards optional live AI/cloud workflow remains manual/scheduled and secret-gated | `.github/workflows/live-compat.yml` |
| `compat_branch_lossy_wrapper_guard` | 1 | Guards branch-name lossy conversion wrapper | `src/internal/branch.rs` |
| `compat_lfs_client_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in `internal/protocol/lfs_client.rs` | `src/internal/protocol/lfs_client.rs` |
| `compat_config_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in `internal/config.rs` | `src/internal/config.rs` |
| `compat_head_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in `internal/head.rs` | `src/internal/head.rs` |
| `compat_util_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in `common_utils.rs` / `utils/` | `src/common_utils.rs`, `src/utils/` |
| `compat_client_storage_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in `utils/client_storage.rs` | `src/utils/client_storage.rs` |
| `compat_extra_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in miscellaneous modules | `src/**` |
| `compat_all_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in general production codebase | `src/**` |
| `compat_agent_run_non_exhaustive_guard` | 1 | Enforces `#[non_exhaustive]` on every `pub enum` under `agent_run/` for additive evolution | `src/internal/ai/agent_run/` |
| `compat_agent_docs_contract` | 1 | Guards active Agent plan claims against stale removed-provider status | `docs/improvement/agent.md`, `src/command/code.rs` |
| `compat_help_examples_banner` | 1 | Every visible command in `src/cli.rs::Commands` renders an `EXAMPLES:` / `Examples:` section in `<cmd> --help` (cross-cutting item B) | `src/cli.rs`, `src/command/**` |
| `compat_error_codes_doc_sync` | 1 | Every `LBR-*-NNN` literal in `src/utils/error.rs` is documented in `docs/error-codes.md` | `src/utils/error.rs`, `docs/error-codes.md` |
| `compat_command_docs_examples_section` | 1 | Every `docs/commands/<name>.md` page carries an `## Examples` / `## Common Commands` heading | `docs/commands/**` |
| `compat_help_flag_descriptions` | 1 | Every visible flag and positional under `Options:` / `Arguments:` carries a non-empty description; covers 42 root commands + 53 sub/sub-sub-commands (110 surfaces) | `src/cli.rs`, `src/command/**` |
| `compat_help_no_impl_meta_leak` | 1 | No `libra <cmd> --help` body leaks contributor-facing rustdoc into clap's long_about; forbids 6 phrase classes (e.g. `Codex pass-`, raw markdown headings, code fences) | `src/cli.rs`, `src/command/**` |

## Wave 2 — Code UI & local automation

| target | wave | one-line purpose | relevant src |
|---|---|---|---|
| `harness_self_test` | 2 | Smoke-checks the PTY harness itself | `tests/harness/` |
| `code_ui_scenarios` | 2 | End-to-end scenarios on the Code UI through the harness | `src/command/code.rs`, `src/internal/tui/` |
| `code_ui_remote_lease_matrix` | 2 | Browser/automation lease lifecycle matrix | `src/command/code.rs` controller, `src/command/code_control.rs` |
| `code_ui_remote_sse_matrix` | 2 | SSE event stream matrix from web view | `src/internal/tui/`, `src/command/code.rs` (axum) |
| `code_ui_remote_state_matrix` | 2 | Cross-surface state replication matrix, including mid-turn detach/cancel settling | `src/internal/tui/`, `src/internal/ai/web/code_ui.rs`, `src/command/code_control.rs` |
| `code_ui_remote_security_matrix` | 2 | Auth/token/origin enforcement matrix | `src/command/code_control*.rs` |
| `code_ui_remote_generation_matrix` | 2 | Generation control across surfaces (no live LLM) | `src/internal/tui/app.rs` |
| `code_ui_remote_approval_matrix` | 2 | Approval flow across TUI/Web/automation | `src/internal/ai/agent/` approvals |
| `code_cli_dispatch_test` | 2 | `libra code …` argv parsing & dispatch | `src/command/code.rs` |
| `code_provider_boot_test` | 2 | Provider/agent bootstrap inside `libra code` | `src/internal/ai/providers/`, `src/internal/ai/agent/` |
| `code_tool_acl_test` | 2 | Tool registry ACL & safety classification | `src/internal/ai/tools/` |
| `code_mcp_dual_entry_test` | 2 | MCP stdio + http dual entry parity | `src/internal/ai/mcp/`, `src/command/code.rs` |
| `code_resume_test` | 2 | Session resume across restarts | `src/internal/ai/session/`, `src/command/code.rs` |
| `code_codex_default_tui_test` | 2 | Codex runtime default TUI wiring | `src/internal/ai/agent/codex*` |
| `code_codex_runtime_test` | 2 | Codex runtime tool loop regression | `src/internal/ai/agent/codex*`, `src/internal/ai/tools/` |
| `ai_code_ui_headless_test` | 2 | Headless Code UI runtime and projection coverage | `src/internal/ai/web/headless.rs` |
| `ai_code_ui_projection_test` | 2 | Projection snapshot replication | `src/internal/ai/history.rs`, `src/internal/tui/` |
| `ai_code_ui_wire_test` | 2 | Wire-format contract for UI events | `src/internal/tui/`, `src/internal/ai/agent/` |
| `intent_flow_test` | 2 | IntentSpec → Plan → Run pipeline (no live LLM) | `src/internal/ai/intentspec/`, `src/internal/ai/orchestrator/` |
| `e2e_mcp_flow` | 2 | End-to-end MCP server flow | `src/internal/ai/mcp/` |
| `mcp_integration_test` | 2 | MCP integration tests | `src/internal/ai/mcp/` |
| `ai_automation_test` | 2 | `.libra/automations.toml` rule execution | `src/internal/ai/automation/`, `src/command/automation.rs` |
| `ai_dag_tool_loop_test` | 2 | DAG-based tool loop regression | `src/internal/ai/agent/` |
| `ai_mock_provider_test` | 2 | Mock provider used by `test-provider` feature | `src/internal/ai/providers/` (test-only) |
| `agent_capture_migration_test` | 2 | Capture/replay store migration | `src/internal/ai/history.rs` |
| `ai_agent_baseline_test` | 2 | Step 1.0 / CEX-00 single-agent baseline tests | `src/command/code.rs`, `src/internal/ai/agent/` |
| `ai_approval_ttl_test` | 2 | CEX-11 approval TTL and canonical key contract tests | `src/internal/ai/agent/` |
| `ai_classifier_test` | 2 | CEX-08 TaskIntent classifier contract tests | `src/internal/ai/completion/` |
| `ai_command_safety_test` | 2 | CEX-01 command safety contract tests | `src/internal/ai/commands/` |
| `ai_compaction_filter_test` | 2 | Integration tests for filter_compacted projection | `src/internal/ai/context_budget/` |
| `ai_compaction_handoff_e2e_test` | 2 | S5 compaction handoff end-to-end scenario | `src/internal/ai/context_budget/` |
| `ai_concurrency_lock_test` | 2 | Session-level advisory lock and CAS conflict tests | `src/command/code.rs`, `src/internal/ai/session/` |
| `ai_context_budget_test` | 2 | CEX-13a context budget core contract tests | `src/internal/ai/context_budget/` |
| `ai_context_compaction_prune_test` | 2 | S5 prune phase + budget-driven sequence tests | `src/internal/ai/context_budget/` |
| `ai_context_frame_test` | 2 | Context frame serialization and lifecycle | `src/internal/ai/context_budget/` |
| `ai_context_handoff_test` | 2 | S5 compaction handoff template parser tests | `src/internal/ai/context_budget/` |
| `ai_dagrs_081_spike_test` | 2 | Phase 0 spike for dagrs 0.8.1 API assumptions | `src/internal/ai/orchestrator/` |
| `ai_dynamic_prompt_test` | 2 | CEX-09 dynamic prompt and intent tool-policy tests | `src/internal/ai/prompt/` |
| `ai_file_undo_test` | 2 | CEX-10 file-level undo contract tests | `src/internal/ai/tools/` |
| `ai_goal_completion_gate_test` | 2 | OC-Phase 6 P6.7 completion gate scenarios | `src/internal/ai/goal/` |
| `ai_goal_flag_off_regression_test` | 2 | OC-Phase 6 Goal mode opt-in flag-off regression tests | `src/internal/ai/goal/` |
| `ai_goal_resume_test` | 2 | OC-Phase 6 Goal mode supervisor resume replay tests | `src/internal/ai/goal/` |
| `ai_goal_state_test` | 2 | OC-Phase 6 Goal mode schema integration tests | `src/internal/ai/goal/` |
| `ai_goal_supervisor_test` | 2 | OC-Phase 6 S6 supervisor non-completion E2E | `src/internal/ai/goal/` |
| `ai_goal_verifier_test` | 2 | OC-Phase 6 P6.2 deterministic GoalVerifier integration tests | `src/internal/ai/goal/` |
| `ai_hardening_contract_test` | 2 | Phase E hardening contract tests | `src/internal/ai/sandbox/` |
| `ai_json_repair_test` | 2 | JSON repair and correction parser tests | `src/internal/ai/completion/` |
| `ai_libra_vcs_safety_test` | 2 | CEX-02 run_libra_vcs parameter-level safety tests | `src/internal/ai/tools/` |
| `ai_memory_anchor_test` | 2 | Short-term/long-term memory anchor contract tests | `src/internal/ai/agent/` |
| `ai_multi_agent_e2e_test` | 2 | S7 multi-agent declarative config E2E | `src/internal/ai/agent/` |
| `ai_projection_resolver_test` | 2 | Phase B projection resolver and scheduler repository tests | `src/internal/ai/orchestrator/` |
| `ai_provider_context_overflow_compact_loop_test` | 2 | OC-Phase 4 context-overflow compaction loop integration tests | `src/internal/ai/providers/` |
| `ai_provider_error_taxonomy_test` | 2 | Integration fixtures for OC-Phase 4 provider error taxonomy | `src/internal/ai/providers/` |
| `ai_provider_retry_policy_test` | 2 | OC-Phase 4 retry-policy integration test | `src/internal/ai/providers/` |
| `ai_provider_transform_test` | 2 | Integration tests for OC-Phase 4 P4.1 provider transform pipeline | `src/internal/ai/providers/` |
| `ai_runtime_contract_test` | 2 | Wave 1A runtime contract tests pinning TaskExecutor | `src/internal/ai/runtime/` |
| `ai_scheduler_plan_set_test` | 2 | Phase 0 selected plan set and task dependency tests | `src/internal/ai/orchestrator/` |
| `ai_schema_migration_test` | 2 | Phase 0 schema migration tests for AI runtime contract tables | `src/internal/db.rs`, `sql/` |
| `ai_security_runtime_test` | 2 | Phase 5 security runtime (authz, redaction, shell, audit) | `src/internal/ai/sandbox/` |
| `ai_semantic_rust_test` | 2 | Semantic Rust code indexing and structure extraction | `src/internal/ai/skills/` |
| `ai_semantic_tools_test` | 2 | Semantic tools registration and classification | `src/internal/ai/tools/` |
| `ai_session_jsonl_test` | 2 | Session JSONL persistence format and event streaming | `src/internal/ai/session/` |
| `ai_skill_test` | 2 | System skills load, parse, and execution validation | `src/internal/ai/skills/` |
| `ai_source_pool_test` | 2 | CEX-14 source-pool isolation and MCP integration tests | `src/internal/ai/session/` |
| `ai_storage_flow_test` | 2 | Integration tests for AI object storage on local and R2 backends | `src/utils/storage/` |
| `ai_subagent_contract_test` | 2 | CEX-S2-10 schema contract tests | `src/internal/ai/agent_run/` |
| `ai_subagent_evidence_query_test` | 2 | CEX-S2-18 Step 2.8 read-only evidence query API: `evidence_query_by_scope` / `evidence_stream` (AND filter) / `merge_decision_distillable_evidence` over the frozen `AgentEvidence` / `MergeDecision` schema; empty-input → empty (flag-off analogue) | `src/internal/ai/agent_run/evidence_query.rs` |
| `ai_subagent_llm_initiated_test` | 3 | OC-Phase 3 LlmInitiated E2E: fake provider → dispatcher → `DefaultSubAgentChildRunner` → tool loop → parent JSONL `Spawned + Completed` | `src/internal/ai/agent/runtime/`, `src/internal/ai/providers/fake/`. Gated `--features test-provider`. |
| `ai_subagent_runtime_context_inheritance_test` | 3 | CEX-S2-12 / S2-INV-06 E2E: child tool invocation inherits the parent's `DispatchContext::runtime_context` (sandbox + approval + file-history authority + output budget) verbatim; a recording tool captures the invocation context, reverting the forward makes it observe `None` | `src/internal/ai/agent/runtime/sub_agent.rs`, `src/internal/ai/providers/fake/`. Gated `--features test-provider`. |
| `ai_subagent_user_initiated_test` | 3 | OC-Phase 3 UserInitiated{bypass_permission_ask:true} E2E: rejecting asker proves bypass really skips step 8; rest of the chain matches the LlmInitiated sibling | `src/internal/ai/agent/runtime/`, `src/internal/ai/providers/fake/`. Gated `--features test-provider`. |
| `ai_subagent_user_initiated_cancel_test` | 3 | OC-Phase 3 UserInitiated cancel E2E: pre-flight cancel short-circuits before JSONL writes; mid-flight parent abort returns `Cancelled { ParentAbort }`, parent JSONL writes `Spawned + Cancelled { UserRequested }`, and child JSONL replays to a cancelled snapshot | `src/internal/ai/agent/runtime/`. Gated `--features test-provider`. |
| `ai_subagent_worktree_readonly_test` | 3 | Sub-agent worktree isolation guard: pins historical edit-tool pre-filter and `libra code` workspace-isolation bootstrap wiring | `src/internal/ai/tools/registry.rs`, `src/internal/ai/permission/`, `src/command/code.rs` |
| `ai_usage_stats_test` | 2 | CEX-16 usage stats persistence and aggregation tests | `src/internal/ai/usage/` |
| `ai_usage_tui_test` | 2 | CEX-16 usage display formatting tests | `src/internal/ai/usage/` |
| `ai_validation_decision_flow_test` | 2 | Phase D validation and decision derived-record tests | `src/internal/ai/orchestrator/` |
| `db_migration_test` | 2 | SQLite schema bootstrap + migration round-trip | `src/internal/db.rs`, `sql/` |
| `diagnostics_redaction_test` | 2 | Diagnostics logs redaction and sanitization | `src/internal/ai/usage/` |
| `local_client_test` | 2 | Local Git protocol client working directory restoration on error | `src/internal/protocol/` |
| `publish_ai_export_test` | 2 | Publish pipeline export representation for AI tasks | `src/internal/publish/` |
| `publish_ai_object_model_contract_test` | 2 | Publish pipeline AI object model contract | `src/internal/publish/` |
| `publish_incremental_test` | 2 | Publish pipeline incremental sync and state tracking | `src/internal/publish/` |
| `publish_preflight_test` | 2 | Publish pipeline validation and preflight checks | `src/internal/publish/` |
| `publish_redaction_contract_test` | 2 | Publish pipeline redaction rules and scanning | `src/internal/publish/` |
| `publish_refs_test` | 2 | Publish pipeline references and branch tracking | `src/internal/publish/` |
| `publish_snapshot_test` | 2 | Publish pipeline snapshot generation and verification | `src/internal/publish/` |
| `publish_upload_test` | 2 | Publish pipeline bundle upload to cloud storage | `src/internal/publish/` |
| `publish_worker_template_embed_test` | 2 | Verification of embedded Worker template exclusion list | `src/internal/publish/` |
| `redaction_contract_test` | 2 | Pin the RedactedBytes contract for transcript output | `src/internal/ai/session/` |

## Wave 3 — network (test-network)

| target | wave | one-line purpose | relevant src |
|---|---|---|---|
| `network_remotes_test` | 3 | Real-network smoke tests against GitHub | `src/internal/protocol/`, `src/git_protocol.rs` |

## Wave 4 — Live AI (test-live-ai / DEEPSEEK_API_KEY)

| target | wave | one-line purpose | relevant src |
|---|---|---|---|
| `ai_agent_test` | 4 | Live LLM agent loop smoke | `src/internal/ai/agent/`, `src/internal/ai/providers/` |
| `ai_chat_agent_test` | 4 | Live LLM chat-mode agent | `src/internal/ai/agent/` |
| `code_ui_remote_model_generation_matrix` | 4 | Live model generation matrix (ignored by default) | `src/internal/ai/providers/`, `src/internal/tui/` |
| `ai_ollama_live_gate_test` | 4 | Ollama live-gate smoke | `src/internal/ai/providers/ollama/` |

## Wave 5 — Live Cloud (test-live-cloud / D1+R2)

| target | wave | one-line purpose | relevant src |
|---|---|---|---|
| `cloud_storage_backup_test` | 5 | D1/R2 backup + restore round-trip | `src/command/cloud.rs`, `src/utils/d1_client.rs`, `src/utils/client_storage.rs` |
| `publish_live_test` | 5 | Publish pipeline against live R2 | `src/publish/`, `src/command/publish.rs` |
| `storage_r2_test` | 5 | Object store R2 path | `src/utils/client_storage.rs` |

## Wave 6 — Performance smoke (LIBRA_RUN_PERF=1)

| target | wave | one-line purpose | relevant src |
|---|---|---|---|
| `code_ui_perf_smoke_test` | 6 | Code UI perf / SSE soak smoke | `src/command/code.rs`, `src/internal/tui/`, `src/internal/ai/web/` |

---

## TODO — uncategorised (one-liner pass needed)

None. All currently known integration targets have a wave, purpose, and
relevant source entry above.

---

## Maintenance

- Every new `tests/<name>.rs` must add a row here in the same PR (enforced by
  §10 of `docs/development/integration-test-plan.md`).
- Renames must update both this index and the plan. An automated integration-plan
  consistency check is still aspirational (tracked as `BASELINE_GAP-INTEG-008` in
  `docs/development/integration-test-plan.md`); until it lands, keep the index and
  plan in sync by hand.
- TODO rows are tracked as `BASELINE_GAP-INTEG-007` — the index pass.
