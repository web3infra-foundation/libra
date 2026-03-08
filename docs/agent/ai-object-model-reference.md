# AI Object Model Reference (git-internal 0.7)

This document is the current reference for Libra's AI workflow model after upgrading to `git-internal = 0.7.0`.

## Scope

- Source of truth objects are under `git_internal::internal::object::*`.
- Libra MCP persists these objects on a single AI history branch (`refs/libra/intent`).
- Lifecycle status is event-sourced in 0.7.0.

## Core Model

```text
Intent ──▶ Plan ──▶ Task ──▶ Run ──▶ PatchSet ──▶ Decision
  │          │         │        │
  │          │         │        ├──▶ ToolInvocation
  │          │         │        ├──▶ Evidence
  │          │         │        └──▶ Provenance
  │          │         └────────────▶ TaskEvent
  │          └──────────────────────▶ PlanStepEvent
  ├──────────────────────────────────▶ IntentEvent
  └──────────────────────────────────▶ ContextSnapshot / ContextFrame
```

## Event-Sourced Lifecycle (Important)

In 0.7.0, mutable status is not stored on `Intent` / `Task` / `Run` objects.

- `Intent` lifecycle is tracked by `IntentEvent`.
- `Task` lifecycle is tracked by `TaskEvent`.
- `Run` lifecycle is tracked by `RunEvent`.
- Step execution lifecycle is tracked by `PlanStepEvent`.

Libra MCP `list_intents`, `list_tasks`, and `list_runs` rebuild status from the latest event per object.

## Object Notes

1. `Intent`
- Immutable revision of user prompt.
- Fields include `prompt`, `spec`, `parents`, `analysis_context_frames`.
- No `status` or `commit` field on the object itself.

2. `Plan`
- Belongs to one `Intent` (`Plan::new(actor, intent_id)`).
- Supports revision DAG via `parents`.
- Uses `context_frames` for planning-time context.
- Legacy `pipeline/fwindow` are not part of 0.7 object schema.

3. `Task`
- Stable work definition.
- Supports provenance links: `parent`, `intent`, `origin_step_id`, `dependencies`.
- No in-object lifecycle status.

4. `Run`
- Immutable execution envelope: `task`, optional `plan`, `commit`, optional `snapshot`, `environment`.
- No in-object runtime status/error/metrics.

5. `PatchSet`
- Candidate diff metadata: `run`, `sequence`, `commit`, `format`, `artifact`, `touched`, `rationale`.
- No `apply_status` field in 0.7. Acceptance/rejection is represented by events and `Decision`.

6. `Provenance`
- Stores provider/model config for a run.
- Uses `parameters` plus convenience fields `temperature` and `max_tokens`.

## MCP Mapping (Libra)

### Object creation tools

- `create_intent`: writes `Intent`; if lifecycle inputs are provided, also writes `IntentEvent`.
- `update_intent`: appends `IntentEvent` (no in-place intent mutation).
- `create_task`: writes `Task` + initial `TaskEvent`.
- `create_run`: writes `Run` + initial `RunEvent` (with optional reason/error/metrics).
- `create_plan`: writes `Plan` bound to `intent_id`.
- `create_patchset`: writes `PatchSet` with `sequence` + touched files.
- `create_evidence`, `create_tool_invocation`, `create_provenance`, `create_decision`: write corresponding immutable objects.

### List tools

- `list_intents`: event-derived status + prompt/spec summary.
- `list_tasks`: latest `TaskEvent`-derived status.
- `list_runs`: latest `RunEvent`-derived status.
- Other list tools summarize immutable object fields directly.

## MCP Input Policy (Current)

MCP parameter schemas now only expose fields that map to current `git-internal 0.7` object model semantics.

- Removed pre-0.7 compatibility inputs (pipeline/fwindow/apply-status/token-usage shim).
- Removed stale Intent task-link input (`CreateIntentParams.task_id`), because task provenance belongs on `Task.intent`.
- Removed `CreateContextSnapshotParams.base_commit_sha` because `ContextSnapshot` has no commit anchor field in 0.7.
- MCP create APIs now validate referenced object IDs (`task_id`, `run_id`, `plan_id`, etc.) when AI history is enabled, preventing dangling links.
- MCP also validates key cross-object relationships:
  - `Evidence.patchset_id` and `Decision.chosen_patchset_id` must belong to the same `run_id`.
  - `Run.plan_id` must match `Task.intent` when the task is intent-bound.
  - `Plan.parent_plan_ids` must belong to the same owning `intent_id`.
- UUID parameters consistently accept both plain UUID and `uuid:<id>` forms.

## Active Context Resource

`libra://context/active` resolves:

1. latest non-terminal run (terminal defined by latest `RunEvent` being `completed`/`failed`),
2. its parent task (status from latest `TaskEvent`),
3. linked `ContextSnapshot` when present.

If no active run exists, it falls back to latest non-terminal task.

## Minimal Usage Snippets

```rust
use git_internal::internal::object::{
    intent::Intent,
    intent_event::{IntentEvent, IntentEventKind},
    types::ActorRef,
};

let actor = ActorRef::human("jackie")?;
let intent = Intent::new(actor.clone(), "refactor MCP")?;
let mut event = IntentEvent::new(actor, intent.header().object_id(), IntentEventKind::Analyzed)?;
event.set_reason(Some("intent analyzed".into()));
```

```rust
use git_internal::internal::object::{
    task::Task,
    task_event::{TaskEvent, TaskEventKind},
    types::ActorRef,
};

let actor = ActorRef::agent("planner")?;
let task = Task::new(actor.clone(), "upgrade mcp", None)?;
let event = TaskEvent::new(actor, task.header().object_id(), TaskEventKind::Created)?;
```

```rust
use git_internal::internal::object::{
    run::Run,
    run_event::{RunEvent, RunEventKind},
    types::ActorRef,
};

let actor = ActorRef::agent("executor")?;
let run = Run::new(actor.clone(), task_id, base_commit_sha)?;
let mut event = RunEvent::new(actor, run.header().object_id(), RunEventKind::Patching)?;
event.set_reason(Some("generating patchset".into()));
```
