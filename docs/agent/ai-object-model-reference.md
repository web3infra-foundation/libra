# AI Object Model Reference

This document is the consolidated reference for the current Libra Agent
design.

It summarizes the object model described in
`docs/agent/agent.md` and `docs/agent/agent-workflow.md`.
If there is any ambiguity, those two design documents remain the source
of truth.

## Core Boundary

```text
git-internal: immutable facts
Libra: current state / scheduling state / index projections
```

The system is split into three layers:

- `git-internal` snapshot objects answer: "what was defined at this revision?"
- `git-internal` event objects answer: "what happened later?"
- Libra projections answer: "what is the current operational view?"

Mutable runtime coordination must not be implemented by rewriting
snapshot objects.

## Layer Model

```text
+--------------------------------------------------------------------------------------+
|                                      Libra [L]                                       |
|--------------------------------------------------------------------------------------|
| Thread / Scheduler / UI / Query Index                                                |
|                                                                                      |
|  current_intent_id                                                                   |
|  latest_intent_id                                                                    |
|  selected_plan_ids[]                                                                 |
|  current_plan_heads[]                                                                |
|  active_task_id / active_run_id                                                      |
|  live_context_window                                                                 |
|  reverse indexes: intent->plans, task->runs, run->events, run->patchsets, ...       |
+--------------------------------------------+-----------------------------------------+
                                             |
                                             v
+--------------------------------------------------------------------------------------+
|                               git-internal : Event [E]                               |
|--------------------------------------------------------------------------------------|
|  IntentEvent / TaskEvent / RunEvent / PlanStepEvent / RunUsage                       |
|  ToolInvocation / Evidence / Decision / ContextFrame                                 |
|                                                                                      |
|  Rule: append-only execution facts and audit records                                 |
+--------------------------------------------+-----------------------------------------+
                                             |
                                             v
+--------------------------------------------------------------------------------------+
|                              git-internal : Snapshot [S]                             |
|--------------------------------------------------------------------------------------|
|  Intent / Plan / Task / Run / PatchSet / ContextSnapshot / Provenance                |
|                                                                                      |
|  Rule: immutable definitions and revisioned structure                                |
+--------------------------------------------------------------------------------------+
```

## Placement Rules

### Snapshot objects in `git-internal`

- `Intent`
- `Plan`
- `Task`
- `Run`
- `PatchSet`
- `ContextSnapshot`
- `Provenance`

### Event objects in `git-internal`

- `IntentEvent`
- `TaskEvent`
- `RunEvent`
- `PlanStepEvent`
- `RunUsage`
- `ToolInvocation`
- `Evidence`
- `Decision`
- `ContextFrame`

### Projection and runtime state in Libra

- `Thread`
- `Scheduler`
- UI-facing current view
- query indexes and reverse indexes
- live context window
- ready queue / parallel groups / checkpoints / retry routing

## Main Relationship Graph

```text
Snapshot layer
==============

Intent[S] --parents------------------------> Intent[S]
Intent[S] --analysis_context_frames-------> ContextFrame[E]
Plan[S]   --intent_id----------------------> Intent[S]
Plan[S]   --parents------------------------> Plan[S]
Plan[S]   --context_frames-----------------> ContextFrame[E]
Task[S]   --intent_id?---------------------> Intent[S]
Task[S]   --parent_task_id?----------------> Task[S]
Task[S]   --origin_step_id?---------------> Plan[S].step_id
Task[S]   --dependencies-------------------> Task[S]
Run[S]    --task_id------------------------> Task[S]
Run[S]    --plan_id?-----------------------> Plan[S]
Run[S]    --context_snapshot_id?-----------> ContextSnapshot[S]
PatchSet[S]   --run_id---------------------> Run[S]
Provenance[S] --run_id---------------------> Run[S]

Event layer
===========

IntentEvent[E]   --intent_id---------------> Intent[S]
IntentEvent[E]   --next_intent_id?---------> Intent[S]
TaskEvent[E]     --task_id-----------------> Task[S]
RunEvent[E]      --run_id------------------> Run[S]
RunUsage[E]      --run_id------------------> Run[S]
PlanStepEvent[E] --plan_id-----------------> Plan[S]
PlanStepEvent[E] --step_id-----------------> Plan[S].step_id
PlanStepEvent[E] --run_id?-----------------> Run[S]
ToolInvocation[E] --run_id-----------------> Run[S]
Evidence[E]       --run_id-----------------> Run[S]
Evidence[E]       --patchset_id?----------> PatchSet[S]
Decision[E]       --run_id-----------------> Run[S]
Decision[E]       --chosen_patchset_id?---> PatchSet[S]
ContextFrame[E]   --intent_id?-------------> Intent[S]
ContextFrame[E]   --run_id?----------------> Run[S]
ContextFrame[E]   --plan_id?---------------> Plan[S]
ContextFrame[E]   --step_id?---------------> Plan[S].step_id

Libra layer
===========

Thread[L] --------current_intent_id-------> Intent[S]
Thread[L] --------latest_intent_id--------> Intent[S]
Thread[L] --------intents[].intent_id-----> Intent[S]
Thread[L] --------intents[].is_head-------> marks current branch heads

Scheduler[L] -----selected_plan_ids[]-----> Plan[S]
Scheduler[L] -----current_plan_heads------> Plan[S]
Scheduler[L] -----active_task_id----------> Task[S]
Scheduler[L] -----active_run_id-----------> Run[S]
Scheduler[L] -----live_context_window-----> ContextFrame[E]
```

## Libra Runtime Terms

### Thread

`Thread` is the conversation-level projection root over a related
`Intent` DAG.

It owns the current conversational view, not immutable history.

Current design fields:

| Field | Type | Meaning |
|---|---|---|
| `thread_id` | `Uuid` | Libra-side primary key |
| `title` | `Option<String>` | Human-readable title |
| `owner` | `ActorRef` | Conversation creator |
| `participants` | `Vec<ThreadParticipant>` | Human and agent participants with thread-local metadata |
| `current_intent_id` | `Option<Uuid>` | Intent currently focused by UI / Scheduler |
| `latest_intent_id` | `Option<Uuid>` | Most recently linked Intent; resume fallback |
| `intents` | `Vec<ThreadIntentRef>` | Ordered membership with `ordinal`, `is_head`, `linked_at`, `link_reason` |
| `metadata` | `Option<serde_json::Value>` | UI and routing hints |
| `archived` | `bool` | Closed thread marker |

Notes:

- `participants` is not just `Vec<ActorRef>`; it carries thread-local
  role and join time metadata.
- `head_intent_ids` is represented by `ThreadIntentRef.is_head`, not by
  a duplicated standalone array.
- `current_intent_id` is the current focus.
- `latest_intent_id` is the latest linked revision and default resume
  fallback when no current focus is set.

### Scheduler

`Scheduler` is the runtime scheduling projection.

It answers: what should run now, what is active, and which execution /
test plan pair is currently selected.

Current design fields:

| Field | Type | Meaning |
|---|---|---|
| `selected_plan_ids` | `Vec<Uuid>` | Current canonical plan heads in stable order: `[execution_plan_id, test_plan_id]` |
| `current_plan_heads` | `Vec<Uuid>` | Active plan leaves |
| `active_task_id` | `Option<Uuid>` | Task currently emphasized by Scheduler / UI |
| `active_run_id` | `Option<Uuid>` | Live run attempt |
| `live_context_window` | `Vec<Uuid>` | Current visible `ContextFrame` ids |

Notes:

- `selected_plan_ids` is a logical fixed pair, not an open-ended list.
  Scheduler must maintain exactly one `execution` plan id and one `test`
  plan id in that order.
- Phase 2 uses a conservative stage barrier: run `execution_dag` first,
  then switch the active stage to `test` only after execution work is
  complete.

Scheduler may also derive or cache:

- ready queue
- `active_dag_stage` (`execution` or `test`)
- current stage DAG progress
- parallel groups
- checkpoints
- retry routing
- staging / integration state
- replanning decisions

### Query Index

`Query Index` is a rebuildable denormalized lookup layer for fast reads.

Typical indexes:

- `intent -> plans`
- `intent -> context_frames`
- `task -> runs`
- `run -> events`
- `run -> patchsets`

Indexes are not historical truth and must be safe to rebuild.

## Object Notes

### Intent

Immutable snapshot of the user request and analyzed specification.

- keeps `parents`, `prompt`, `spec`, `analysis_context_frames`
- does not keep mutable lifecycle, selected plan pointers, or final
  execution outcomes
- lifecycle belongs to `IntentEvent`

### Plan

Immutable snapshot of strategy and step structure.

- keeps `intent`, `parents`, `steps`, `context_frames`
- `PlanStep.step_id` is the stable logical step identity across plan
  revisions
- runtime step progress belongs to `PlanStepEvent`
- provider-facing draft output is not a `Plan`; it is normalized into
  immutable `Plan.steps` only after the local planner accepts it
- `PlanStep` stays inside `Plan`; it is not a top-level snapshot object

There is no mutable `ExecutionPlan` object in `git-internal`.

### Task

Stable work-unit definition.

- keeps immutable provenance links:
  `intent`, `parent`, `origin_step_id`, `dependencies`
- runtime status, retries, and active run belong to events or Libra
  projection
- `Task.origin_step_id` points to the persisted `PlanStep.step_id`
  that produced the work-unit snapshot

### Run

Immutable execution-attempt envelope.

- keeps `task`, optional `plan`, `commit`, optional `snapshot`,
  `environment`
- status transitions and failure details belong to `RunEvent`
- usage and cost belong to `RunUsage`

### PatchSet

Immutable candidate diff snapshot.

- keeps `run`, `sequence`, `commit`, `format`, `artifact`, `touched`,
  `rationale`
- acceptance, rejection, and final selection belong to `Decision` or
  Libra projection

### Provenance

Immutable model / provider / execution-parameter record for one run.

- keeps provider, model, and execution parameters
- usage accounting is tracked separately by `RunUsage`

### ContextSnapshot

Optional stable environment baseline.

- used when the system needs a frozen starting or ending context
- not required for every phase
- should not be used as a mutable runtime context container

### ContextFrame

Immutable incremental context fact.

- replaces the old mutable `ContextPipeline` runtime concept
- may be attached to intent analysis, planning, execution, or step-level
  context
- readonly provider analysis in Phase 0 / Phase 1 should also emit
  `ContextFrame`
- Libra keeps only the current `live_context_window`

## Workflow Mapping

| Phase | Libra runtime / projection | Snapshot writes (`git-internal`) | Event writes (`git-internal`) |
|---|---|---|---|
| Phase 0 | Thread bootstrap, current intent revision, IntentSpec review, live context bootstrap | `Intent`, optional `ContextSnapshot` | `ToolInvocation`, `ContextFrame`, optional terminal `Decision` / `IntentEvent` |
| Phase 1 | selected plan head, current plan heads, plan review, ready queue preview | `Plan`, `Task` | `ToolInvocation`, `ContextFrame`, optional terminal `Decision` / `IntentEvent` |
| Phase 2 | live context window, retry / replan loop, staging area | `Run`, `PatchSet`, `Provenance` | `TaskEvent`, `RunEvent`, `PlanStepEvent`, `ToolInvocation`, `Evidence`, `ContextFrame`, `RunUsage` |
| Phase 3 | audit indexing, release candidate view | optional final `ContextSnapshot` | `Evidence`, `Decision`, terminal `TaskEvent` / `RunEvent` / `IntentEvent` |
| Phase 4 | review UI, current thread pointers | none | `Decision`, optional terminal `IntentEvent` |

Phase 0 `IntentSpec` review exposes only three formal actions:
`Confirm`, `Modify`, and `Cancel`.

There is no separate `Regenerate` workflow state in the object model.
If a UI offers a "try again" / regenerate affordance, Libra should model
it as `Modify` and reuse the same revision path instead of introducing a
distinct `Snapshot` / `Event` / `Projection` transition.

Phase 3 is a fixed validator pipeline over the release candidate. It
does not materialize or execute a planner-defined DAG.

## Rebuild and Read Contract

Projection loss must not block read access.

Rules:

1. `Thread`, `Scheduler`, and `Query Index` are rebuildable from
   immutable snapshots and events.
2. Missing projection rows mean "projection missing or stale", not
   necessarily "the logical object does not exist".
3. Read paths should prefer Libra projection first, then fall back to
   rebuild or history traversal when projection data is missing.

Examples:

- `Thread` can be rebuilt from `Intent`, `Intent.parents`, and
  `IntentEvent.next_intent_id` edges.
- `Scheduler` can be rebuilt from `Plan`, `Task`, `Run`,
  `PlanStepEvent`, and related execution events.
- query indexes can be regenerated by scanning snapshot and event
  history.

## Deprecated or Removed Concepts

The current design intentionally removes several older patterns:

- no mutable `ContextPipeline`; use `live_context_window +
  ContextFrame`
- no mutable in-object lifecycle fields on `Intent`, `Task`, or `Run`
- no mutable `ExecutionPlan` object in `git-internal`
- no "accepted" field written back to `PatchSet`

## Summary Rule

```text
1. Snapshot stores "what it is"
2. Event stores "what happened"
3. Libra stores "what is current"
```
