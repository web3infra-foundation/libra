# `libra notes`

Historical design for adding, showing, listing, or removing notes attached to
commits without modifying the commits themselves.

> Status: `partial`. `libra notes` is now registered in the public CLI. The core
> operations (`add`, `append`, `list`, `show`, `remove`) are supported. Advanced
> Git notes subcommands (`edit`, `copy`, `merge`, `prune`, `get-ref`) and
> interactive editor support are not implemented.

## Synopsis

```
libra notes add [-m <message> | -F <file>] [-f] [<object>]
libra notes append [-m <message> | -F <file>] [<object>]
libra notes list [<object>]
libra notes show [<object>]
libra notes remove [<object>...]
```

## Description

The unpublished design manages annotations attached to commit objects. Unlike commit
messages, notes can be added or removed after the commit is created — the
original commit hash stays unchanged. This makes them useful for post-hoc
metadata such as code-review results, CI status, or deploy tracking.

Notes are stored as blob objects under a notes ref (default
`refs/notes/commits`). Use `--ref <ref>` to operate on a different namespace
(e.g., `refs/notes/review`).

If this command is published in a future release, omitting a subcommand should default to `list`.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<object>` | positional (optional) | Commit to annotate, show, or remove notes from. Defaults to HEAD. |
| `-m` | `--message` | `<msg>` | Note message text. Repeatable; blank lines separate messages. |
| `-F` | `--file` | `<file>` | Read note message from file (`-` for stdin). |
| `-f` | `--force` | | Overwrite an existing note (for `add`). |
| | `--ref` | `<ref>` | Operate on a specific notes ref (default: `refs/notes/commits`). |

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `add` | Add a note to an object. Fails if a note already exists; use `-f` to overwrite. Requires `-m` or `-F`. |
| `append` | Append a message to an object's note (separated by a blank line), creating the note if absent. Requires `-m` or `-F`. |
| `list` | List note objects and the commits they annotate (default subcommand). |
| `show` | Show the note text for an object. |
| `remove` | Remove notes for one or more objects. |

### Flag examples

```bash
# Annotate HEAD with a review result
libra notes add -m "Reviewed-by: Alice <alice@example.com>"

# Add from a file
libra notes add -F review-summary.txt abc1234

# Force-overwrite an existing note
libra notes add -m "Updated review" -f HEAD

# Append another line to HEAD's note (blank-line separated)
libra notes append -m "Deployed-by: CI"

# List all notes
libra notes list

# Show the note on HEAD
libra notes show

# Show the note on a specific commit
libra notes show abc1234

# Remove the note on HEAD
libra notes remove

# Remove notes from multiple commits
libra notes remove abc1234 def5678

# Use a custom namespace
libra notes --ref refs/notes/ci add -m "Passed all tests" HEAD
libra notes --ref refs/notes/ci show HEAD

# JSON output for agents
libra notes show --json
libra notes list --json
```

## Common Commands

```bash
libra notes add -m "Reviewed-by: Alice"       # Add a note to HEAD
libra notes show                                # Show the note on HEAD
libra notes list                                # List all notes
libra notes remove abc1234                      # Remove a note
libra notes add -f -m "Updated" HEAD            # Force-overwrite a note
libra notes --json show                         # Structured JSON output
```

## Human Output

- `libra notes add -m "msg"`: `Added note to abc1234 in refs/notes/commits`
- `libra notes show`: prints the note text as-is
- `libra notes list`: `<note-hash> <annotated-object-hash>`, one per line
- `libra notes remove abc1234`: `Removed note from abc1234 in refs/notes/commits`
- `libra notes` (no args): same as `list`

## Structured Output (JSON examples)

If this command is published in a future release, `--json` / `--machine` should use `action` to distinguish operations:

### `add`

```json
{
  "ok": true,
  "command": "notes",
  "data": {
    "action": "add",
    "ref": "refs/notes/commits",
    "object": "abc1234...",
    "note_hash": "def5678..."
  }
}
```

### `show`

```json
{
  "ok": true,
  "command": "notes",
  "data": {
    "action": "show",
    "ref": "refs/notes/commits",
    "object": "abc1234...",
    "note_hash": "def5678...",
    "text": "Reviewed-by: Alice <alice@example.com>"
  }
}
```

### `list`

```json
{
  "ok": true,
  "command": "notes",
  "data": {
    "action": "list",
    "ref": "refs/notes/commits",
    "notes": [
      { "note_hash": "def5678...", "annotated_object": "abc1234..." },
      { "note_hash": "1111222...", "annotated_object": "def5678..." }
    ]
  }
}
```

When `<object>` is given and no note exists, `note_hash` is `null`.

### `remove`

```json
{
  "ok": true,
  "command": "notes",
  "data": {
    "action": "remove",
    "ref": "refs/notes/commits",
    "removed": [
      { "object": "abc1234...", "note_hash": "def5678..." }
    ]
  }
}
```

## Design Rationale

### Why no editor support?

Git opens an editor when `-m` / `-F` are not given. Libra omits editor
invocations — editors assume an interactive user and are incompatible with
headless or agent-driven workflows. `-m <message>` or `-F <file>` is required
for note creation.

### Why no `edit`, `copy`, `merge`, `prune`, `get-ref`?

These Git subcommands add complexity for niche or collaborative workflows.
The core operations (`add`, `append`, `list`, `show`, `remove`) cover the
primary use case: attaching structured metadata to commits. The remaining
subcommands can be emulated (`edit` is `remove` + `add`) and added incrementally
if real users or agents need them.

### Why SQLite-backed notes refs?

Libra stores notes refs in SQLite rather than loose files under
`.git/refs/notes/`. This provides atomic transactions (add/remove in a single
operation), efficient queries (listing all notes is one query, not a directory
scan), and concurrency safety via SQLite WAL mode.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | Libra | jj |
|---------|-----|-------|----|
| Add note | `git notes add [-m <msg>] [<obj>]` | `libra notes add -m <msg> [<obj>]` | N/A |
| List notes | `git notes list [<obj>]` | `libra notes list [<obj>]` | N/A |
| Show note | `git notes show [<obj>]` | `libra notes show [<obj>]` | N/A |
| Remove note | `git notes remove [<obj>...]` | `libra notes remove [<obj>...]` | N/A |
| Append | `notes append` | Supported | N/A |
| Edit / Copy / Merge / Prune | Supported | Not supported | N/A |
| Custom ref | `--ref <ref>` | `--ref <ref>` | N/A |
| File input | `-F <file>` | `-F <file>` | N/A |
| Editor support | Interactive editor (default) | Not supported (`-m` / `-F` required) | N/A |
| Structured output | No | `--json` / `--machine` | N/A |
| Ref storage | Loose files + packed-refs | SQLite (libra.db) | N/A |

Note: jj does not have a notes feature.

## Error Handling

| Scenario | Error Code | Hint |
|----------|-----------|------|
| Object already has a note (add) | `LBR-CONFLICT-002` | "use '-f' to overwrite the existing note." |
| Object has no note (show/remove) | `LBR-CLI-003` | "use 'libra notes list' to see which objects have notes." |
| Neither `-m` nor `-F` provided (add) | `LBR-CLI-002` | "provide a message with '-m <msg>' or '-F <file>'." |
| Invalid object reference | `LBR-CLI-003` | "use 'libra log' to find valid commit references." |
| Invalid notes ref name | `LBR-CLI-002` | "notes refs must start with 'refs/notes/'; e.g. 'refs/notes/commits'." |
| Not a libra repository | `LBR-REPO-001` | Initialize with `libra init` or navigate to a repo. |
| Failed to load/store blob object | `LBR-IO-002` | Check repository integrity. |
| Failed to read/write notes ref | `LBR-IO-002` | Check database permissions and writability. |
