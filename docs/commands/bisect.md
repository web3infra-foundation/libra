# `libra bisect`

Use binary search to find the commit that introduced a bug.

## Synopsis

```
libra bisect start [<bad> [<good>...]] [--good <commit>]
libra bisect bad [<rev>]
libra bisect good [<rev>]
libra bisect reset [<rev>]
libra bisect skip [<rev>]
libra bisect log
libra bisect run <cmd> [<args>...]
libra bisect view
```

## Description

`libra bisect` performs a binary search through the commit history to find the specific commit that introduced a regression or bug. The user marks commits as "good" (working correctly) or "bad" (containing the bug), and bisect systematically checks out commits in between until the first bad commit is identified.

A bisect session begins with `bisect start`, which saves the current HEAD and branch so they can be restored later. The user then marks the boundaries: a "bad" commit (where the bug exists) and one or more "good" commits (where the bug does not exist). Bisect calculates the midpoint between good and bad in the commit graph using BFS traversal, checks out that commit, and waits for the user to test and mark it. This process repeats, halving the search space each time, until the culprit commit is found.

Bisect state is persisted in a `bisect_state` table in the SQLite database, making sessions survive process restarts. The state tracks the original HEAD, the bad commit, all good commits, skipped commits, the current test commit, estimated remaining steps, and whether the session has completed.

When bisect identifies the culprit, it prints the commit details and marks the session as completed. The user must then run `bisect reset` to end the session and restore HEAD to its original position.

## Options

### Subcommand: `start`

Begin a new bisect session. Saves the current HEAD and branch for later restoration.

| Argument / Flag | Description |
|-----------------|-------------|
| `[<rev>...]` | Positional bounds: the first rev is the bad commit, any remaining revs are good commits (matches `git bisect start <bad> <good>...`). If omitted, mark them later with `bisect bad` / `bisect good`. |
| `--good` / `-g` | Optional good commit, appended after any positional good commits (kept as a compatibility alias). |

```bash
# Start with no initial markers
libra bisect start

# Start with known bad (current HEAD) and good commit
libra bisect start HEAD --good v1.0

# Start with a specific bad commit
libra bisect start abc1234 --good def5678

# Start with one bad and multiple good commits (positional)
libra bisect start abc1234 def5678 0123abc
```

> `bisect start` does not support `--` pathspec limiting (path-limited bisect is
> unsupported). `libra bisect start <bad> -- <pathspec>` is rejected with a usage
> error (`LBR-CLI-002`, exit 129) rather than silently treating the pathspec as a
> good commit.

### Subcommand: `bad`

Mark the current or given commit as bad (contains the bug). If both good and bad commits are known, bisect immediately calculates the next midpoint and checks it out.

| Argument | Description |
|----------|-------------|
| `<rev>` | Commit to mark as bad. Defaults to the current HEAD. |

```bash
# Mark current commit as bad
libra bisect bad

# Mark a specific commit as bad
libra bisect bad abc1234
```

### Subcommand: `good`

Mark the current or given commit as good (does not contain the bug). If both good and bad commits are known, bisect calculates the next midpoint and checks it out.

| Argument | Description |
|----------|-------------|
| `<rev>` | Commit to mark as good. Defaults to the current HEAD. |

```bash
# Mark current commit as good
libra bisect good

# Mark a specific commit as good
libra bisect good def5678
```

### Subcommand: `reset`

End the bisect session and restore HEAD to its original position (the branch or commit that was checked out before `bisect start`). If a `<rev>` is provided, HEAD is restored to that commit instead of the original.

| Argument | Description |
|----------|-------------|
| `<rev>` | Optional commit to reset to instead of the original HEAD. |

```bash
# End bisect and restore original HEAD
libra bisect reset

# End bisect and go to a specific commit
libra bisect reset main
```

### Subcommand: `skip`

Skip the current commit and move to the next candidate. Useful when the current commit cannot be tested (e.g., it does not compile). Skipped commits are excluded from future midpoint calculations. If too many commits are skipped, bisect may not be able to narrow down the culprit precisely.

| Argument | Description |
|----------|-------------|
| `<rev>` | Commit to skip. Defaults to the current HEAD. |

```bash
# Skip the current commit
libra bisect skip

# Skip a specific commit
libra bisect skip abc1234
```

### Subcommand: `log`

Show the bisect log, displaying all good, bad, and skipped marks made during the current session.

```bash
libra bisect log
```

### Subcommand: `run`

Run a command at each bisect step and dispatch `good` / `bad` / `skip` automatically based on its exit code. The command is invoked at each candidate commit and bisect advances until convergence (or until candidates are exhausted).

`bisect run` requires an active session that already has both a bad bound and at least one good bound, so start it with `libra bisect start <bad> --good <good>` or mark both bounds manually before invoking automation.

| Argument | Description |
|----------|-------------|
| `<cmd> [<args>...]` | The command to execute. The first token is the executable; everything after is forwarded verbatim. `--` is allowed and pass-through (e.g. `libra bisect run cargo test -- --ignored`). |

Exit-code semantics (aligned with stock `git bisect run`):

| Exit code | Mark / Action |
|-----------|---------------|
| `0` | `good` |
| `1`–`124`, `126`–`127` | `bad` |
| `125` | `skip` (cannot test this commit) |
| `128` and above | Terminate the bisect with a fatal `BISECT_RUN_FAILED` error |

Killed by signal also terminates the bisect with a fatal error.

```bash
# Drive bisect with a cargo test
libra bisect run cargo test --test foo

# Pass flags through to the underlying test command
libra bisect run cargo test -- --ignored

# Use a custom shell script
libra bisect run bash -c 'cargo build && ./target/debug/repro'
```

### Subcommand: `view`

Show the current bisect state — good / bad boundaries, current HEAD, remaining candidates, and any skipped commits.

```bash
libra bisect view
```

If no bisect is in progress, returns a fatal error (`NOT_IN_BISECT`).

## JSON / Machine Output

`libra bisect` supports global `--json` and `--machine` for all subcommands.
Both modes emit a single `bisect` command envelope on success; `--machine`
uses the same envelope as one compact line and suppresses human progress.

Common fields:

| Field | Description |
|-------|-------------|
| `action` | One of `start`, `mark`, `skip`, `reset`, `log`, `view`, `run`. |
| `status` | Present for state transitions: `started`, `waiting_for_good`, `waiting_for_bad`, `testing`, `converged`, or `all_skipped`. |
| `bad` / `good` / `current` | Full commit IDs for the current bisect bounds and candidate. |
| `remaining` / `steps` | Candidate count and estimated remaining search steps when known. |
| `first_bad` | Full commit ID when the session converged. |

Example:

```json
{
  "ok": true,
  "command": "bisect",
  "data": {
    "action": "view",
    "head": "901abcd...",
    "good": ["abc1234..."],
    "bad": "def5678...",
    "current": "901abcd...",
    "remaining": 1,
    "completed": false
  }
}
```

## Common Commands

```bash
# Start a bisect session
libra bisect start

# Mark the current version as broken
libra bisect bad

# Mark a known-good version
libra bisect good v1.0

# Test the checked-out commit, then mark it
# (run your tests here)
libra bisect good    # if tests pass
libra bisect bad     # if tests fail

# Skip an untestable commit
libra bisect skip

# View the bisect log
libra bisect log

# End the session
libra bisect reset

# Quick start with known boundaries
libra bisect start HEAD --good abc1234

# Start with one bad and multiple good commits (positional)
libra bisect start HEAD abc1234 def5678
```

## Human Output

**`bisect start`**:

```text
Bisect session started.
```

**`bisect start <bad> --good <good>`** (with both markers):

```text
Bisect session started.
Bisecting: N revisions left to test (roughly M steps)
[abc1234] commit message here
```

**`bisect bad`** / **`bisect good`** (narrowing down):

```text
Bisecting: N revisions left to test (roughly M steps)
[abc1234] commit message here
```

**`bisect bad`** / **`bisect good`** (culprit found):

```text
abc1234def5678901234567890abcdef12345678 is the first bad commit
commit abc1234def5678901234567890abcdef12345678
Author: Alice <alice@example.com>
Date:   Mon Jan 15 10:30:00 2024 -0800

    introduce the bug here
```

**`bisect skip`**:

```text
Bisecting: N revisions left to test (roughly M steps)
[def5678] next candidate commit message
```

**`bisect log`**:

```text
# bad: [abc1234] broken commit message
# good: [def5678] working commit message
# skip: [ghi9012] untestable commit
```

**`bisect reset`**:

```text
Bisect session reset. HEAD restored to original position.
```

**`bisect run`** (converging):

```text
Bisecting: 5 candidates remaining
Running cargo test --test foo at abc1234... PASS (good)
Bisecting: 2 candidates remaining
Running cargo test --test foo at def5678... FAIL (bad)
Bisecting: 1 candidate remaining
Running cargo test --test foo at 901abcd... FAIL (bad)
Converged: first bad commit is 901abcd
3 steps, 0 skipped
```

**`bisect view`**:

```text
Bisecting between abc1234 (good) and def5678 (bad)
HEAD: 901abcd
Remaining: 1 candidate
Skipped: (none)
```

## Design Rationale

### Why is bisect not hidden?

Despite being listed as a hidden command in some early designs, `libra bisect` is a fully visible subcommand. Binary search for regressions is a fundamental debugging workflow that benefits both human users and AI agents. Hiding it would reduce discoverability without meaningful benefit. The command is stable and follows the same patterns as other Libra commands.

### How does `bisect run` handle exit codes?

`bisect run` mirrors stock `git bisect run` to keep AI-agent and CI integration straightforward. The exit-code contract is:

- `0` → mark `good` and advance.
- `1`–`124` or `126`–`127` → mark `bad` and advance.
- `125` → `skip` (the commit cannot be tested — e.g. it does not compile) and advance.
- `128` and above → fatal: terminate the bisect and surface `BISECT_RUN_FAILED` so the caller can react. Killed by signal (e.g. SIGINT) is treated the same way.

The full command line is passed through verbatim, so `libra bisect run cargo test -- --ignored` forwards `--ignored` to the test command rather than parsing it as a `bisect` flag. This is enabled by `trailing_var_arg` + `allow_hyphen_values` on the `cmd` argument.

Manual marking (`bisect good` / `bisect bad`) remains the recommended path for AI agents that evaluate results in-process and prefer explicit control over each step.

### Why no `--first-parent`?

Git's `git bisect --first-parent` restricts the search to first-parent commits only, which is useful in workflows with many merge commits. Libra's bisect traverses the full commit graph using BFS, which is simpler and correct for all topologies. First-parent restriction is primarily useful for large projects with a strict merge-commit workflow; Libra's target use case of trunk-based development typically has a more linear history where this optimization is unnecessary.

### Why SQLite state persistence?

Bisect sessions can span hours or days as the user tests each candidate. Storing state in the SQLite `bisect_state` table ensures the session survives process restarts, editor closes, and system reboots. Git uses flat files in `.git/BISECT_*`, which achieves the same persistence but with less structure. SQLite provides transactional writes and the ability to query state programmatically, which is valuable for AI agent integration.

### Why does `reset` accept an optional `<rev>`?

Sometimes the user wants to end the bisect session but go to a different commit than where they started. For example, after finding the culprit, they might want to reset to the commit just before the bug was introduced. The optional `<rev>` parameter provides this flexibility without requiring a separate `checkout` after reset.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Start session | `bisect start [<bad> [<good>...]] [--good <commit>]` | `bisect start [<bad> [<good>...]]` | N/A |
| Mark bad | `bisect bad [<rev>]` | `bisect bad [<rev>]` | N/A |
| Mark good | `bisect good [<rev>]` | `bisect good [<rev>]` | N/A |
| Reset | `bisect reset [<rev>]` | `bisect reset [<commit>]` | N/A |
| Skip | `bisect skip [<rev>]` | `bisect skip [<rev>...]` | N/A |
| Show log | `bisect log` | `bisect log` | N/A |
| Automated run | `bisect run <cmd> [<args>...]` | `bisect run <script>` | N/A |
| Show current state | `bisect view` | `bisect visualize` (GUI / log) | N/A |
| Visualize (GUI) | Not supported | `bisect visualize` | N/A |
| First-parent only | Not supported | `--first-parent` | N/A |
| Multiple good commits | Positional args to `start` (or repeated `bisect good`) | Positional args to `start` | N/A |
| Custom terms (`terms`/`--term-*`) | Declined (`LBR-CLI-002`, exit 129) — see declined.md D7 | `bisect terms` / `--term-{old,new}` | N/A |
| Replay session from log | Declined (`LBR-CLI-002`, exit 129) — see declined.md D6 | `bisect replay <logfile>` | N/A |
| Path-limited bisect | Rejected (`LBR-CLI-002`, exit 129) | `bisect start -- <pathspec>` | N/A |
| State storage | SQLite (`bisect_state` table) | Flat files (`.git/BISECT_*`) | N/A |

Note: jj does not have a bisect command. Users who need binary search debugging with jj must use external tooling or manually check out commits. This is a gap in jj's feature set that Libra addresses.

## Declined subcommands

The following `git bisect` surface is intentionally not implemented. These are not
registered subcommands/flags, so they are rejected by the argument parser as an
unrecognized subcommand / unexpected argument and mapped to `LBR-CLI-002` (default
coarse exit code **129**; `2` under `LIBRA_FINE_EXIT_CODES=1`):

| Declined | Why | Alternative |
|----------|-----|-------------|
| `bisect terms` / `--term-old` / `--term-new` | Custom good/bad aliases (e.g. fast/slow) are workflow personalisation, not core locating — see [declined.md D7](../improvement/compatibility/declined.md#d7-bisect-terms). | Use the default `good`/`bad` vocabulary; wrap custom semantics in your own script. |
| `bisect replay <logfile>` | Replaying a session from a `bisect log` dump has limited CI value and is reassessed once `bisect log` output stabilises — see [declined.md D6](../improvement/compatibility/declined.md#d6-bisect-replay). | Re-issue the `good`/`bad` sequence from `bisect log` manually. |
| `bisect start -- <pathspec>` | Path-limited bisect is a low-priority topology feature. Because `start` collects variadic positional revs, a raw `--` separator is rejected after clap confirms `bisect start`, so the pathspec is never silently treated as a good commit. | Filter paths in a wrapper script, then bisect normally. |

`bisect next` and `bisect visualize` are also not separate subcommands: candidate
advancement is built into `bad`/`good`/`run`, and `bisect view` is the
intentionally-different equivalent of `visualize`.

## Error Handling

Libra maps clap parse errors and bisect runtime errors through the shared
`CliError` layer. Default exits use Git-style coarse categories; setting
`LIBRA_FINE_EXIT_CODES=1` switches to the stable code's fine category.

| Scenario | Stable code | Default exit | Fine exit | Notes |
|----------|-------------|--------------|-----------|-------|
| Success, including a completed `bisect run` | — | 0 | 0 | The script's `0` / `125` / `1..=127` values are interpreted as good / skip / bad, not returned directly. |
| Bare `libra bisect` with no subcommand | — | 0 | 0 | Clap prints help and returns success. |
| Unknown subcommand (`terms`, `replay`) or unknown term flag (`--term-old`, `--term-new`) | `LBR-CLI-002` | 129 | 2 | Parser rejection; stderr names the unrecognized subcommand or unexpected argument. |
| Unsupported path-limited start (`bisect start <bad> -- <pathspec>`) | `LBR-CLI-002` | 129 | 2 | Rejected after parse so a pathspec cannot be mistaken for a good revision. |
| Invalid revision / invalid bisect range | `LBR-CLI-003` | 129 | 2 | The error names the bad argument or range. |
| Dirty tree, empty repository, missing/corrupt run state, missing HEAD, or candidate-count failure | `LBR-REPO-003` | 128 | 3 | Fatal repository-state error with a reset/stash hint where applicable. |
| `bisect view` / `bisect run` without an active session | `LBR-BISECT-001` | 128 | 3 | Start a session before calling the subcommand. |
| `bisect run` script exits `128+` or is killed by a signal | `LBR-BISECT-002` | 128 | 8 | The bisect state is preserved for inspection. |
| `bisect run` has no remaining candidate commits | `LBR-BISECT-003` | 128 | 3 | Inspect with `libra bisect view` or reset the session. |
| Bisect state database read/write failure | `LBR-IO-001` / `LBR-IO-002` | 128 | 5 | Check repository database health and filesystem permissions. |
