# Libra Web API Design

> **Status**: draft / interface specification
> **Owners**: Web team, Agent runtime team
> **Audience**: Rust backend implementers, web client engineers
> **Last updated**: 2026-04-25

This document defines the HTTP + streaming API contract that the **Libra Web client** (Next.js / React app under `web/`) needs from the **Libra agent backend** (Rust runtime exposed by `libra code`). Today the web client renders against an in-process mock data layer (`web/src/lib/mock/*`); when the Rust side starts serving these endpoints, the client swaps the mock module out for a real fetch layer with no UI changes.

The contract is shaped by the five-phase pipeline described in [docs/agent/agent-workflow.md](agent-workflow.md):

```
Phase 0 Intent  →  Phase 1 Plan  →  Phase 2 Execution  →  Phase 3 Validation  →  Phase 4 Release
```

Every UI surface in the web app maps to one or more of these phases:

| UI surface                     | Phase(s)            | Section                                |
|--------------------------------|---------------------|----------------------------------------|
| Sidebar threads list           | meta / cross-phase  | [Threads](#1-threads)                  |
| Chat transcript                | 0–2                 | [Messages](#2-messages--streaming)     |
| Workflow pipeline / cards      | 0–4                 | [Workflow snapshot](#3-workflow-snapshot) |
| Workflow → Summary tab         | 0–4                 | [Summary](#4-summary)                  |
| Workflow → Diff tab            | 2–3                 | [Patches & diff](#5-patches--diff)     |
| Workflow → Detail panel        | 0–4                 | [Detail records](#6-detail-records)    |
| Terminal pane                  | 2 (sandbox runtime) | [Sandbox stream](#7-sandbox-stream)    |
| Workspace footer Pause/Continue| 2                   | [Run control](#8-run-control)          |

---

## 0. Conventions

### 0.1 Base URL

```
{LIBRA_BASE_URL}/api/v1
```

For local development this is `http://127.0.0.1:7373/api/v1` — the same port that `libra code --web` already binds.

### 0.2 Authentication

Local: implicit loopback session. No header required.
Remote: `Authorization: Bearer <token>` issued by the Libra credential service.

The web client stores the token in `localStorage` under `libra.auth.token`. All examples below assume the loopback case.

### 0.3 Identifiers

| Prefix | Meaning              | Example       |
|--------|----------------------|---------------|
| `thr_` | Thread               | `thr_t1`      |
| `msg_` | Chat message         | `msg_8a3f`    |
| `pln_` | Plan                 | `plan-exec-04`|
| `stp_` | Plan step            | `s1`, `t1`    |
| `run_` | Execution run        | `run-11`      |
| `pat_` | PatchSet             | `ps-07`       |
| `frm_` | ContextFrame         | `cf-0418`     |
| `evd_` | Evidence             | `evd_b3d2`    |
| `int_` | Intent revision      | `r2`          |

IDs are opaque short strings. Clients must not parse them.

### 0.4 Timestamps

All timestamps are RFC 3339 UTC (`2026-04-25T10:42:13Z`). The UI converts to local time for display.

### 0.5 Phase enumeration

```ts
type PhaseKey = "intent" | "plan" | "execution" | "validate" | "release";
type PhaseOrdinal = 0 | 1 | 2 | 3 | 4;
```

### 0.6 Errors

```jsonc
// HTTP 4xx / 5xx
{
  "error": {
    "code": "phase_locked",            // stable machine code
    "message": "Phase 2 is gated on plan confirmation.",
    "phase": "execution",              // optional context
    "retryable": false
  }
}
```

Stable error codes the UI handles explicitly:

| Code                | Meaning                                                  | UI behavior                              |
|---------------------|----------------------------------------------------------|------------------------------------------|
| `unauthenticated`   | Missing/expired token                                    | Force re-auth                            |
| `forbidden`         | Token lacks scope                                        | Toast + disable action                   |
| `not_found`         | Thread / run / patch missing                             | Empty state                              |
| `phase_locked`      | Action illegal for current phase                         | Show inline gate banner                  |
| `intent_unconfirmed`| Phase 1+ requested before intent confirmed               | Bounce user to Intent card               |
| `sandbox_offline`   | Sandbox image not booted                                 | Disable terminal input                   |
| `rate_limited`      | Token quota exceeded                                     | Show remaining-quota chip                |

### 0.7 Streaming transport

Long-running surfaces (chat tokens, workflow updates, sandbox lines) use **Server-Sent Events** (`text/event-stream`).
Each event has a `type` and an opaque `seq` to support resume:

```
event: message.delta
id: 1842
data: {"messageId":"msg_8a3f","delta":"reading src/lib/query.ts…"}
```

Resume by sending `Last-Event-ID: <seq>` on reconnect.

WebSockets are intentionally avoided — SSE composes with the static-export web client and Next.js's edge runtime, and matches Libra's append-only event model.

---

## 1. Threads

A **Thread** is the top-level unit. One user-confirmed Intent → one Thread. A Thread carries one or more plan revisions, runs, evidence records, and a final release decision.

### 1.1 List threads — `GET /threads`

Returns the data behind the **Sidebar threads list**.

**Query params**

| Param   | Type            | Default | Notes                                  |
|---------|-----------------|---------|----------------------------------------|
| `q`     | string          | —       | Substring filter against thread titles |
| `phase` | `PhaseKey[]`    | —       | Filter to threads currently in phase   |
| `limit` | int (1–200)     | 50      |                                        |
| `cursor`| string          | —       | Opaque pagination cursor               |

**Response**

```json
{
  "items": [
    {
      "id": "thr_t1",
      "title": "Add optimistic updates to useMutation",
      "phase": 2,
      "phaseKey": "execution",
      "branch": "agent/optimistic-mutate",
      "updatedAt": "2026-04-25T10:46:11Z",
      "ago": "1m"
    }
  ],
  "nextCursor": null
}
```

`ago` is server-rendered as a human string for display parity. Clients may recompute from `updatedAt`.

### 1.2 Create thread — `POST /threads`

Creates a thread in **Phase 0 (Intent draft)**.

**Body**

```json
{ "title": "Optional seed title", "seedMessage": "Optional first user prompt" }
```

**Response**: a single thread entity (same shape as 1.1's items).

### 1.3 Get thread — `GET /threads/{threadId}`

Same shape as 1.1's item plus an embedded `intent` revision summary (without the full markdown body):

```json
{
  "id": "thr_t1", "title": "...", "phase": 2, "phaseKey": "execution",
  "branch": "agent/optimistic-mutate",
  "intent": { "id": "int_r2", "revision": "r2", "confirmed": true }
}
```

### 1.4 Delete / archive — `DELETE /threads/{threadId}`

Soft-archives. Threads in Phase 4 with a sealed IntentEvent cannot be hard-deleted.

---

## 2. Messages & streaming

The chat transcript is an append-only list scoped to one thread. Assistant tokens stream over SSE; user messages are atomic POSTs.

### 2.1 List messages — `GET /threads/{threadId}/messages`

```jsonc
{
  "items": [
    {
      "id": "msg_8a3f",
      "role": "user",                   // user | assistant
      "body": "Let's add optimistic…",
      "createdAt": "2026-04-25T10:42:00Z",
      "streaming": false
    },
    {
      "id": "msg_4b21",
      "role": "assistant",
      "body": "I read src/lib/query.ts…",
      "createdAt": "2026-04-25T10:42:14Z",
      "streaming": false,
      "modelId": "claude-sonnet-4.5"
    }
  ]
}
```

### 2.2 Post user message — `POST /threads/{threadId}/messages`

```json
{
  "body": "Looks right. One thing — the rollback has to preserve ordering…",
  "context": [
    { "kind": "file", "path": "src/lib/query.ts" }
  ],
  "mode": "Plan"
}
```

`mode` is `"Plan" | "Build"` — matches the composer toggle. `Plan` keeps phase 0/1 read-only; `Build` allows the agent to advance into Phase 2.

**Response**: the persisted message + the assistant's pending streaming message:

```json
{
  "user":      { "id": "msg_5a9c", "role": "user", "body": "...", "createdAt": "..." },
  "assistant": { "id": "msg_5a9d", "role": "assistant", "body": "", "streaming": true, "createdAt": "..." }
}
```

### 2.3 Stream events — `GET /threads/{threadId}/events` (SSE)

A single multiplexed event stream covering messages, workflow state, evidence, and run progress. The web client opens this once per active thread and routes events by `type`.

```
event: message.delta
data: {"messageId":"msg_5a9d","delta":"Got it — \"add optimistic updates"}

event: message.done
data: {"messageId":"msg_5a9d"}

event: workflow.patch
data: {"path":["plans","execution","steps","s3","status"],"value":"running"}

event: run.update
data: {"runId":"run-13","result":"running","ago":"now","patch":"…"}

event: evidence.append
data: {"kind":"tool","label":"grep \"MutationOptions\"","meta":"9 matches in 4 files"}

event: terminal.line
data: {"kind":"info","text":"[agent] capturing PatchSet ps-07"}

event: phase.changed
data: {"phase":2,"phaseKey":"execution","reason":"plan confirmed"}
```

Event types:

| Type                | Payload schema                                  | UI surface           |
|---------------------|-------------------------------------------------|----------------------|
| `message.delta`     | `{messageId, delta}`                            | Chat streaming text  |
| `message.done`      | `{messageId}`                                   | Chat finalize        |
| `workflow.patch`    | RFC 6902 add/replace patch on the snapshot tree | Workflow cards       |
| `run.update`        | Same shape as `ExecutionRun`                    | Runs card / timeline |
| `evidence.append`   | `EvidenceRow`                                   | Evidence pane        |
| `terminal.line`     | `TerminalLine`                                  | Terminal panel       |
| `phase.changed`     | `{phase, phaseKey, reason}`                     | Phase strip          |
| `intent.revised`    | `{intentId, revision}`                          | Intent card          |
| `decision.recorded` | `{kind: "auto"|"human", verdict}`               | Release card         |

### 2.4 Cancel a streaming message — `POST /threads/{threadId}/messages/{messageId}/cancel`

Stops the streaming message and triggers a `message.done` event with `{cancelled: true}`.

---

## 3. Workflow snapshot

The Workflow pane reads a single denormalized snapshot per thread, then applies SSE patches in place. This mirrors `web/src/lib/mock/workflow.ts` exactly.

### 3.1 Get snapshot — `GET /threads/{threadId}/workflow`

```jsonc
{
  "currentPhase": 2,
  "intent": {
    "id": "int_r2",
    "title": "Add optimistic updates to useMutation",
    "revision": "r2",
    "summary": "Introduce optimistic cache patching with rollback-on-error…",
    "constraints": [
      "Do not break MutationOptions<T> public shape",
      "Keep rollback safe under concurrent mutations",
      "Cover happy + error path with tests"
    ],
    "confirmed": true
  },
  "plans": {
    "execution": {
      "id": "plan-exec-04",
      "steps": [
        { "id": "s1", "label": "Snapshot cache at mutate() entry",          "status": "done"    },
        { "id": "s2", "label": "Apply optimistic patch to subscribers",     "status": "done"    },
        { "id": "s3", "label": "Per-key revision counter for safe rollback","status": "running" },
        { "id": "s4", "label": "Reconcile server response into cache",      "status": "queued"  },
        { "id": "s5", "label": "Surface onError with rollback context",     "status": "queued"  }
      ]
    },
    "test": {
      "id": "plan-test-02",
      "steps": [
        { "id": "t1", "label": "Happy-path optimistic update reflects immediately", "status": "queued" },
        { "id": "t2", "label": "Failure rolls back and preserves concurrent writes","status": "queued" },
        { "id": "t3", "label": "Reconciliation replaces optimistic entry",          "status": "queued" }
      ]
    }
  },
  "runs": [
    { "id": "run-11", "step": "s1", "result": "pass",    "ago": "2m", "patch": "+12 −0" },
    { "id": "run-12", "step": "s2", "result": "pass",    "ago": "2m", "patch": "+34 −7" },
    { "id": "run-13", "step": "s3", "result": "running", "ago": "now","patch": "…"      }
  ],
  "evidence": [
    { "kind": "tool",  "label": "read src/lib/query.ts",       "meta": "214 lines" },
    { "kind": "tool",  "label": "read src/hooks/useMutation.ts","meta": "88 lines" },
    { "kind": "tool",  "label": "grep \"MutationOptions\"",     "meta": "9 matches in 4 files" },
    { "kind": "frame", "label": "ContextFrame cf-0418",         "meta": "cache shape captured" },
    { "kind": "patch", "label": "PatchSet ps-07",               "meta": "+46 −7 across 2 files" }
  ],
  "tokensUsed": 48200,
  "graphHead": "agent/optimistic-mutate"
}
```

**Field semantics**

| Field            | Type                                   | Notes                                                |
|------------------|----------------------------------------|------------------------------------------------------|
| `currentPhase`   | `PhaseOrdinal`                         | Drives the phase strip + status badge                |
| `intent`         | `IntentDoc`                            | One per thread; `revision` increments on revisions   |
| `plans`          | `{execution: Plan, test: Plan}`        | Test plan is gated until execution DAG settles       |
| `runs`           | `ExecutionRun[]`                       | Chronological; `result === "running"` is exclusive   |
| `evidence`       | `EvidenceRow[]`                        | Append-only; never reordered                         |
| `tokensUsed`     | int                                    | Display: `48.2k` chip in workflow header             |
| `graphHead`      | string                                 | Display: footer of GitTimeline                       |

`StepStatus` is `"queued" | "running" | "done" | "failed"`. At most one step per plan may be `running`.

### 3.2 Update intent — `PATCH /threads/{threadId}/intent`

```json
{ "title": "...", "summary": "...", "constraints": ["..."], "confirmed": true }
```

Confirming an intent (`confirmed: true`) is the gate that admits Phase 1.

### 3.3 Confirm a plan — `POST /threads/{threadId}/plans/{planId}/confirm`

Returns 200 on success, `409 phase_locked` if the prerequisite phase is incomplete.

---

## 4. Summary

The **Summary tab** is a derived projection. It can be served as either a denormalized GET or computed by the client from the workflow snapshot — backend choice. The web client expects:

### 4.1 Get summary — `GET /threads/{threadId}/summary`

```jsonc
{
  "progress": [
    { "done": true,  "text": "Read src/lib/query.ts and snapshot current cache shape" },
    { "done": false, "text": "Wire per-key revision counter so rollback preserves ordering" }
  ],
  "branch": {
    "name": "agent/optimistic-mutate",
    "base": "main",
    "pr":   "No pull request",
    "changes": "2 files changed, 1 untracked"
  },
  "artifacts": [
    { "kind": "PatchSet", "id": "ps-07",   "meta": "+46 −7 across 2 files" },
    { "kind": "Frame",    "id": "cf-0418", "meta": "cache shape captured" }
  ],
  "todo": [
    { "done": true,  "text": "Snapshot cache at mutate() entry" },
    { "done": false, "text": "Per-key revision counter for safe rollback" }
  ]
}
```

`progress` reflects the agent's narrative checklist (often the message-level breakdown). `todo` mirrors execution-plan steps. Both must be ordered stably.

---

## 5. Patches & diff

The **Diff tab** is fed by one or more PatchSets emitted in Phase 2.

### 5.1 List patches — `GET /threads/{threadId}/patches`

```json
{
  "items": [
    { "id": "pat_ps-07", "createdAt": "2026-04-25T10:46:08Z", "stats": { "files": 2, "add": 46, "del": 7 } }
  ]
}
```

### 5.2 Get patch contents — `GET /patches/{patchId}`

```jsonc
{
  "id": "pat_ps-07",
  "stats": { "files": 2, "add": 46, "del": 7 },
  "files": [
    {
      "path": "src/lib/query.ts",
      "add": 34, "del": 7,
      "hunks": [
        {
          "header": "@@ -214,10 +214,23 @@ export function useMutation<T>(",
          "lines": [
            { "kind": "ctx", "n1": 214, "n2": 214, "text": "    const [state, setState] = React.useState…" },
            { "kind": "del", "n1": 217,             "text": "      const result = await fetcher(input);" },
            { "kind": "add",            "n2": 217, "text": "      const snap = cache.snapshot(key);" }
          ]
        }
      ]
    }
  ]
}
```

`kind` is `"ctx" | "add" | "del"`. Both line numbers are present for context lines; only `n1` for deletions; only `n2` for additions.

### 5.3 Latest patch shorthand — `GET /threads/{threadId}/patch`

Resolves to the most recent PatchSet for the thread — the Diff tab's default load.

---

## 6. Detail records

The Workflow detail panel opens five kinds of detail. Each kind has its own endpoint so the panel can paginate / lazy-load tool calls and outputs without bloating the snapshot.

### 6.1 Intent detail — `GET /threads/{threadId}/intent/{revision}`

Returns the full markdown body alongside the structured fields:

```json
{
  "id": "int_r2", "revision": "r2", "title": "Add optimistic updates to useMutation",
  "summary": "Introduce optimistic cache patching…",
  "constraints": ["..."],
  "markdown": "# Add optimistic updates to useMutation\n\n…",
  "confirmed": true,
  "createdAt": "2026-04-25T10:42:14Z"
}
```

### 6.2 Plan-step detail — `GET /plans/{planId}/steps/{stepId}`

```json
{
  "id": "s3", "label": "Per-key revision counter for safe rollback", "status": "running",
  "planId": "plan-exec-04", "planKind": "execution",
  "purpose": "Execution step — mutates cache/code inside the sandbox…",
  "toolCalls": [
    { "name": "read", "arg": "src/lib/query.ts", "result": "214 lines",  "running": false },
    { "name": "edit", "arg": "src/lib/query.ts", "result": "patchset ps-07", "running": false },
    { "name": "test", "arg": "useMutation.test.ts", "result": "running…", "running": true }
  ],
  "siblings": ["s1", "s2", "s4", "s5"]
}
```

### 6.3 Run detail — `GET /runs/{runId}`

```jsonc
{
  "id": "run-13",
  "step": "s3",
  "result": "running",
  "ago": "now",
  "patch": "…",
  "sandbox": "libra-sbx-04 · rw",
  "output": "$ cargo test --lib optimistic\n   Compiling libra-cache v0.3.1\n…",
  "diff": {
    "path": "src/lib/query.ts",
    "patch": "@@ useMutation ()\n- const result = await fetcher(input);\n+ const snap = cache.snapshot(key);\n…"
  }
}
```

### 6.4 Validation detail — `GET /threads/{threadId}/validation`

```json
{
  "checks": [
    { "name": "SAST · static analysis",      "status": "queued" },
    { "name": "SCA · dependency advisories", "status": "queued" },
    { "name": "Type-check",                  "status": "queued" },
    { "name": "Test plan · full run",        "status": "queued" },
    { "name": "Compatibility · API surface", "status": "queued" }
  ],
  "verdict": null,
  "evidenceLink": "/threads/thr_t1/evidence?kind=audit"
}
```

`verdict` is `"pass" | "fail" | null` (still running).

### 6.5 Release detail — `GET /threads/{threadId}/release`

```json
{
  "policy": "web3infra/default",
  "surface": "internal hook · 2 callers",
  "blastRadius": "low",
  "reversibility": "clean revert",
  "decision": null,
  "intentEventId": null
}
```

`decision` becomes `"auto-merge" | "request-review"` once Phase 4 closes; `intentEventId` is the signed event in the append-only log.

---

## 7. Sandbox stream

The Terminal panel reads from a per-thread sandbox.

### 7.1 Get history — `GET /threads/{threadId}/terminal`

```json
{
  "sandbox": {
    "id": "libra-sbx-04",
    "image": "rust:1.81-slim",
    "fs":    "rw(tmp)",
    "net":   "off"
  },
  "lines": [
    { "kind": "meta",   "text": "libra sandbox v0.4.2 · image rust:1.81-slim · net=off · fs=rw(tmp)" },
    { "kind": "prompt", "text": "cargo test --lib optimistic" },
    { "kind": "pass",   "text": "test optimistic::snapshot_before_mutate ... ok" }
  ]
}
```

`kind` is one of `"meta" | "prompt" | "stdout" | "pass" | "fail" | "run" | "warn" | "info"` — matches `TerminalLineKind` in `web/src/lib/mock/types.ts`.

### 7.2 Run a sandbox command — `POST /threads/{threadId}/terminal/exec`

```json
{ "cmd": "ls", "tab": "tools" }
```

The reply line(s) arrive over the thread SSE stream as `terminal.line` events. The HTTP response is just an acknowledgement:

```json
{ "accepted": true, "execId": "exec_91" }
```

The Rust side may reject commands when the sandbox is locked to agent execution (`sandbox_offline` / `phase_locked`).

---

## 8. Run control

The Workflow footer's Pause/Continue buttons map to two endpoints:

### 8.1 Pause — `POST /threads/{threadId}/control/pause`
Halts the execution DAG at the next safe checkpoint. The currently running step finishes (or rolls back via its sandbox guard); no further steps start.

### 8.2 Continue — `POST /threads/{threadId}/control/continue`
Resumes from the most recent paused checkpoint. Returns `409 phase_locked` if the thread isn't paused.

Both reply with the latest workflow snapshot for optimistic UI updates.

---

## 9. Threading model & idempotency

- Every write endpoint accepts an optional `Idempotency-Key` header. The Rust side persists `(threadId, key) → response` for 24h so retries don't duplicate messages or runs.
- Workflow patches are versioned: each `workflow.patch` event carries a monotonically increasing `version`. The client refetches the full snapshot if it observes a gap.
- The intent and the release decision are sealed via append-only `IntentEvent` records — the same primitive used by the Rust core (see [agent-overview-zh.md](agent-overview-zh.md)).

---

## 10. Mock → real swap plan

The web client today imports mock data through a single barrel:

```ts
// web/src/lib/mock/index.ts
export { THREADS, MESSAGES, WORKFLOW, SUMMARY, REVIEW, TERMINAL_LINES, PHASES } from "./*";
```

When the Rust backend ships, replace this barrel with a thin client module:

```ts
// web/src/lib/api/index.ts
export const PHASES = …;          // static; can stay client-side
export async function getThreads()         { return fetch("/api/v1/threads").then(r => r.json()); }
export async function getWorkflow(id: string) { … }
export function openThreadStream(id, onEvent) { /* EventSource */ }
```

Components currently consume mock constants directly. The migration step is:

1. Convert each consumer to a hook (e.g. `useThreads()`, `useWorkflow(threadId)`) backed by SWR/React Query.
2. Replace the mock module with HTTP calls; type signatures stay identical because the mock types in `web/src/lib/mock/types.ts` are the contract.
3. Add a shared `<ThreadStreamProvider>` that opens `/threads/{id}/events` once and dispatches events to per-card subscribers.

The mock module is the spec — keep its types in sync with this document.

---

## 11. Open questions

1. **Auth surface for self-hosted Libra**: do we federate via the Libra credential service, or accept any signed token from a configurable JWKS? Stub assumes the former.
2. **Multi-thread fan-out**: the SSE design opens one stream per active thread. Should the sidebar also subscribe to a coarse `/threads/events` stream for unread badges? Likely yes, but kept out of scope here.
3. **Patch storage**: PatchSets are large. Do we serve them inline as in §5, or stream them as `application/x-libra-patch` chunked? Inline is fine until a single hunk exceeds ~1MB.
4. **Terminal binary output**: today only text. If sandbox tools emit binary (e.g. flame graphs), we'll need a separate `terminal.attachment` event referencing an artifact ID.

These should be resolved before the first real implementation lands.
