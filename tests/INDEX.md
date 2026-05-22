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
| `compat_matrix_alignment` | 1 | Guards the docs compat matrix vs. real subcommands | `docs/commands/`, `src/cli.rs` |
| `compat_branch_lossy_wrapper_guard` | 1 | Guards branch-name lossy conversion wrapper | `src/internal/branch.rs` |
| `compat_*_production_unwrap_guard` | 1 | Bans `unwrap()/expect()` in named production modules | `src/**` |
| `db_migration_test` | 1 | SQLite schema bootstrap + migration round-trip | `src/internal/db.rs`, `sql/` |

## Wave 2 — Code UI & local automation

| target | wave | one-line purpose | relevant src |
|---|---|---|---|
| `harness_self_test` | 2 | Smoke-checks the PTY harness itself | `tests/harness/` |
| `code_ui_scenarios` | 2 | End-to-end scenarios on the Code UI through the harness | `src/command/code.rs`, `src/internal/tui/` |
| `code_ui_remote_lease_matrix` | 2 | Browser/automation lease lifecycle matrix | `src/command/code.rs` controller, `src/command/code_control.rs` |
| `code_ui_remote_sse_matrix` | 2 | SSE event stream matrix from web view | `src/internal/tui/`, `src/command/code.rs` (axum) |
| `code_ui_remote_state_matrix` | 2 | Cross-surface state replication matrix | `src/internal/tui/`, `src/command/code_control.rs` |
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
| `ai_code_ui_headless_test` | 2 | Headless TUI rendering / event coverage | `src/internal/tui/` |
| `ai_code_ui_projection_test` | 2 | Projection snapshot replication | `src/internal/ai/history.rs`, `src/internal/tui/` |
| `ai_code_ui_wire_test` | 2 | Wire-format contract for UI events | `src/internal/tui/`, `src/internal/ai/agent/` |
| `intent_flow_test` | 2 | IntentSpec → Plan → Run pipeline (no live LLM) | `src/internal/ai/intentspec/`, `src/internal/ai/orchestrator/` |
| `e2e_mcp_flow` | 2 | End-to-end MCP server flow | `src/internal/ai/mcp/` |
| `mcp_integration_test` | 2 | MCP integration tests | `src/internal/ai/mcp/` |
| `ai_automation_test` | 2 | `.libra/automations.toml` rule execution | `src/internal/ai/automation/`, `src/command/automation.rs` |
| `ai_dag_tool_loop_test` | 2 | DAG-based tool loop regression | `src/internal/ai/agent/` |
| `ai_mock_provider_test` | 2 | Mock provider used by `test-provider` feature | `src/internal/ai/providers/` (test-only) |
| `agent_capture_migration_test` | 2 | Capture/replay store migration | `src/internal/ai/history.rs` |

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
| `code_ui_perf_smoke_test` | 6 | Code UI startup / first-token latency smoke | `src/command/code.rs`, `src/internal/tui/` |

---

## TODO — uncategorised (one-liner pass needed)

These targets are real but not yet indexed. AI/human owners: pick a row, replace
`TODO` with a one-line purpose + relevant src paths, and move it to the matching
Wave above.

```
ai_agent_baseline_test                          TODO
ai_approval_ttl_test                            TODO
ai_classifier_test                              TODO
ai_command_safety_test                          TODO
ai_compaction_filter_test                       TODO
ai_compaction_handoff_e2e_test                  TODO
ai_context_budget_test                          TODO
ai_context_compaction_prune_test                TODO
ai_context_frame_test                           TODO
ai_context_handoff_test                         TODO
ai_dagrs_081_spike_test                         TODO
ai_dynamic_prompt_test                          TODO
ai_file_undo_test                               TODO
ai_goal_completion_gate_test                    TODO
ai_goal_flag_off_regression_test                TODO
ai_goal_resume_test                             TODO
ai_goal_state_test                              TODO
ai_goal_supervisor_test                         TODO
ai_goal_verifier_test                           TODO
ai_hardening_contract_test                      TODO
ai_json_repair_test                             TODO
ai_libra_vcs_safety_test                        TODO
ai_memory_anchor_test                           TODO
ai_multi_agent_e2e_test                         TODO
ai_projection_resolver_test                     TODO
ai_provider_context_overflow_compact_loop_test  TODO
ai_provider_error_taxonomy_test                 TODO
ai_provider_retry_policy_test                   TODO
ai_provider_transform_test                      TODO
ai_runtime_contract_test                        TODO
ai_scheduler_plan_set_test                      TODO
ai_schema_migration_test                        TODO
ai_semantic_rust_test                           TODO
ai_semantic_tools_test                          TODO
ai_session_jsonl_test                           TODO
ai_skill_test                                   TODO
ai_source_pool_test                             TODO
ai_storage_flow_test                            TODO
ai_subagent_contract_test                       TODO
ai_usage_stats_test                             TODO
ai_usage_tui_test                               TODO
ai_validation_decision_flow_test                TODO
diagnostics_redaction_test                      TODO
local_client_test                               TODO
publish_ai_export_test                          TODO
publish_ai_object_model_contract_test           TODO
publish_incremental_test                        TODO
publish_preflight_test                          TODO
publish_redaction_contract_test                 TODO
publish_refs_test                               TODO
publish_snapshot_test                           TODO
publish_upload_test                             TODO
publish_worker_template_embed_test              TODO
redaction_contract_test                         TODO
```

---

## Maintenance

- Every new `tests/<name>.rs` must add a row here in the same PR (enforced by
  §10 of `docs/development/integration-test-plan.md`).
- Renames must update both this index and the plan; `scripts/check_integration_plan_consistency.sh`
  will fail CI on dangling references.
- TODO rows are tracked as `BASELINE_GAP-INTEG-007` — the index pass.
