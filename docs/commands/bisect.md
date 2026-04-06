# `libra bisect`

Use binary search to find the commit that introduced a bug.

## Synopsis

```
libra bisect start [<bad>] [--good <commit>]
libra bisect bad [<rev>]
libra bisect good [<rev>]
libra bisect reset [<rev>]
libra bisect skip [<rev>]
libra bisect log
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
| `<bad>` | Optional commit to immediately mark as bad. If omitted, use `bisect bad` later. |
| `--good` / `-g` | Optional commit to immediately mark as good. If omitted, use `bisect good` later. |

```bash
# Start with no initial markers
libra bisect start

# Start with known bad (current HEAD) and good commit
libra bisect start HEAD --good v1.0

# Start with a specific bad commit
libra bisect start abc1234 --good def5678
```

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

## Design Rationale

### Why is bisect not hidden?

Despite being listed as a hidden command in some early designs, `libra bisect` is a fully visible subcommand. Binary search for regressions is a fundamental debugging workflow that benefits both human users and AI agents. Hiding it would reduce discoverability without meaningful benefit. The command is stable and follows the same patterns as other Libra commands.

### Why no `--run` for automated bisect?

Git's `git bisect run <script>` automatically runs a test script at each step and marks commits as good or bad based on the exit code. This is a powerful feature but introduces several complications: shell escaping, cross-platform script execution, error handling for flaky tests, and timeout management. Libra omits `--run` in favor of explicit `good`/`bad` marking for the following reasons:

1. **Agent integration**: AI agents can drive the bisect loop programmatically by calling `bisect good` and `bisect bad` based on their own test evaluation, which is more flexible than a shell script.
2. **Simplicity**: The manual workflow is straightforward and covers the common case. Automated bisect can be built on top using a shell loop.
3. **Error transparency**: With manual marking, the user always knows exactly which commit is being tested and can investigate unexpected results. Automated runs can mask flaky test failures.

### Why no `--first-parent`?

Git's `git bisect --first-parent` restricts the search to first-parent commits only, which is useful in workflows with many merge commits. Libra's bisect traverses the full commit graph using BFS, which is simpler and correct for all topologies. First-parent restriction is primarily useful for large projects with a strict merge-commit workflow; Libra's target use case of trunk-based development typically has a more linear history where this optimization is unnecessary.

### Why SQLite state persistence?

Bisect sessions can span hours or days as the user tests each candidate. Storing state in the SQLite `bisect_state` table ensures the session survives process restarts, editor closes, and system reboots. Git uses flat files in `.git/BISECT_*`, which achieves the same persistence but with less structure. SQLite provides transactional writes and the ability to query state programmatically, which is valuable for AI agent integration.

### Why does `reset` accept an optional `<rev>`?

Sometimes the user wants to end the bisect session but go to a different commit than where they started. For example, after finding the culprit, they might want to reset to the commit just before the bug was introduced. The optional `<rev>` parameter provides this flexibility without requiring a separate `checkout` after reset.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Start session | `bisect start [<bad>] [--good <commit>]` | `bisect start [<bad> [<good>...]]` | N/A |
| Mark bad | `bisect bad [<rev>]` | `bisect bad [<rev>]` | N/A |
| Mark good | `bisect good [<rev>]` | `bisect good [<rev>]` | N/A |
| Reset | `bisect reset [<rev>]` | `bisect reset [<commit>]` | N/A |
| Skip | `bisect skip [<rev>]` | `bisect skip [<rev>...]` | N/A |
| Show log | `bisect log` | `bisect log` | N/A |
| Automated run | Not supported | `bisect run <script>` | N/A |
| Custom terms | Not supported | `bisect terms` / `--term-old` / `--term-new` | N/A |
| Replay session | Not supported | `bisect replay <logfile>` | N/A |
| Visualize | Not supported | `bisect visualize` | N/A |
| First-parent only | Not supported | `--first-parent` | N/A |
| Multiple good commits | Via repeated `bisect good` | Positional args to `start` | N/A |
| State storage | SQLite (`bisect_state` table) | Flat files (`.git/BISECT_*`) | N/A |

Note: jj does not have a bisect command. Users who need binary search debugging with jj must use external tooling or manually check out commits. This is a gap in jj's feature set that Libra addresses.

## Error Handling

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-REPO-003` | No commits in repository |
| `LBR-CLI-002` | Bisect session already in progress (for `start`) |
| `LBR-CLI-002` | No bisect session in progress (for `bad`, `good`, `skip`, `log`) |
| `LBR-CLI-003` | Commit not found (invalid rev argument) |
| `LBR-CLI-003` | Bad commit is an ancestor of good commit (invalid range) |
| `LBR-CONFLICT-001` | Uncommitted changes would be overwritten by checkout |
| `LBR-IO-001` | Failed to read bisect state from database |
| `LBR-IO-002` | Failed to save bisect state to database |
| `LBR-IO-002` | Failed to create bisect_state table |
