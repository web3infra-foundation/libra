# Libra Worktree Architecture vs Traditional Git Worktree

This document compares Libra's current Worktree mechanisms with
traditional Git worktrees. The focus is the Agent execution path under
`docs/agent`: task-local isolated worktrees that let Libra run code
generation and validation without letting intermediate state leak into the
primary workspace.

Libra has two related but different worktree mechanisms:

- **User-facing linked worktrees** from `libra worktree`, implemented in
  [`src/command/worktree.rs`](../../src/command/worktree.rs), with optional
  FUSE support in
  [`src/command/worktree-fuse.rs`](../../src/command/worktree-fuse.rs).
- **Agent task worktrees**, implemented in
  [`src/internal/ai/orchestrator/workspace.rs`](../../src/internal/ai/orchestrator/workspace.rs)
  and exposed through
  [`src/internal/ai/runtime/environment.rs`](../../src/internal/ai/runtime/environment.rs).

The Agent mechanism is the higher-level architecture. It treats a
worktree as an execution environment owned by Libra's scheduler, not as a
long-lived branch checkout owned by the VCS.

## Current Libra Worktree Model

### Repository-Linked Worktrees

`libra worktree` manages persistent linked working trees that share one
repository storage directory.

```text
+--------------------------+        +--------------------------+
| main workspace           |        | linked workspace         |
| /repo                    |        | /repo-feature            |
|                          |        |                          |
| .libra/                  |<-------| .libra -> /repo/.libra   |
| worktrees.json           |        | files restored from HEAD |
| SQLite DB / object store |        |                          |
+--------------------------+        +--------------------------+
```

Important properties:

- The shared `.libra` directory contains the SQLite database, object
  store, configuration, and `worktrees.json`.
- Each linked worktree contains a `.libra` symlink back to the shared
  storage directory.
- `worktrees.json` stores canonical paths, the main-worktree marker, lock
  state, and optional lock reasons.
- State writes are atomic: Libra writes a temporary JSON file and renames
  it into place.
- `libra worktree add` creates an empty linked directory and, when `HEAD`
  exists, restores committed `HEAD` content into that directory. It does
  not copy staged-only index state.
- `libra worktree remove` unregisters the worktree but intentionally does
  not delete the directory, reducing accidental data-loss risk.
- `libra worktree repair` deduplicates registry entries and restores the
  invariant that exactly one entry is the main worktree.

With the `worktree-fuse` feature, Libra can also maintain
`worktrees-fuse.json` plus a per-worktree upper directory under
`.libra/worktrees-fuse/`. The target path is mounted as a FUSE overlay:
the current workspace is the lower layer and the per-worktree upper
directory stores writes. This mode can select or create a branch during
worktree creation, while still using Libra-managed metadata.

### Agent Task Worktrees

Agent task worktrees are ephemeral. Libra creates one for a task attempt,
runs tools inside it, syncs successful implementation changes back through
a contract-aware replay step, and then cleans it up.

```text
primary workspace
      |
      | snapshot_workspace()
      v
+------------------------------+
| TaskWorktree baseline        |
| - file hashes                |
| - symlink targets            |
| - gitignore-aware traversal  |
| - protected metadata skipped |
+------------------------------+
      |
      v
+--------------------------------------------------------------+
| isolated task workspace                                      |
|                                                              |
| FUSE backend when available:                                 |
|   workspace/  = mounted overlay                              |
|   lower/      = materialized baseline                        |
|   upper/      = writes + .libra symlink                      |
|                                                              |
| Copy backend fallback:                                       |
|   workspace/  = materialized baseline + .libra symlink       |
|   file copies prefer CoW clonefile/FICLONE, then copy        |
+--------------------------------------------------------------+
      |
      | task tools run here
      v
+------------------------------+
| sync_task_worktree_back()    |
| - diff against baseline      |
| - enforce touchFiles/scope   |
| - reject concurrent changes  |
| - copy/delete changed paths  |
+------------------------------+
      |
      v
primary workspace updated only after successful replay
```

Provisioning steps:

1. Libra snapshots the primary workspace with
   `snapshot_workspace()`. The traversal respects `.gitignore`,
   preserves symlink entries without following them, and skips protected
   metadata directories such as `.git`, `.libra`, `.codex`, and
   `.agents`.
2. Libra allocates a temporary root named with the backend, process id,
   and task UUID.
3. On Unix with an active Tokio runtime, Libra first tries a FUSE overlay
   backend. It materializes the baseline into `lower/`, links shared
   `.libra` storage into `upper/`, mounts `workspace/`, and runs a health
   check.
4. If FUSE is unavailable or unhealthy, Libra falls back to a copy
   backend. It links `.libra` into `workspace/`, materializes the
   snapshot there, and uses platform copy-on-write clone operations where
   possible.
5. For implementation tasks, tool registries and hook runners are rebound
   to the isolated working directory before the task runs.

Execution and replay rules:

- Implementation tasks run inside a task worktree. Their changes are
  synced back only if the task completes successfully.
- Gate tasks also run inside isolated worktrees, but their output is
  discarded after the check. This keeps verification scratch files out of
  the main workspace.
- Sync-back computes changed paths by comparing the task snapshot with the
  captured baseline.
- Before replaying a changed path, Libra enforces the task write contract:
  `touchFiles` takes priority when present, otherwise `scope_in` and
  `scope_out` define the allowed write area.
- Libra checks that the corresponding path in the primary workspace still
  matches the baseline. If the user or another task changed it
  concurrently, sync-back fails instead of overwriting it.
- A scheduler-level mutex serializes sync-back, so parallel task worktrees
  can run concurrently but integrate changes one at a time.
- Cleanup unmounts FUSE worktrees when needed and removes the temporary
  root.

## Traditional Git Worktree Model

Git worktrees are persistent checkouts attached to one Git repository.
They are branch/ref oriented rather than task oriented.

```text
main checkout
      |
      | common Git directory
      v
.git/
  objects/
  refs/
  worktrees/
    feature/
      HEAD
      index
      gitdir
      commondir

linked checkout
  .git  -> text file: "gitdir: /repo/.git/worktrees/feature"
  files checked out for that worktree's HEAD
```

Important properties:

- Each linked worktree has a `.git` file pointing to a per-worktree admin
  directory under the common `.git/worktrees/` area.
- The per-worktree admin directory stores worktree-local state such as
  `HEAD`, `index`, and the pointer back to the common Git directory.
- Object storage and most refs are shared through the common directory.
- Git prevents the same branch from being checked out in multiple
  worktrees by default.
- `git worktree add` usually creates or checks out a branch, or creates a
  detached worktree at a commit.
- `git worktree remove` deletes the linked working tree by default after
  safety checks.
- `git worktree prune`, lock files, and repair operations manage stale
  metadata and missing directories.

This design is excellent for human branch-based workflows. It is less
directly aligned with Agent execution because Git worktrees do not encode
task contracts, scheduler state, audit events, or safe replay semantics.

## Architecture Differences

| Area | Traditional Git Worktree | Libra Worktree |
|---|---|---|
| Primary owner | Git ref and checkout machinery | Libra scheduler and execution environment |
| Main unit of isolation | Branch, detached commit, and per-worktree index | Task attempt, baseline snapshot, and write contract |
| Metadata layout | Scattered filesystem control files under `.git/worktrees/<id>` plus `.git` pointer files | Human/agent-readable JSON for persistent worktrees; ephemeral task state for Agent worktrees |
| Repository storage link | `.git` file points to a per-worktree Git admin directory; `commondir` points back to common storage | `.libra` symlink points directly to shared Libra storage; task FUSE backend links storage into the writable upper layer |
| Starting content | Git checkout from a branch or commit into a worktree and index | CLI linked worktree restores `HEAD`; Agent worktree snapshots current workspace state, including uncommitted files that are not ignored |
| Parallel work | Multiple persistent branch checkouts | Multiple ephemeral task attempts can run in parallel without allocating branches |
| Integration | User runs merge, rebase, cherry-pick, or manual copy | Libra syncs successful task changes back after scope and concurrency checks |
| Failure behavior | Failed experiments leave a persistent worktree until removed | Failed Agent tasks are discarded; the primary workspace remains unchanged |
| Validation scratch space | Commands can dirty the checkout unless manually isolated | Gate tasks run in throwaway worktrees, so scratch files do not leak |
| Safety boundary | Git protects branch checkout conflicts and some uncommitted states | Libra protects declared write scope, concurrent main-workspace edits, ignored metadata, and non-destructive removal |
| Auditability | Git records commits and reflogs after user actions | Libra records task/runtime events, tool calls, evidence, and patch artifacts around the isolated execution |

## Why Libra's Design Fits Agent Execution Better

### Task-First Isolation

An Agent task needs an isolated filesystem, not necessarily a new branch.
Libra can run several implementation tasks from the same baseline in
parallel, then replay only the successful, in-scope edits. Git worktrees
make branch checkout the central abstraction, which adds branch management
overhead even when the real goal is a short-lived task sandbox.

### Contract-Aware Replay

Git worktrees isolate directories, but they do not know what a task was
allowed to change. Libra carries the task contract into sync-back:
`touchFiles`, `scope_in`, and `scope_out` are enforced after execution and
before the primary workspace is modified. This turns "the agent changed a
directory" into "the agent changed exactly the allowed paths and the main
workspace still matches the baseline."

### Clean Failure Semantics

Libra does not let failed implementation attempts or gate scratch files
pollute the user's checkout. A task's worktree is temporary; if the task
fails, cleanup removes the worktree and no sync-back occurs. This makes
retry and replan loops much cheaper operationally than persistent
branch-per-attempt workflows.

### Concurrency Without Silent Overwrite

Parallel tasks can execute independently. Integration is serialized and
checks the primary workspace against the captured baseline path by path.
If another task or the user changed the same path, Libra fails the replay
instead of silently overwriting the newer state.

### Performance-Oriented Materialization

Libra uses the cheapest available isolation mechanism:

- FUSE overlay on Unix when the runtime and mount are healthy.
- Copy-on-write cloning through `clonefile` on macOS or `FICLONE` on
  Linux when using the copy backend.
- Normal file copy as the final fallback.

The snapshot traversal also respects ignore files, so generated build
outputs such as `target/` or `node_modules/` are not copied into task
worktrees when ignored.

### Shared Repo Capabilities Without Copying Storage

Task worktrees keep `.libra` visible by linking to shared repository
storage. Tools that need repository metadata can run in the isolated
workspace without duplicating the database or object store. At the same
time, metadata directories are excluded from the content snapshot so
execution diffs focus on project files.

### Safer Persistent Worktree UX

For long-lived user-facing worktrees, Libra keeps the registry in one
inspectable `worktrees.json` file and avoids destructive default removal.
This is intentionally more conservative than Git's directory-deleting
`worktree remove` behavior and is friendlier to AI-assisted workflows
where uncommitted work may exist in a linked checkout.

## Current Trade-Offs

Libra's design is optimized for Agent execution, so it intentionally does
not match every Git worktree behavior:

- The non-FUSE `libra worktree add` path does not allocate a branch per
  worktree. It creates a linked directory restored from `HEAD`.
- FUSE-backed user worktrees are optional and platform/runtime dependent.
  If the mount path is unavailable, Agent task worktrees fall back to the
  copy backend.
- Agent task worktrees are ephemeral. They are not a replacement for
  long-lived human checkouts.
- Sync-back is a conservative file-level replay. It rejects concurrent
  changes instead of attempting a semantic merge.
- Because task worktrees link shared `.libra` storage, scheduler policy
  and prompts must continue to keep normal coder tasks from performing
  unintended version-control mutations.

These trade-offs are deliberate. Traditional Git worktrees optimize for
human branch checkouts. Libra Worktrees optimize for reproducible,
contract-bound, auditable Agent execution.
