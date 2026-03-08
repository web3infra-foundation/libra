# MCP Upgrade Report (2026-03-08)

## 1. Scope

This report summarizes the `git-internal` upgrade and MCP refactor performed in the Libra repository.

- Repository: `/Users/jackie/rustprogram/libra`
- Baseline dependency: `git-internal = 0.6.0`
- Target dependency: `git-internal = 0.7.0`

## 2. git-internal 0.7 object model changes (used by this refactor)

`git-internal 0.7` moves AI workflow lifecycle state to event objects and keeps core objects immutable:

- `Intent` / `Task` / `Run` no longer carry mutable runtime status.
- Lifecycle is reconstructed from:
  - `IntentEvent`
  - `TaskEvent`
  - `RunEvent`
- `Plan` is intent-bound (`Plan::new(actor, intent_id)`) with plan revision parents/context frames.
- `PatchSet` no longer has `apply_status`; acceptance/rejection is represented via `Decision` and run events.
- `Provenance` uses structured `parameters` plus `temperature` and `max_tokens`.
- `ArtifactRef` API is simplified in 0.7 and was adapted in storage extensions.

## 3. MCP upgrade implementation summary

### 3.1 Core migration to 0.7 event model

- Updated MCP object persistence and list/read paths to align with event-sourced lifecycle.
- `list_intents`, `list_tasks`, `list_runs`, and `libra://context/active` now derive status from latest events.
- MCP create/update flows now emit `IntentEvent`, `TaskEvent`, and `RunEvent` correctly.

### 3.2 Compatibility-path removal (no data migration mode)

Per development-phase requirement, compatibility-only inputs were removed instead of maintaining migration shims:

- Removed from MCP input schema:
  - `CreatePlanParams.plan_version`
  - `CreatePlanParams.pipeline_id`
  - `CreatePlanParams.fwindow`
  - `CreatePatchSetParams.apply_status`
  - `CreateProvenanceParams.token_usage_json`
  - `CreateIntentParams.task_id`
  - `CreateContextSnapshotParams.base_commit_sha`
- Removed corresponding compatibility handling code and comments in MCP implementation.

### 3.3 Caller sync updates

Updated MCP callsites to match the new strict schema:

- `src/command/code.rs`
- `src/internal/ai/intentspec/persistence.rs`
- `src/internal/tui/app.rs`

### 3.4 Reference integrity guardrails

To avoid dangling workflow graphs, MCP create flows now validate referenced IDs (when history manager is present), including:

- `task_id` / `plan_id` / `context_snapshot_id` on run creation
- `run_id` on patchset/evidence/tool-invocation/provenance/decision creation
- `intent_id` / `parent_task_id` / `dependencies` on task creation
- `parent_ids` on intent revision creation
- `chosen_patchset_id` on decision creation

Additional relational integrity checks were added:

- `Evidence.patchset_id` and `Decision.chosen_patchset_id` must reference a patchset owned by the same `run_id`.
- `Run.plan_id` is rejected when the selected plan intent does not match the task's bound intent.
- `Plan.parent_plan_ids` are rejected when parent plans belong to a different intent than the new plan.
- `update_intent` now normalizes `intent_id` before lookup, so both `uuid:<id>` and plain UUID are accepted consistently.

### 3.5 Docs and comments sync

Updated documentation to match current behavior:

- `docs/agent/ai-object-model-reference.md`
- `src/internal/ai/mcp/mod.rs`
- `src/internal/ai/mcp/server.rs`
- `src/internal/ai/mcp/resource.rs`

## 4. Tests updated

- MCP and intent-flow related tests were updated for 0.7 event model and constructor/API changes.
- Integration coverage for MCP list/create/read flows was kept green after schema cleanup.

## 5. Verification results

All required checks were run on current workspace state:

1. `cargo +nightly fmt --all` passed.
2. `cargo clippy --all-targets --all-features` passed.
3. `cargo test --all` passed.
4. Cloud script run against current project code:
   - Script used: `/Users/jackie/rustprogram/libra/scripts/run_cloud_tests.sh`
   - Final result: passed (`14 passed; 0 failed`).

## 6. Commit phases

Relevant commits in this task chain:

- `ce3a370` `fix command parity behaviors`
- `660255b` `refactor mcp core for git-internal 0.7`
- `cc47eba` `update mcp docs and tests for event model`
- `bff564d` `align mcp params with git-internal 0.7`
