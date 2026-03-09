# AI Object Model Reference

This document describes the AI object model in `git-internal` after the
snapshot / event / Libra split.

## Design Principle

`git-internal` stores immutable historical facts.

- **Snapshot objects** answer: "what was stored at this revision?"
- **Event objects** answer: "what happened later?"
- **Libra projections** answer: "what is the system's current view?"

High-frequency runtime state must not be accumulated by rewriting parent
objects in `git-internal`.

## Three-Layer ASCII Diagram

```text
+--------------------------------------------------------------------------------------+
|                                      Libra [L]                                       |
|--------------------------------------------------------------------------------------|
| Thread / Scheduler / UI / Query Index                                                |
|                                                                                      |
|  current_intent_id                                                                   |
|  selected_plan_id                                                                    |
|  current_plan_heads[]                                                                |
|  active_task_id / active_run_id                                                      |
|  task_latest_run_id                                                                  |
|  run_latest_patchset_id                                                              |
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
|  Rule: every event is append-only; no parent object is rewritten to append history   |
+--------------------------------------------+-----------------------------------------+
                                             |
                                             v
+--------------------------------------------------------------------------------------+
|                              git-internal : Snapshot [S]                             |
|--------------------------------------------------------------------------------------|
|  Intent / Plan / Task / Run / PatchSet / ContextSnapshot / Provenance                |
|                                                                                      |
|  Rule: a snapshot only answers "what it is at this revision"                        |
+--------------------------------------------------------------------------------------+
```

## Libra Layer Terms

The Libra layer is not part of `git-internal` object storage. It holds
the current operational view reconstructed from immutable snapshots and
events.

### Thread

Conversation-level projection over related `Intent` snapshots.

- groups the `Intent` DAG for one ongoing discussion or task stream
- stores the current resume target, branch heads, and thread-local
  metadata
- can always be rebuilt from immutable history plus Libra-side
  projection records

### Scheduler

Runtime scheduler that turns immutable history into executable work.

- selects the active `Plan` head and computes current ready work
- tracks active `Task` / `Run`, retry routing, and replanning decisions
- manages the live execution order without rewriting snapshot objects

### UI

User-facing presentation layer over the current system view.

- shows the active thread, selected plan, task / run progress, and audit
  evidence
- reads from Libra projections and immutable history
- does not define historical truth; it only renders the current view

### Query Index

Rebuildable lookup and denormalized access structures used for fast
queries.

- examples: `intent -> plans`, `intent -> analysis_context_frames`,
  `task -> runs`, `run -> events`, `run -> patchsets`
- optimized for retrieval, filtering, and dashboard queries
- not part of the immutable object graph and can be recomputed
  if needed

## Main Object Relationships

```text
Snapshot layer
==============

Intent[S] --parents------------------------> Intent[S]
Intent[S] --analysis_context_frames-------> ContextFrame[E]
Plan[S]   --intent_id----------------------> Intent[S]
Plan[S]   --context_frames-----------------> ContextFrame[E]
Plan[S]   --parents------------------------> Plan[S]
Task[S]   --intent_id?---------------------> Intent[S]
Task[S]   --parent_task_id?----------------> Task[S]
Task[S]   --origin_step_id?---------------> Plan[S].step_id
Run[S]    --task_id------------------------> Task[S]
Run[S]    --plan_id?-----------------------> Plan[S]
Run[S]    --context_snapshot_id?-----------> ContextSnapshot[S]
PatchSet[S]   --run_id---------------------> Run[S]
Provenance[S] --run_id---------------------> Run[S]

Event layer
===========

IntentEvent[E]   --intent_id---------------> Intent[S]
IntentEvent[E]   --next_intent_id?---------> Intent[S]
ContextFrame[E]  --intent_id?--------------> Intent[S]
TaskEvent[E]     --task_id-----------------> Task[S]
RunEvent[E]      --run_id------------------> Run[S]
RunUsage[E]      --run_id------------------> Run[S]
PlanStepEvent[E] --plan_id-----------------> Plan[S]
PlanStepEvent[E] --step_id-----------------> Plan[S].step_id
PlanStepEvent[E] --run_id------------------> Run[S]
ToolInvocation[E] --run_id-----------------> Run[S]
Evidence[E]       --run_id-----------------> Run[S]
Evidence[E]       --patchset_id?----------> PatchSet[S]
Decision[E]       --run_id-----------------> Run[S]
Decision[E]       --chosen_patchset_id?---> PatchSet[S]
ContextFrame[E]   --run_id? / plan_id? / step_id? --> Run[S] / Plan[S] / Plan[S].step_id

Libra layer
===========

Thread[L] --------current_intent_id-------> Intent[S]
Thread[L] --------latest_intent_id--------> Intent[S]
Thread[L] --------intents[].intent_id-----> Intent[S]
Thread[L] --------intents[].is_head-------> marks current branch heads

Scheduler[L] -----selected_plan_id--------> Plan[S]
Scheduler[L] -----current_plan_heads------> Plan[S]
Scheduler[L] -----active_task_id----------> Task[S]
Scheduler[L] -----active_run_id-----------> Run[S]
Scheduler[L] -----live_context_window-----> ContextFrame[E]

QueryIndex[L] ----task_latest_run_id------> Run[S]
QueryIndex[L] ----run_latest_patchset_id--> PatchSet[S]
QueryIndex[L] ----reverse indexes---------> all [S] / [E]
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

`IntentEvent.next_intent_id` is a recommendation edge for "what
Intent should be handled next after this one completed". It does not
replace `Intent.parents`, which remains the semantic revision lineage.

### Runtime / projection state in Libra

- current selected plan head
- active task / active run
- thread heads / latest intent
- live context window
- reverse indexes and query acceleration

## Object Notes

### Intent

Snapshot of the user request and optional analyzed spec.

- keep: `parents`, `prompt`, `spec`, `analysis_context_frames`
- do not keep in snapshot: mutable status log, selected plan pointer, final commit pointer
- lifecycle belongs to `IntentEvent`
- `analysis_context_frames` freezes the context used to derive this
  `IntentSpec` revision

### Plan

Snapshot of the strategy and step structure.

- keep: `intent`, `parents`, `context_frames`, `steps`
- `context_frames` is planning-time context used to derive the plan
  from the `IntentSpec`, not prompt-analysis context
- `PlanStep.step_id` is the stable logical step identity across Plan revisions
- execution-time step state belongs to `PlanStepEvent`

### Task

Stable work definition.

- keep: title, description, goal, constraints, acceptance criteria, requester
- keep canonical provenance links: `intent`, `parent`, `origin_step_id`, `dependencies`
- runtime progress belongs to `TaskEvent`

### Run

Execution-attempt envelope.

- keep: `task`, `plan`, `commit`, `snapshot`, `environment`
- phase changes, failure details, and metrics belong to `RunEvent`
- usage/cost belongs to `RunUsage`

### PatchSet

Candidate diff snapshot.

- keep: `run`, `sequence`, `commit`, `format`, `artifact`, `touched`, `rationale`
- acceptance/rejection belongs to `Decision` or Libra projection

### Provenance

Immutable model/provider configuration for one run.

- keep: provider/model/parameters/temperature/max_tokens
- usage belongs to `RunUsage`

### ContextFrame

Immutable incremental context record.

- replaces the old mutable `ContextPipeline` runtime container
- referenced by `Intent.analysis_context_frames`,
  `Plan.context_frames`, and `PlanStepEvent.consumed_frames` /
  `produced_frames`
- `intent_id` can attach a frame directly to the intent-analysis phase

## Summary Rule

```text
1. Snapshot stores "what it is"
2. Event stores "what happened"
3. Libra stores "what is current"
```
