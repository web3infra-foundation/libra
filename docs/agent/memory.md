# Memory: Persistent, Versioned Knowledge for Libra Agents

This document specifies `Memory` — a Libra subsystem that lets agents
remember things across runs, threads, and branches without polluting
context with a flat blob like `CLAUDE.md`.

The design is informed by the
[`memoir-ai`](https://github.com/zhangfengcdt/memoir) project ("Git for
AI Memory"), plus related memory systems such as
[Letta / MemGPT](https://docs.letta.com/guides/agents/memory),
[LangGraph memory](https://docs.langchain.com/oss/python/langgraph/memory),
[OpenAI Agents SDK memory](https://openai.github.io/openai-agents-python/sandbox/memory/),
[Mem0](https://arxiv.org/abs/2504.19413), and
[Zep / Graphiti](https://arxiv.org/abs/2501.13956). It is adapted to
Libra's three-layer object model documented in [`agent.md`](./agent.md)
and [`ai-object-model-reference.md`](./ai-object-model-reference.md).

If anything in this document conflicts with `agent.md` or
`agent-workflow.md`, those documents win.

## 1. Goals and Non-Goals

### 1.1 Goals

- Give agents a **persistent, queryable knowledge store** that survives
  thread closure, process restart, and branch switches.
- Let humans **audit, diff, blame, and revert** what an agent learned —
  using the same Git-grade tooling Libra already builds for code.
- Keep retrieval **transparent and cheap**: hierarchical-path lookup
  first, LLM-based classification only when needed, and **no embedding
  index** in the base implementation.
- Reuse Libra's existing **Snapshot / Event / Projection** split so
  Memory inherits the same audit, rebuild, and concurrency guarantees as
  `Intent` / `Plan` / `Task` / `Run`.
- Be **branch-aware**: switching the user's working branch should swap
  the agent's memory view automatically.
- Keep writes **reviewable and reversible**: automatic capture may draft
  memories, but promotion to prompt-visible truth is gated by review
  state, confidence, provenance, and conflict checks.
- Support **multiple logical collections** in one repo store, so user
  facts, codebase onboarding, project onboarding, metrics, and private
  actor notes do not collapse into one noisy namespace.

### 1.2 Non-Goals

- Replace `ContextFrame`, `ContextSnapshot`, or `MemoryAnchor`. Memory
  is a new layer that complements them — see §3.1.
- Provide a vector / embedding search engine. Path-based recall is the
  default; embedding indexes can be a later extension.
- Provide a graph database in the base implementation. Temporal and
  entity graphs can be layered on top of Memory's event stream, but the
  historical source of truth remains `git-internal`.
- Silently store secrets, private data, or untrusted web claims. Intake
  must classify sensitivity and trust before any memory becomes prompt
  visible.
- Federate memory across repositories. Memory is a per-repo construct,
  the same way `.libra/` state is. Cross-repo federation is left to a
  future design.
- Compete with full chat history persistence. Memory stores **distilled,
  reusable facts**, not raw transcripts. Transcripts already live in
  `.libra/sessions/*.jsonl` and `git-internal` AI history.

## 2. Why Memory

### 2.1 The CLAUDE.md anti-pattern

Today many agents bolt long-term memory onto a flat global file
(`CLAUDE.md`, `MEMORY.md`, scratchpads). This has three failure modes
(memoir-ai's framing applies directly):

- **Context contamination.** A `git checkout` to a different branch
  leaves the agent reasoning with the *previous branch's* notes.
- **Token rent.** Every minor edit invalidates the prefix cache; the
  agent re-reads the entire memory blob each turn.
- **No versioning.** A bad insertion (a hallucination, a stale
  invariant) poisons every future retrieval. There is no `blame`, no
  `revert`, no `diff` over the memory itself.

### 2.2 What Libra already has

| Mechanism | Scope | Lifetime | Granularity |
|---|---|---|---|
| `ContextFrame` ([E]) | One Run / Plan / Step | Append-only inside a Run | Incremental fact, immutable |
| `ContextSnapshot` ([S]) | One Run / release candidate | Frozen baseline | Stable bundle of frames |
| `MemoryAnchor` ([E] in session JSONL) | One Thread or Project | Confirmed during a thread | Single rule, prompt-injectable |
| `Run` / `Evidence` / `Decision` | One execution attempt | Immutable | Audit fact |

Memory is the layer **above** all of these:

- `ContextFrame` is **within-run** scratch — gone the moment the Run is
  superseded.
- `MemoryAnchor` is **within-thread** scratch — useful for one
  conversation, but not addressable by path or branch.
- Memory is **across-thread, across-branch, queryable** — durable
  knowledge keyed by semantic path.

### 2.3 What memoir-ai gets right

memoir-ai's three core moves we adopt verbatim:

1. **Hierarchical semantic paths** (`procedural.coding.tabs`) instead of
   UUIDs or vector keys. O(log n) prefix lookup; humans can read them.
2. **Memory aggregation at the path** — multiple memories collected
   under one semantic location, not scattered as separate documents.
3. **Worthiness filtering** — not every turn deserves a memory write;
   the system explicitly classifies whether to remember at all.

memoir-ai uses a Prolly-tree backing store. **Libra does not need
this**: we already implement Git on-disk format and have first-class
content-addressed storage, refs, branches, commits, blame, and revert.
Memory rides on `git-internal` refs alongside the existing AI history
branch.

### 2.4 What other systems change in this design

External systems reinforce the same direction, but expose gaps that the
first draft must close:

| System | Useful idea | Libra adaptation |
|---|---|---|
| memoir-ai | Taxonomy paths, branch-aware memory, read hooks, Stop-hook capture, codebase onboarding namespaces | Adopt path-keyed storage and branch refs, but store historical truth in `git-internal` instead of a Prolly tree. Add namespaces so `default` user facts and `codebase:onboard` snapshots do not share retention or prompt policy. |
| Letta / MemGPT | Split always-visible core memory from on-demand archival memory | Map always-visible memory to `ContextSegmentKind::ProjectMemory` / `MemoryAnchor`; keep episodic and large semantic memory on-demand unless selected by recall. |
| LangGraph / Deep Agents | Distinguish semantic, episodic, procedural memory; support user / agent / organization scopes and background consolidation | Keep the CoALA-style kind axis, add `namespace`, and make consolidation a scheduled Memory job rather than an ad hoc prompt summary. |
| OpenAI Agents SDK memory | Progressive disclosure: small summary first, then search index, then detailed rollout summaries; memories may be stale | `memory.summarize()` must be the default agent-facing primitive. Retrieved notes are guidance and must carry evidence, confidence, trust, and staleness metadata. |
| Mem0 | Extraction, consolidation, graph-enhanced retrieval, and measurable latency / token savings | Add an extraction/consolidation pipeline and latency/token measurement. Keep vector/graph retrieval optional secondary indexes, never the source of truth. |
| Zep / Graphiti | Temporal facts and entity relationships improve multi-hop and "what changed when" recall | Add validity intervals, source timestamps, and explicit memory links now; defer entity-graph materialization to an extension. |
| [agentmemory.md](https://agentmemory.md/) / [Memoria](https://arxiv.org/abs/2512.12686) | Human-readable files, append-only logs, hybrid search, rollback, quarantine of low-confidence or contradictory facts | Keep auditability and rollback as first-class. Add quarantine, privacy gates, and projection-level pruning instead of destructive deletes. |

## 3. Conceptual Model

### 3.1 Memory vs ContextFrame vs MemoryAnchor

```text
within-run        cross-thread / cross-run
+---------+      +-------------------------+
| Context |      |        Memory           |
| Frame   |      | (this document)         |
| (per    |  --> | path-keyed,             |
|  Run)   |      | versioned, branched     |
+---------+      +-------------------------+
                              ^
                              | confirm / promote
                              |
                +-------------------------+
                | MemoryAnchor (within    |
                | thread, prompt-tier)    |
                +-------------------------+
```

Promotion rules:

- A `ContextFrame` discovered to be reusable (e.g. "user prefers tabs",
  detected during a Run) can be **distilled** into a Memory write.
- A `MemoryAnchor` confirmed in a thread can be **promoted** to a
  Memory entry under an appropriate path. Demotion (Memory → anchor) is
  the read-side operation: Memory entries relevant to the current
  prompt are projected back into the `with_memory_anchors()` injection
  slot at prompt-build time.

### 3.2 Four-axis classification

Every Memory entry is classified along four orthogonal axes:

- **Kind** (what it is): `procedural` / `semantic` / `episodic` —
  matches the CoALA agent-memory ontology used by memoir-ai's mementos.
- **Scope** (who/where it applies): `repo` / `branch` / `worktree` /
  `actor` / `global`. Scope determines which queries return the entry.
- **Namespace** (which collection it belongs to): `default`,
  `codebase:onboard`, `project:onboard`, `metrics.turn`,
  `metrics.code`, or `private:<actor-ref>`. Namespace determines
  retention, prompt-injection, onboarding, and review policy.
- **Lifecycle** (how it changes): `replacement` (overwrite-at-path,
  e.g. `semantic.user.timezone`) or `accretive` (append at path, e.g.
  `episodic.runs.<run-id>.outcome`).

`scope + namespace + path` identifies a **memory cell**, not a single
note. A cell may contain multiple live notes, matching memoir-ai's
aggregation model. Replacement lifecycle means "at most one confirmed
live note per logical fact", not "at most one note under the path".
Accretive lifecycle means all non-revoked notes under the path remain
visible until pruning policy removes their projection rows.

### 3.3 Memory taxonomy roots

Three top-level roots, mirroring CoALA / memoir-ai's mementos but
named for Libra's context (code agents in a VCS):

```text
procedural.*    -- HOW the agent should work
                   (rules, conventions, build/test commands,
                   repo-specific lints)
                   replacement-mostly

semantic.*      -- WHAT the world is
                   (user identity, tool inventory, infra facts,
                   architecture decisions)
                   replacement-mostly

episodic.*      -- WHAT has happened
                   (run outcomes, incidents, debugging breadcrumbs,
                   verified findings tied to a date or commit)
                   accretive
```

Examples (paths are illustrative, not normative):

```text
procedural.coding.style.tabs
procedural.coding.tests.command
procedural.review.merge-policy

semantic.user.timezone
semantic.user.preferences.terse-replies
semantic.repo.entry-binary
semantic.repo.architecture.three-layer-split

episodic.commits.cb8adb64.regression
episodic.runs.2026-05-09.flaky-test-1147
episodic.findings.context-window-too-small
```

A **fixed seed taxonomy** ships in the binary (~50–100 paths covering
the cases above). Agents may **expand** it via the iterative classifier
(§7), but expansions become first-class taxonomy nodes that get
audited like any other write.

Seed namespaces ship with policy defaults:

| Namespace | Purpose | Prompt default | Retention |
|---|---|---|---|
| `default` | User-captured and agent-captured durable facts | Summarize + selective inject | Kind-specific |
| `codebase:onboard` | Git repo structure, commands, current architecture, lessons | Inject compact summary at SessionStart | Refresh on commit movement / 30-day staleness |
| `project:onboard` | Non-git project structure and workflows | Inject compact summary at SessionStart | Refresh on filesystem snapshot hash |
| `metrics.turn` | Per-turn latency, token, tool, and outcome metrics | Never prompt-inject by default | Prune projection aggressively |
| `metrics.code` | Code-change audit metrics per branch | Never prompt-inject by default | Keep summaries; prune raw tails |
| `private:<actor-ref>` | Actor-local preferences or secrets-adjacent notes | Only visible to matching actor | Opt-in promotion only |

## 4. Object Model

Memory follows the **same Snapshot / Event / Projection split** as
the rest of Libra (see `ai-object-model-reference.md`). All three
layers are needed; skipping any of them re-introduces the CLAUDE.md
anti-pattern.

### 4.1 `MemoryNote` — Snapshot [S]

Immutable, content-addressed body of a single memory revision.

| Field | Type | Meaning |
|---|---|---|
| `note_id` | `Uuid` | Stable logical identity across revisions of the same fact |
| `revision_id` | `ObjectId` | Content hash of this revision (Git OID) |
| `namespace` | `String` | Logical collection, e.g. `default` or `codebase:onboard` |
| `path` | `String` | Taxonomy path, e.g. `procedural.coding.tabs` |
| `kind` | enum | `Procedural` / `Semantic` / `Episodic` |
| `scope` | enum | `Repo` / `Branch(name)` / `Worktree(id)` / `Actor(ref)` / `Global` |
| `lifecycle` | enum | `Replacement` / `Accretive` |
| `body` | `String` | The remembered statement (Markdown allowed, kept short) |
| `rationale` | `Option<String>` | Optional "why this matters" / "where it came from" |
| `evidence_refs` | `Vec<EvidenceRef>` | Pointers to `Evidence`, `Run`, `Decision`, commit OIDs that justify this memory |
| `links` | `Vec<MemoryLink>` | Explicit sibling / prerequisite / contradicts / supersedes links |
| `parents` | `Vec<ObjectId>` | Previous revisions of the same `note_id` (revision lineage) |
| `tags` | `Vec<String>` | Free-form labels (`security`, `flaky`, `infra`, …) |
| `confidence` | enum | `Low` / `Medium` / `High` (reused from `MemoryAnchorConfidence`) |
| `trust` | enum | `Verified` / `RepoEvidence` / `UserAsserted` / `ExternalUntrusted` / `Inferred` |
| `sensitivity` | enum | `Public` / `Internal` / `Confidential` / `SecretLike` |
| `valid_from` | `Option<DateTime<Utc>>` | When the fact starts being true, if known |
| `valid_until` | `Option<DateTime<Utc>>` | When the fact stops being true, if known |
| `expires_at` | `Option<DateTime<Utc>>` | Prompt-visibility TTL; historical note remains immutable |
| `author` | `ActorRef` | Human or agent who proposed this revision |
| `created_at` | `DateTime<Utc>` | Frozen at write |

Rules (mirroring `Intent` / `Plan` snapshot rules from `agent.md`):

- A `MemoryNote` snapshot answers **"what does the agent believe at
  this revision?"** and is never rewritten.
- Revoking, superseding, or pruning a memory is an **Event**, not an
  edit to the snapshot.
- `namespace`, `scope`, and `path` are logically immutable per
  `note_id`. To move a memory, write a new note and supersede the old
  one (§10.2).
- `SecretLike` notes may be stored only as redacted bodies with
  evidence references; they are never prompt-injected.

### 4.2 `MemoryEvent` — Event [E]

Append-only lifecycle record for a `MemoryNote`.

| Field | Type | Meaning |
|---|---|---|
| `event_id` | `Uuid` | Event identity |
| `note_id` | `Option<Uuid>` | Target note; absent for namespace / taxonomy / prompt meta-events |
| `revision_id` | `Option<ObjectId>` | Specific revision affected; absent for meta-events |
| `namespace` | `Option<String>` | Namespace affected by meta-events |
| `target_path` | `Option<String>` | Path affected by meta-events |
| `action` | enum | `Created` / `Revised` / `Confirmed` / `Quarantined(reason)` / `Superseded(by_revision)` / `Revoked(reason)` / `Pruned(policy)` / `RejectedAtIntake(reason)` / `TaxonomyExpanded` / `PromptTrimmed` / `SessionAttached` / `Consolidated` |
| `actor` | `ActorRef` | Who took the action |
| `at` | `DateTime<Utc>` | When |
| `evidence_refs` | `Vec<EvidenceRef>` | Optional new evidence justifying the action |
| `next_note_id` | `Option<Uuid>` | Same role as `IntentEvent.next_intent_id` — recommendation edge to a successor |

Rules:

- `MemoryEvent` is the **only** way memory state changes. There is no
  mutable field on `MemoryNote`.
- Note lifecycle events must carry `note_id` and `revision_id`.
  Namespace, taxonomy, and prompt-trimming meta-events carry
  `namespace` / `target_path` instead.
- The current "what does the agent believe right now at path X" is a
  **projection** (§4.3 and §4.4) computed by walking events.

### 4.3 `MemoryHead` — Projection [L]

Per-`(scope, namespace, path, note_id)` cursor pointing at the current
effective revision of one logical note, plus denormalised metadata for
fast reads. Lives in SQLite, not in `git-internal`.

| Field | Type | Meaning |
|---|---|---|
| `scope_key` | `String` | Canonical scope encoding (e.g. `branch:main`) |
| `namespace` | `String` | Logical collection |
| `path` | `String` | Taxonomy path |
| `note_id` | `Uuid` | Logical note |
| `head_revision_id` | `ObjectId` | Current effective revision |
| `head_action` | enum | Last action that produced this head (`Confirmed`, `Revised`, `Superseded`, …) |
| `head_review_state` | enum | `Draft` / `Confirmed` / `Quarantined` / `Revoked` / `Superseded` |
| `recent_revisions` | `Vec<ObjectId>` | Capped tail used by `memory log` |
| `last_used_at` | `DateTime<Utc>` | Updated on retrieval; drives pruning policy |
| `use_count` | `u64` | Updated on retrieval |
| `rank_hint` | `i64` | Deterministic prompt-order tie-breaker derived from kind, confidence, recency, and use count |

Rules:

- A missing `MemoryHead` row means **"projection missing"**, not
  **"memory does not exist"** — same contract as `Thread` and
  `Scheduler` projections (§7 of `agent.md`).
- The projection is fully rebuildable from `MemoryNote` + `MemoryEvent`
  history. `libra memory rebuild` performs this.

### 4.4 `MemoryPathSummary` — Projection [L]

Per-`(scope, namespace, path)` aggregate used for memoir-style path
aggregation and progressive disclosure.

| Field | Type | Meaning |
|---|---|---|
| `scope_key` | `String` | Canonical scope encoding |
| `namespace` | `String` | Logical collection |
| `path` | `String` | Taxonomy path |
| `confirmed_count` | `u64` | Confirmed live notes directly under this path |
| `quarantined_count` | `u64` | Quarantined live notes directly under this path |
| `child_count` | `u64` | Immediate child path count |
| `prefix_count` | `u64` | Confirmed live notes under this prefix |
| `preview` | `String` | Stable one-sentence summary for caller-driven recall |
| `last_changed_at` | `DateTime<Utc>` | Latest event affecting this path |
| `last_used_at` | `DateTime<Utc>` | Latest retrieval touching this path |

Rules:

- `memory.get(scope, namespace, path)` returns all confirmed
  `MemoryHead` rows for that cell, ordered by `rank_hint`.
- `memory.get_note(note_id)` is the direct single-note lookup.
- `MemoryPathSummary` is allowed to be lossy; it is a prompt-selection
  aid, not historical truth.

### 4.5 `MemoryTaxonomy` — Projection [L]

Cached, rebuildable view of the active taxonomy tree.

| Field | Type | Meaning |
|---|---|---|
| `path` | `String` | Full path, e.g. `procedural.coding` |
| `parent_path` | `Option<String>` | `procedural` for the row above |
| `is_seed` | `bool` | `true` if shipped in the binary |
| `expanded_from` | `Option<EventRef>` | Which iterative classifier event introduced this branch |
| `note_count` | `u64` | Live notes whose `path == self.path` |
| `prefix_count` | `u64` | Live notes whose path is under `self.path` (for `O(log n)` summarise) |
| `last_classified_at` | `DateTime<Utc>` | Drives staleness for the LLM cache |

### 4.6 Relationship graph

```text
Snapshot
========

MemoryNote[S] --parents---------------> MemoryNote[S]      (revision lineage)
MemoryNote[S] --evidence_refs---------> Evidence[E]
MemoryNote[S] --evidence_refs---------> Run[S] / Decision[E] / commit OID
MemoryNote[S] --links-----------------> MemoryNote[S]      (sibling / contradicts)

Event
=====

MemoryEvent[E] --note_id--------------> MemoryNote[S]
MemoryEvent[E] --revision_id----------> MemoryNote[S]
MemoryEvent[E] --next_note_id?--------> MemoryNote[S]

Projection
==========

MemoryHead[L] --(scope,namespace,path,note_id)--> MemoryNote[S].note_id
MemoryHead[L] --head_revision_id------> MemoryNote[S]
MemoryPathSummary[L] --(scope,namespace,path)---> set of MemoryHead[L]
MemoryTaxonomy[L] --path--------------> set of MemoryHead[L] / MemoryNote[S]

Cross-layer
===========

MemoryAnchor (existing) <-----promote--- MemoryHead[L]      (read-time projection
                                                           into prompt slot)
ContextFrame[E] -----distil-----------> MemoryNote[S]      (write-time)
```

## 5. Storage Layout

### 5.1 git-internal refs

Memory rides on the existing AI history branch convention.

```text
refs/libra/ai/main                       # existing AI history (Intent/Plan/...)
refs/libra/memory/main                   # new: Memory commits (NEW)
refs/libra/memory/branch/<branch-name>   # branch-scoped memory (NEW)
refs/libra/memory/worktree/<id>          # worktree-scoped memory (NEW)
```

A "memory commit" is a normal Git commit whose tree contains:

```text
notes/<namespace>/<note_id>/<revision_id>.json    # MemoryNote body
events/<yyyy>/<mm>/<event_id>.json                # MemoryEvent
taxonomy/expansion/<event_id>.json                # taxonomy expansion records
```

This means `libra log refs/libra/memory/branch/main` already works,
`libra blame` on a note path already works, and `libra cherry-pick`
across memory refs already works — no new VCS code.

### 5.2 SQLite projection tables

Add these projection tables to the schema in
`sql/sqlite_20260309_init.sql`:

```sql
-- Current head per live logical note.
CREATE TABLE memory_head (
    scope_key             TEXT NOT NULL,
    namespace             TEXT NOT NULL,
    path                  TEXT NOT NULL,
    note_id               TEXT NOT NULL,
    head_revision_id      TEXT NOT NULL,
    head_action           TEXT NOT NULL,
    head_review_state     TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    lifecycle             TEXT NOT NULL,
    confidence            TEXT NOT NULL,
    trust                 TEXT NOT NULL,
    sensitivity           TEXT NOT NULL,
    valid_from            TEXT,
    valid_until           TEXT,
    expires_at            TEXT,
    rank_hint             INTEGER NOT NULL DEFAULT 0,
    last_used_at          TEXT,
    use_count             INTEGER NOT NULL DEFAULT 0,
    updated_at            TEXT NOT NULL,
    PRIMARY KEY (scope_key, namespace, path, note_id)
);
CREATE INDEX idx_memory_head_lookup
    ON memory_head(scope_key, namespace, path, head_review_state);
CREATE INDEX idx_memory_head_path_prefix
    ON memory_head(namespace, path);

-- Current aggregate per path. This is the fast path for summarize(),
-- prompt injection, and taxonomy drill-down.
CREATE TABLE memory_path_summary (
    scope_key             TEXT NOT NULL,
    namespace             TEXT NOT NULL,
    path                  TEXT NOT NULL,
    confirmed_count       INTEGER NOT NULL DEFAULT 0,
    quarantined_count     INTEGER NOT NULL DEFAULT 0,
    child_count           INTEGER NOT NULL DEFAULT 0,
    prefix_count          INTEGER NOT NULL DEFAULT 0,
    preview               TEXT NOT NULL DEFAULT '',
    last_changed_at       TEXT NOT NULL,
    last_used_at          TEXT,
    PRIMARY KEY (scope_key, namespace, path)
);
CREATE INDEX idx_memory_path_summary_prefix
    ON memory_path_summary(namespace, path);

-- Reverse index: note_id -> head row, for O(1) "where does this note live?".
CREATE TABLE memory_note_index (
    note_id               TEXT PRIMARY KEY,
    scope_key             TEXT NOT NULL,
    namespace             TEXT NOT NULL,
    path                  TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    lifecycle             TEXT NOT NULL,
    review_state          TEXT NOT NULL,
    confidence            TEXT NOT NULL,
    trust                 TEXT NOT NULL,
    sensitivity           TEXT NOT NULL,
    created_at            TEXT NOT NULL
);

-- Derived link index. Historical truth is MemoryNote.links.
CREATE TABLE memory_link_index (
    source_note_id        TEXT NOT NULL,
    target_note_id        TEXT NOT NULL,
    link_kind             TEXT NOT NULL,
    source_path           TEXT NOT NULL,
    target_path           TEXT NOT NULL,
    PRIMARY KEY (source_note_id, target_note_id, link_kind)
);
CREATE INDEX idx_memory_link_target
    ON memory_link_index(target_note_id, link_kind);

-- Taxonomy projection (rebuildable).
CREATE TABLE memory_taxonomy_node (
    path                  TEXT PRIMARY KEY,
    parent_path           TEXT,
    is_seed               INTEGER NOT NULL,
    expanded_from         TEXT,
    note_count            INTEGER NOT NULL DEFAULT 0,
    prefix_count          INTEGER NOT NULL DEFAULT 0,
    last_classified_at    TEXT
);
CREATE INDEX idx_memory_taxonomy_parent ON memory_taxonomy_node(parent_path);

-- Optional: classifier cache, with TTL, keyed on
-- hash(scope + namespace + content + taxonomy_version).
CREATE TABLE memory_classifier_cache (
    cache_key             TEXT PRIMARY KEY,
    namespace             TEXT NOT NULL,
    suggested_path        TEXT NOT NULL,
    confidence            TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    expires_at            TEXT NOT NULL
);
```

These are projections, **not** historical truth. Rebuild from
`refs/libra/memory/...` if dropped.

### 5.3 ClientStorage tiering

`MemoryNote` blobs go through the same `ClientStorage` (local + S3/R2)
as other AI snapshots — see `agent-workflow.md` 2026-04-29 note. No
special handling: a memory body is just another small JSON blob.

Large memories (>`LIBRA_STORAGE_THRESHOLD`) tier out automatically.

## 6. Taxonomy

### 6.1 Built-in seed roots

The binary ships a fixed seed taxonomy under three roots
(`procedural`, `semantic`, `episodic`) with ~50–100 paths covering the
common cases for a code agent. Seed paths are marked `is_seed = true`
and may not be deleted (they can be empty).

### 6.2 Path grammar

```text
path        := segment ("." segment)*
segment     := [a-z][a-z0-9-]* | "<" identifier ">"
identifier  := [A-Za-z0-9-]+
```

- All-lowercase, hyphenated.
- `<...>` segments are dynamic (e.g. `episodic.runs.<run-id>.outcome`).
  Dynamic segments may not appear in seed paths.
- Maximum depth: **5 segments**. Deeper paths are forbidden — keeps
  retrieval prompts short.

### 6.3 Iterative expansion

When the LLM classifier (§7.3) is asked to place content that no
existing path covers, it may **propose** a new child segment.
Acceptance follows memoir-ai's `LLMIterativeTaxonomy` pattern:

1. Proposal must be under an existing parent.
2. Proposal must not exceed depth 5.
3. Proposal is recorded as a `MemoryEvent` with action `TaxonomyExpanded`
   (treated as a meta-event; not in §4.2 to keep that table focused on
   note lifecycles — it lives in the same event log).
4. Once accepted, the new path is a first-class taxonomy node and
   future writes can target it directly.

Cross-references between memories (memoir-ai's `related_keys`) are
stored historically in `MemoryNote.links` and projected into
`memory_link_index`. Link kinds:

- `sibling`: the same write was classified to multiple paths.
- `supports`: this note strengthens or explains another note.
- `contradicts`: this note conflicts with another live note and should
  trigger quarantine or manual resolution.
- `supersedes`: this note intentionally replaces another logical note.

Direct-path edits preserve existing `sibling` links by fetch-then-merge,
matching memoir-ai's edit semantics. Classifier-driven rewrites may
replace links because the classifier is deliberately reclustering the
note.

## 7. Classification Pipeline

A write request is `(content, optional_namespace, optional_path, scope,
kind?, lifecycle?, trust?, sensitivity?)`. Classification fills in the
missing fields.

### 7.1 Stage 0 — Worthiness filter

memoir-ai calls this "memory worthiness". For a code agent it
typically excludes:

- Greetings, small talk, transient state ("I'll check that for you").
- Restatements of code that is already in the diff.
- Tool error messages already captured in `Evidence`.
- Secrets, tokens, credentials, private keys, or high-risk personal data
  unless the stored body is redacted and the note is marked
  `SecretLike` so it cannot be prompt-injected.
- External-web claims that are not tied to a fetched source or explicit
  user assertion.

The filter is **deterministic-first** (regex / heuristic) and
**LLM-fallback** for borderline cases. A worthiness rejection is
recorded as a `MemoryEvent` with action `RejectedAtIntake` so humans
can see why the agent didn't remember something.

### 7.2 Stage 1 — Pattern classifier (offline)

If the caller already supplied a `path`, skip directly to validation
(§7.4). Otherwise:

- Look up `cache_key = sha256(scope || namespace || content ||
  taxonomy_version)`
  in `memory_classifier_cache`. Hit → return cached suggestion.
- Run a fixed pattern matcher (regex tables seeded per top-level root)
  against the content. A high-confidence match short-circuits the LLM
  call.

This is the "1–5ms" fast path memoir-ai documents.

### 7.3 Stage 2 — LLM classifier (with cache)

On miss, build a single LLM prompt containing:

- The query content.
- The taxonomy block (rendered from `memory_taxonomy_node`, with note
  counts and one sample per path — same shape memoir-ai uses).
- Instructions: pick one or more specific existing paths, OR propose
  one new child segment under an existing parent.

Output is structured JSON:

```json
{
  "namespace": "default",
  "paths": ["procedural.coding.tests.command"],
  "kind": "procedural",
  "lifecycle": "replacement",
  "confidence": "high",
  "trust": "repo_evidence",
  "sensitivity": "internal",
  "propose_new": null,
  "rationale": "Command preference is reusable across runs."
}
```

Multi-path results create sibling-linked notes (§6.3). The result is
cached with a TTL keyed on the taxonomy version.

The LLM provider follows libra's existing provider matrix
(`gemini` / `openai` / `anthropic` / `deepseek` / `kimi` / `zhipu` /
`ollama`). Default model is configurable via
`LIBRA_MEMORY_CLASSIFIER_MODEL`; recommendation: a small/fast model
(Haiku-class or local-small where privacy policy requires it).

### 7.4 Stage 3 — Path validation & fallback

- If `path` is invalid (depth >5, unknown root, dynamic segment in
  static slot), apply progressive shortening: drop the last segment
  until a valid prefix is found.
- If still invalid, fall back to `<root>.unsorted` (a guaranteed seed
  path) and emit a warning event.

### 7.5 Stage 4 — Conflict and trust gate

Before a note becomes prompt-visible:

1. Load confirmed notes in the same `(scope, namespace, path)` cell.
2. For `replacement` notes, check whether the new body contradicts an
   existing live body and whether either note has stronger evidence.
3. If the conflict is resolvable by lineage (`parents` contains the old
   revision), confirm the new revision and supersede the old one.
4. If both sides are plausible and neither dominates by evidence,
   create `MemoryEvent { action: Quarantined(reason) }`, add
   `contradicts` links, and exclude both from prompt injection until
   `libra memory resolve` chooses an outcome.
5. If `trust == ExternalUntrusted`, require either an `EvidenceRef` to
   the fetched source or explicit human confirmation before the note can
   leave `Draft`.

This is the point where Memoria-style quarantine and Zep-style temporal
truth handling enter Libra without requiring a graph database in the
base design.

## 8. Retrieval Pipeline

Memory exposes four retrieval modes — same factoring memoir-ai uses,
plus Libra-specific direct-get and caller-driven primitives.

### 8.1 Direct path get (no LLM)

```rust
memory.get(scope, namespace, "procedural.coding.tabs") -> Vec<MemoryNote>
memory.get_note(note_id) -> Option<MemoryNote>
memory.list_prefix(scope, namespace, "procedural.coding.") -> Vec<MemoryPathSummary>
```

O(log n) via SQLite `memory_head` and `memory_path_summary`. This is
the call agents make most often once they know the path.

### 8.2 Single-stage classifier recall (in-engine, 1 LLM call)

For a free-text query when the path is unknown but latency matters:

1. Render a compact taxonomy block from `memory_path_summary`.
2. Ask the LLM to pick up to 5 concrete paths and return structured
   JSON.
3. Direct `get` on the picked paths.

This is lower latency than tiered recall, but less robust on large
taxonomies. It backs `memory recall --mode single`.

### 8.3 Tiered drill-down (in-engine, 2–3 LLM calls)

For a free-text query when the path is unknown:

1. Build an L1 histogram from `memory_taxonomy_node.prefix_count` —
   no LLM.
2. LLM picks 1–2 L1 buckets (`procedural` vs `semantic` vs `episodic`).
3. Within each bucket, LLM picks 1–3 L2 / L3 paths from a focused list.
4. Direct `get` on the picked paths, plus their immediate children if
   `lifecycle == Accretive`.

Total budget: ≤2 LLM calls per recall. This is the default for
`memory recall`.

### 8.4 Caller-driven (no LLM inside Memory)

Expose two LLM-free primitives an outer agent can compose:

```rust
memory.summarize(scope, namespace, prefix, depth) -> Vec<MemoryPathSummary>
memory.get(scope, namespace, path) -> Vec<MemoryNote>
```

`MemoryPathSummary` carries the path, child paths, note counts,
quarantine counts, and a stable 1-sentence preview. The outer agent
(already an LLM) does the picking — and gets to use conversational
context the memory engine doesn't have. This is what `libra code`
runtime should use by default.

### 8.5 Prompt-time injection

At prompt build time, `with_memory_anchors()` (existing) is extended
into `with_memory(...)`:

1. Include the compact `codebase:onboard` or `project:onboard` summary
   for the resolved scope when it is fresh.
2. Include high-confidence, confirmed `procedural.*` and selected
   `semantic.*` heads from `default` whose scope matches the current
   branch / worktree. "Selected" means the note is short, recent enough,
   and not superseded, expired, quarantined, or secret-like.
3. For `episodic.*`, retrieve the top-K most relevant heads ranked by
   recency × use-count × tag overlap with the active task. K is small
   (5–10).
4. Inject into the prompt as budgeted `ProjectMemory` and
   `MemoryAnchor` context segments, capped at a
   configurable token ceiling (default 1.5k tokens).

The injection is rendered as a stable, prefix-cache-friendly block —
the order is deterministic and the format does not change between
turns unless a head changed.

Prompt-visible notes must show `path`, `namespace`, `scope`,
`confidence`, `trust`, and a short evidence pointer. The agent is
instructed that memory is guidance, and current source files / command
output override stale memory.

## 9. Branching and Versioning

### 9.1 Per-branch memory

`scope = Branch("main")` notes live under
`refs/libra/memory/branch/main`. Switching the user's working branch
(via `libra switch`) implicitly switches the queried scope:

```text
libra switch experiment
   -> agent reads memory from
      refs/libra/memory/branch/experiment, falling back to
      refs/libra/memory/main for Repo-scoped entries
```

This solves the "context contamination" failure mode in §2.1.

### 9.2 Memory log / diff / blame

```bash
libra memory log [path]                  # commits affecting this path
libra memory diff <rev1>..<rev2> [path]  # what changed between two memory revisions
libra memory blame <path>                # who set the current head and when
```

These are thin shims over Libra's existing `log` / `diff` / `blame`
commands, scoped to `refs/libra/memory/...`.

### 9.3 Merge and rebase

A memory merge is a normal Git merge over the memory ref. Conflict
resolution rules:

- **Same `note_id` lineage**: fast-forward to the descendant revision.
- **Replacement lifecycle, same cell, different `note_id`**: if bodies
  are compatible, keep both; if they contradict, quarantine the lower
  evidence note or both notes and require `libra memory resolve`.
  "Latest timestamp wins" is not safe enough for production memory.
- **Accretive lifecycle**: union the entries and deduplicate by
  normalized body hash + evidence hash. Both sides keep their notes
  unless one is explicitly revoked.
- **Taxonomy expansion**: merge only if parent path still exists and the
  new segment does not collide with a sibling; otherwise quarantine the
  expansion event and keep notes at their previous valid path.

### 9.4 Cherry-pick across branches

`libra memory cherry-pick <rev>` lifts a memory revision from one
branch ref to another. Useful when an experiment branch discovered a
real invariant that should land on `main`.

## 10. Lifecycle and Worthiness

### 10.1 Creation

Three entry points:

1. **Explicit** — `libra memory remember "..."` from CLI, or
   `memory_remember` from MCP / agent tool.
2. **Promoted from anchor** — at thread end, confirmed
   `MemoryAnchor`s with `MemoryAnchorScope::Project` are promoted via
   the Stop hook (§11.5).
3. **Distilled from ContextFrame** — when a Run produces a
   `ContextFrame` flagged as reusable (e.g. `kind == VerifiedFinding`),
   the agent may propose a Memory write.

In all three cases, the worthiness filter (§7.1) runs first.

### 10.2 Supersession

Writing a new revision at the same `note_id` produces a normal new
revision. Writing a new note that should **replace** an existing one
at the same path:

1. Write new `MemoryNote` (new `note_id`).
2. Append `MemoryEvent { action: Superseded(by_revision = new) }` to
   the old note.
3. Update `MemoryHead` to point at the new revision.

The old note remains queryable via `libra memory log`, never deleted.

### 10.3 Revocation

```bash
libra memory revoke <path-or-note-id> --reason "..."
```

Appends `MemoryEvent { action: Revoked(reason) }`. The head moves to
the most recent non-revoked revision, or `MemoryHead` is removed
entirely if no revision survives. Prompt injection skips revoked
heads.

### 10.4 Pruning

Pruning operates at the **projection** level only — it never rewrites
`git-internal` history. Default policy:

- `episodic.*` heads with `last_used_at` older than 90 days and
  `use_count <= 1` are pruned from `memory_head`.
- The underlying notes remain on disk and can be revived via
  `libra memory revive <path>`.

Pruning is opt-in for `procedural.*` and `semantic.*`.

### 10.5 Consolidation

Consolidation is the scheduled counterpart to memoir-ai Stop-hook
capture and OpenAI-style layout consolidation:

1. Read recent `episodic.*` notes, confirmed `MemoryAnchor`s, and
   high-signal `ContextFrame`s for a scope / namespace window.
2. Produce candidate `semantic.*` or `procedural.*` notes with compact
   bodies, evidence refs back to the source notes, and `links.supports`
   edges.
3. Mark the source episodic notes as `Consolidated`, not revoked.
4. Keep the consolidated note in `Draft` unless policy allows automatic
   confirmation.

This keeps raw episodes available for audit while preventing prompt
injection from slowly becoming a dated incident log.

### 10.6 Privacy and forgetting

Memory has two deletion-like operations with different guarantees:

- `revoke`: removes a note from the current projection and prompt
  injection, but preserves the historical body for audit.
- `forget`: for legally or policy-sensitive content, writes a tombstone
  event and replaces the prompt-visible body with a redacted placeholder
  in projections. If the underlying object store later gains encrypted
  tombstone compaction, `forget` is the API that will drive it.

`forget` requires a reason and refuses to run on evidence refs that are
needed by immutable release artifacts unless the caller chooses an
explicit `--break-evidence-link` mode.

## 11. Agent Runtime Integration

Memory hooks into the existing libra agent lifecycle
(`src/internal/ai/hooks/event.rs`). No new hook events are needed.

### 11.1 SessionStart

- Resolve scope: `(repo, current_branch, current_worktree, actor)`.
- Load the fresh `codebase:onboard` summary for git repos, or
  `project:onboard` summary for non-git folders. If stale, inject only a
  staleness hint and ask the agent to refresh when useful.
- Eagerly load a small number of confirmed `procedural.*` and
  high-confidence `semantic.*` heads under `default`.
- Warm the classifier cache.
- Emit a `MemoryEvent { action: SessionAttached }` for telemetry.

### 11.2 UserPromptSubmit (existing hook)

If the user's message looks like a directive ("from now on…",
"remember that…", "don't forget X"), pre-emptively run the worthiness
filter and, if accepted, draft a Memory write that surfaces in the
agent's tool-call space — same UX shape as memoir-ai's prompt-submit
hook but without auto-committing.

### 11.3 PreToolUse / PostToolUse

- `PreToolUse`: if the about-to-run tool has a known invariant in
  Memory (`procedural.shell.never-rm-rf-root`, etc.), surface it as an
  advisory in the tool description.
- `PostToolUse`: if the tool produced new `Evidence` flagged
  `VerifiedFinding`, run the distillation and consolidation pipeline
  (§10.1 and §10.5) on it.

### 11.4 Onboarding refresh

The memoir-ai plugin splits user facts from codebase snapshots; Libra
should do the same:

- `libra memory onboard --namespace codebase:onboard` performs a cold
  scan: top-level directories, README / AGENTS / CLAUDE files, package
  manifests, workflows, and recent commit summaries. It writes
  deterministic paths with `-p` so no LLM classifier is required.
- A warm refresh compares the current commit to the last onboarded
  commit and rewrites only affected `semantic.repo.*`,
  `procedural.repo.*`, and `episodic.commits.*` paths.
- A meta-only refresh updates `semantic.repo.onboard.last-refresh` when
  the commit has not moved.
- Non-git folders use `project:onboard` and a filesystem snapshot hash
  instead of branch / commit metadata.

### 11.5 SessionEnd / Stop

- For each confirmed `MemoryAnchor` with
  `MemoryAnchorScope::Project`, propose promotion to a Memory write.
- Run the worthiness filter on the conversation tail (last 2 turns)
  and create draft candidates for accepted facts.
- Interactive mode surfaces candidates as a "memorize?" prompt and does
  not confirm without user approval.
- Auto-mode may confirm only when all of these hold: classifier
  confidence is `High`, trust is at least `RepoEvidence`, sensitivity is
  not `Confidential` or `SecretLike`, no conflict is detected, and the
  namespace policy allows auto-confirm.

### 11.6 MemoryAnchor relationship

`MemoryAnchor` (existing in
`src/internal/ai/context_budget/memory_anchor.rs`) keeps its current
role as the **prompt-injection slot for the active thread**. Memory
becomes the **persistent backing store** that fills that slot:

- At SessionStart, `MemoryAnchor` rows are seeded from `MemoryHead`
  reads (read-side projection).
- At SessionEnd, confirmed anchors flow back into Memory writes
  (write-side promotion).

The two systems share `MemoryAnchorKind` and `MemoryAnchorConfidence`
where possible. Memory's review state extends the existing
`MemoryAnchorReviewState` vocabulary with `Quarantined`; the anchor
layer can continue to treat quarantined rows as non-active until a later
refactor folds anchors into a thin Memory projection.

### 11.7 Prompt budget

Hard ceiling on Memory's prompt slot: configurable via
`LIBRA_MEMORY_PROMPT_BUDGET_TOKENS` (default 1500). When the budget
overflows, the injector drops in this order:

1. Expired notes and stale onboarding hints.
2. Low-confidence semantic / procedural notes.
3. Older episodic findings not tied to the active task.
4. Medium-confidence semantic facts.
5. High-confidence procedural rules are retained last unless they are
   longer than the entire budget; then they are replaced by their path
   summary and a direct-get hint.

Drops are logged to a `MemoryEvent { action: PromptTrimmed }` so the
behaviour is auditable.

## 12. CLI Surface

```text
libra memory remember <text> [-n <namespace>] [-p <path>] [--scope <s>] [--confidence <c>]
libra memory recall <query> [-n <namespace>] [--mode {direct|single|tiered|caller}] [--limit N]
libra memory summarize [-n <namespace>] [--prefix <p>] [--depth N]
libra memory get <path> [-n <namespace>]
libra memory get-note <note-id>
libra memory list [--prefix <p>] [-n <namespace>] [--scope <s>]
libra memory confirm <path-or-id> [--reason <r>]
libra memory quarantine <path-or-id> --reason <r>
libra memory resolve <path> --choose <note-id> --reason <r>
libra memory revoke <path-or-id> --reason <r>
libra memory forget <path-or-id> --reason <r> [--break-evidence-link]
libra memory revise <path> <text>            # writes new revision at same note_id
libra memory move <old-path> <new-path>      # supersede + new write
libra memory onboard [--namespace codebase:onboard|project:onboard] [--force]
libra memory log [<path>]
libra memory diff <rev1>..<rev2> [<path>]
libra memory blame <path>
libra memory branches                        # list memory refs
libra memory rebuild                         # rebuild SQLite projections from refs
libra memory show-taxonomy [--root <r>]
libra memory expand <parent-path> <new-segment> --rationale <r>
libra memory prune [--policy <p>] [--dry-run]
libra memory revive <path>
libra memory inspect-injection [--last-run|--current]
```

Conventions:

- `--scope` accepts `branch`, `repo`, `worktree`, `actor:<ref>`,
  `global`. Default is `branch:<current-head>`.
- `-n / --namespace` defaults to `default`; recall may search multiple
  namespaces only when the caller passes `--all-namespaces`.
- All write commands respect `--dry-run` and `--json` for scripting.
- `libra memory recall` defaults to `tiered` mode.

## 13. MCP Surface

Add to `src/internal/ai/mcp/`:

| Tool | Purpose |
|---|---|
| `memory_remember` | Write a memory; runs worthiness + classification pipeline |
| `memory_recall` | Free-text recall; supports `mode` parameter |
| `memory_get` | Direct path lookup |
| `memory_get_note` | Direct note lookup by `note_id` |
| `memory_list_prefix` | Cheap prefix listing for caller-driven retrieval |
| `memory_summarize` | LLM-free summary (path, child paths, note counts, preview) |
| `memory_confirm` | Confirm a draft or quarantined note |
| `memory_resolve` | Resolve a path conflict |
| `memory_revoke` | Revoke a path or note |
| `memory_forget` | Redact prompt-visible content for policy-sensitive notes |
| `memory_log` | History of a path |
| `memory_onboard` | Populate or refresh `codebase:onboard` / `project:onboard` |
| `memory_propose_taxonomy` | Propose a new taxonomy segment (returns event id; needs confirmation) |

Each tool maps 1:1 to a CLI command, so a Claude Desktop session and
a `libra memory ...` shell user see the same surface.

## 14. Database Schema Additions

Append to `sql/sqlite_20260309_init.sql`:

- `memory_head` (§5.2)
- `memory_path_summary` (§5.2)
- `memory_note_index` (§5.2)
- `memory_link_index` (§5.2)
- `memory_taxonomy_node` (§5.2)
- `memory_classifier_cache` (§5.2, optional, TTL'd)

All projection tables tolerate being dropped + rebuilt
from `refs/libra/memory/...` via `libra memory rebuild`.

## 15. Phased Roadmap

### Phase A — Auditable Store (1–2 weeks)

- `MemoryNote` / `MemoryEvent` Rust types in
  `src/internal/ai/memory/`.
- `git-internal` ref `refs/libra/memory/main` writes / reads.
- SQLite projections (§5.2) + Sea-ORM entities:
  `memory_head`, `memory_path_summary`, `memory_note_index`,
  `memory_link_index`, `memory_taxonomy_node`.
- `libra memory remember / get / get-note / list / summarize / log /
  blame / rebuild`.
- No classifier — caller must supply `namespace` and `path`.

Exit gate: path aggregation works; projection rebuild is byte-for-byte
stable; branch switches do not leak memory across refs.

### Phase B — Intake Safety (1–2 weeks)

- Worthiness filter, secret / PII redaction, sensitivity and trust
  classification.
- Review states: `Draft`, `Confirmed`, `Quarantined`, `Revoked`,
  `Superseded`.
- Conflict detection and `libra memory resolve`.
- `forget` API with redacted projection semantics.

Exit gate: no unreviewed, secret-like, external-untrusted, or
quarantined note can enter prompt injection.

### Phase C — Classification, Recall, and Injection (2–3 weeks)

- Pattern + LLM classifier with cache and multi-path sibling links.
- Direct, single-stage, tiered, and caller-driven retrieval.
- MCP tools (§13).
- Prompt-time injection (§11.7) using `ProjectMemory` and
  `MemoryAnchor` budget segments.
- `libra memory inspect-injection` for observability.

Exit gate: recall has per-stage timing metadata, deterministic
fallbacks, and test fixtures for malformed LLM output.

### Phase D — Onboarding, Consolidation, and Branch Ops (2 weeks)

- `codebase:onboard` and `project:onboard` cold / warm / meta-only
  refresh.
- SessionEnd draft capture and scheduled consolidation (§10.5).
- Per-branch / per-worktree refs (§9.1).
- Memory merge / cherry-pick conflict resolution (§9.3).

Exit gate: a feature branch can carry its own onboarding and memories,
then explicitly cherry-pick or merge selected notes into `main`.

### Phase E — UI and Optional Indexes (later)

- Web UI in `libra code` for browsing taxonomy, path summaries, links,
  quarantines, prompt injections, and diffs.
- Optional embedding index for ANN recall.
- Optional temporal/entity graph projection for Zep-style multi-hop
  questions.
- Plugin parity with memoir-ai's Claude Code plugin (slash commands,
  statusline, UI launch) where it makes sense for Libra.

## 16. Verification Plan

Ship Memory only with targeted regression coverage:

- Projection rebuild: write notes/events, drop all memory projection
  tables, rebuild, and assert identical summaries, heads, and links.
- Branch contamination: create conflicting branch-scoped memories,
  switch branches, and assert prompt injection changes with the branch.
- Aggregation: store multiple notes under one path and assert
  `get(path)` returns all confirmed notes while `get-note(id)` returns
  exactly one.
- Quarantine: introduce contradictory replacement notes and assert both
  recall and prompt injection exclude unresolved conflicts.
- Privacy: feed token-like and private-key-like strings through intake
  and assert redacted storage / no prompt injection.
- Retrieval robustness: malformed LLM output, `NONE`, unknown mode, and
  empty result cases must fail loud or return timing-only observability,
  never silently inject arbitrary memory.
- Prompt budget: overfill memory and assert high-confidence procedural
  rules survive longer than low-confidence or stale notes.
- Onboarding: cold, warm, and meta-only refresh produce deterministic
  paths and do not rewrite unrelated namespaces.

## 17. Open Questions

1. **Cross-worktree visibility.** `libra worktree`-spawned linked
   worktrees today share `.libra/`. Should they also share memory? The
   current proposal: yes for `Repo`-scoped, no for `Worktree`-scoped.
   Needs design alignment with
   [`libra-worktree-architecture.md`](./libra-worktree-architecture.md).
2. **Encryption.** `procedural.review.merge-policy` may capture
   sensitive policy. The base proposal stores blobs in cleartext on
   disk like every other Libra object, leaning on filesystem
   permissions. A later option: integrate with the existing
   `LIBRA_STORAGE_*` envelope encryption pipeline.
3. **LFS for large memories.** Long episodic findings (e.g. an
   incident postmortem stub) may exceed reasonable inline size. Reuse
   Libra's existing LFS plumbing (`lfs_structs.rs`,
   `protocol/lfs_client.rs`) when a memory body crosses
   `LIBRA_STORAGE_THRESHOLD`.
4. **Embedding index extension.** memoir-ai consciously avoids vectors
   in its core; this design follows suit. A later optional
   `memory.embed` extension can layer ANN search **on top of**
   path-keyed retrieval, never replacing it.
5. **Memory federation across repos.** Out of scope for now. A user
   working on multiple repos still gets one memory store per repo.
6. **Prompt-injection observability.** Phase B should ship with a
   `libra code --debug-memory` flag that prints the injected memory
   slot verbatim each turn, so humans can see exactly what the agent
   "remembers" before each tool call.

## Summary Rule

```text
1. Hierarchical paths replace flat blobs.        (memoir-ai)
2. Namespaces separate user facts, onboarding,
   metrics, and private actor memory.             (memoir-ai + Libra)
3. Snapshot stores "what the memory is".          (libra)
4. Event stores "what happened to the memory".    (libra)
5. Projection stores "what is current".           (libra)
6. Draft, quarantine, trust, and sensitivity
   gates decide what can enter the prompt.         (Libra safety)
7. git-internal is the historical truth — refs,
   commits, blame, diff, revert all work
   unchanged on memory.                           (libra-native)
```

The agent's memory is now a versioned, branched, auditable artifact
of the repository — same as the code.
