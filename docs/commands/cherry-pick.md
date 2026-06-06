# `libra cherry-pick`

Apply the changes introduced by some existing commits.

## Synopsis

```
libra cherry-pick [options] <commit>...
libra cherry-pick (--continue | --skip | --abort | --quit)
```

## Description

`libra cherry-pick` applies the changes introduced by the specified commits onto the current branch. For each named commit, Libra performs a three-way apply (base = the commit's parent, ours = the current branch, theirs = the commit) and, unless `--no-commit` is given, records a new commit. Multiple commits are applied left-to-right.

When a commit cannot be applied cleanly, Libra writes conflict markers into the working tree and stage 1/2/3 entries into the index, then **persists the in-progress sequence** so it can be resumed. The sequencer state lives only in the SQLite `cherry_pick_state` table (there is no `.git`/`.libra` sequencer file), matching Libra's metadata-in-SQLite convention. Resolve the conflict, `libra add` the paths, and run `libra cherry-pick --continue` (or `--skip` / `--abort` / `--quit`).

The command requires an active branch (not detached HEAD). A merge commit is refused unless `-m <parent-number>` selects which parent to treat as the base.

## Options

### `-n`, `--no-commit`

Apply the changes to the index and working tree but do **not** create a commit. Multiple commits accumulate into the index. A conflict during a `--no-commit` sequence is terminal — it is **not** resumable; clean up with `libra reset --hard` / `libra restore`.

### `-x`

Append a `(cherry picked from commit <oid>)` line to the commit message. **Off by default** (matching Git). If the message already contains the line it is not duplicated.

### `-s`, `--signoff`

Append a `Signed-off-by: <name> <email>` trailer (identity resolved from config, then the `GIT_*` / `LIBRA_COMMITTER_*` / `EMAIL` environment cascade). With `-x`, the cherry-picked-from line comes first and `Signed-off-by` last.

### `-e`, `--edit`

Open the configured editor (`GIT_EDITOR` → `core.editor` → `VISUAL` → `EDITOR`) to edit the message before committing. With no usable editor, no TTY, or in machine/JSON mode, it silently keeps the assembled message.

### `--allow-empty`

Cherry-pick a commit whose own change set is empty (its tree equals its parent's). Blocked by default.

### `--allow-empty-message`

Allow a commit with an empty message. Blocked by default.

### `--keep-redundant-commits`

Keep a commit that becomes redundant after being replayed (produces no change against the current HEAD). Blocked by default.

### `-m <n>`, `--mainline <n>`

Cherry-pick a **merge commit** by treating parent number `<n>` (1-based) as the base for the diff. Required for merge commits; an out-of-range `<n>`, or `-m` on a non-merge commit, is a usage error.

### `--ff`

When the picked commit is a single-parent direct child of HEAD and no message-rewriting modifier (`-n`/`-x`/`-s`/`-e`/`-m`) is set, fast-forward HEAD to the commit instead of replaying it (no new commit, no hash drift).

### `-S`, `--gpg-sign`

Sign the new commit using the Libra vault signing key (the same chain as `libra merge --gpg-sign`). `--no-gpg-sign` (default) disables it.

### `--continue` / `--skip` / `--abort` / `--quit`

Drive the conflict sequencer (mutually exclusive; cannot be combined with `<commit>` arguments):

- **`--continue`** — after resolving conflicts and `libra add`-ing them, finalize the conflicted commit and resume the rest of the sequence.
- **`--skip`** — discard the current conflicted commit and continue with the next.
- **`--abort`** — restore HEAD, index, and working tree to the pre-sequence state and clear the sequencer.
- **`--quit`** — forget the sequencer without touching the working tree.

`--continue`/`--abort` refuse to run from a different branch than the one the sequence began on.

### `--json` / `--machine` / `--quiet`

`--json` emits a structured envelope; `--machine` emits the same envelope as one NDJSON line; `--quiet` suppresses human-readable stdout. `--machine` does **not** suppress output (it emits machine JSON).

## Unsupported Git options

The following are explicitly rejected with `LBR-UNSUPPORTED-001` (exit 128): `--strategy`, `-X` / `--strategy-option`, `--empty=<mode>` (use `--allow-empty` / `--keep-redundant-commits`), `--cleanup=<mode>`, `--rerere-autoupdate` / `--no-rerere-autoupdate`, and `--commit` (auto-commit is the default; use `-n` to stage only). A `-S<keyid>` / `--gpg-sign=<keyid>` form (external key id) is not accepted — vault signing takes no external key id.

## Common Commands

```bash
# Cherry-pick a single commit onto the current branch
libra cherry-pick abc1234

# Cherry-pick multiple commits in sequence
libra cherry-pick abc1234 def5678

# Apply without committing (changes accumulate in the index)
libra cherry-pick -n abc1234 def5678

# Append provenance / sign-off
libra cherry-pick -x -s abc1234

# Cherry-pick a merge commit following its first parent
libra cherry-pick -m 1 <merge-commit>

# Resolve a conflict, then resume
libra add <resolved-paths>
libra cherry-pick --continue

# Cancel an in-progress cherry-pick
libra cherry-pick --abort

# JSON output for AI agents or scripts
libra cherry-pick --json abc1234
```

## Human Output

With auto-commit (default): `[def5678] cherry-picked from abc1234`

With `-n`: `Changes from abc1234 staged. Use 'libra commit' to finalize.`

`--abort`: `cherry-pick aborted; HEAD reset to <short>` · `--quit`: `cherry-pick state cleared; working tree left unchanged`

A conflict is an **error** (`LBR-CONFLICT-001`), printed to stderr with a resolution hint — it is never reported through the success envelope.

## Structured Output (JSON)

```json
{
  "command": "cherry-pick",
  "data": {
    "picked": [
      {
        "source_commit": "abc1234…",
        "short_source": "abc1234",
        "new_commit": "def5678…",
        "short_new": "def5678"
      }
    ],
    "no_commit": false
  }
}
```

With `--no-commit`, `new_commit`/`short_new` are `null`. Sequencer actions add optional fields (omitted for a plain pick, so old consumers are unaffected): `action` (`"continue"`/`"skip"`/`"abort"`/`"quit"`) and, for `--abort`, `restored_head`.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Git | jj | Libra |
|-----------|-----|-----|-------|
| Positional commits | `git cherry-pick <commit>...` | N/A (`jj rebase`) | `libra cherry-pick <commit>...` |
| No-commit mode | `-n` / `--no-commit` | N/A | `-n` / `--no-commit` (multi-commit allowed) |
| Append provenance | `-x` | N/A | `-x` (off by default) |
| Sign-off | `-s` / `--signoff` | N/A | `-s` / `--signoff` |
| Edit message | `-e` / `--edit` | N/A | `-e` / `--edit` |
| Mainline parent | `-m <n>` / `--mainline <n>` | N/A | `-m <n>` / `--mainline <n>` |
| Fast-forward | `--ff` | N/A | `--ff` |
| Continue / skip / abort / quit | `--continue` / `--skip` / `--abort` / `--quit` | N/A | `--continue` / `--skip` / `--abort` / `--quit` (state in SQLite) |
| GPG sign | `--gpg-sign` / `-S` | N/A | `-S` / `--gpg-sign` (vault) |
| Allow empty / keep redundant | `--allow-empty` / `--keep-redundant-commits` | N/A | `--allow-empty` / `--keep-redundant-commits` |
| Allow empty message | `--allow-empty-message` | N/A | `--allow-empty-message` |
| Strategy / `-X` | `--strategy` / `-X` | N/A | Unsupported (`LBR-UNSUPPORTED-001`) |
| `--empty` / `--cleanup` / rerere | `--empty` / `--cleanup` / `--rerere-autoupdate` | N/A | Unsupported (`LBR-UNSUPPORTED-001`) |
| JSON / machine output | N/A | N/A | `--json` / `--machine` |

**Note:** jj has no direct cherry-pick; the closest is `jj rebase -r <rev> -d <dest>`.

## Conflict resolution workflow

```bash
libra cherry-pick c1 c2 c3       # c2 conflicts
# ... edit the conflicted files to resolve the <<<<<<< / ======= / >>>>>>> markers ...
libra add <resolved-paths>
libra cherry-pick --continue     # finalizes c2 and applies c3
```

While a cherry-pick is in progress, a new `cherry-pick`, `merge`, or `rebase` is refused with `LBR-CONFLICT-002` until you `--continue`, `--abort`, or `--quit`.

## Error Handling

Exit codes follow Libra's coarse contract: `Cli`-class codes exit **129**; `Repo` / `Conflict` / `Io` / `Unsupported` classes exit **128**; clap parse failures exit **2**, except clap argument conflicts for a present subcommand (e.g. `--continue --abort`), which Libra remaps to `command_usage` → **129**.

| Code | Condition | Exit |
|------|-----------|------|
| `LBR-REPO-001` | Not inside a libra repository | 128 |
| `LBR-REPO-003` | Detached HEAD, or a sequencer flag with no cherry-pick in progress, or continuing on the wrong branch | 128 |
| `LBR-CLI-003` | Cannot resolve a commit reference | 129 |
| `LBR-CLI-002` | Merge commit without `-m`, `-m` out of range, `-m` on a non-merge commit, empty/redundant/empty-message commit blocked, or a clap usage conflict | 129 |
| `LBR-UNSUPPORTED-001` | An unsupported option (`--strategy`/`-X`/`--empty`/`--cleanup`/`--rerere-autoupdate`/`--commit`) | 128 |
| `LBR-CONFLICT-001` | Conflict during cherry-pick (state persisted for commit-per-pick mode) | 128 |
| `LBR-CONFLICT-002` | A new cherry-pick / merge / rebase while a cherry-pick is in progress | 128 |
| `LBR-IO-001` | Failed to read an object, index, or sequencer state | 128 |
| `LBR-IO-002` | Failed to write an object, index, worktree, or sequencer state | 128 |

## Design notes

- **`-x` is off by default** (Git-compatible). Earlier Libra builds always appended the provenance line; that was a behavior change in v0.17.1309.
- **Merge commits are unblocked via `-m`** (a reversal of the earlier "merge commits rejected entirely" stance).
- **Sequencer state is SQLite, not files.** Unlike Git's `.git/sequencer/`, Libra persists the in-progress pick in the `cherry_pick_state` table. Conflict detection is path-level (a divergent file becomes one conflict to resolve), not Git's line-level hunk merge.
- **Signing reuses the vault chain.** `-S` calls the same `vault` signing path as `libra merge --gpg-sign`; no separate signing subsystem is introduced.
