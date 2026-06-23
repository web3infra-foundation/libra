# `libra cat-file`

Inspect Git objects and Libra AI history objects stored in the repository.

## Synopsis

```
libra cat-file [OPTIONS] [OBJECT]
```

## Description

`libra cat-file` is a low-level debugging tool analogous to `git cat-file`. It
can print the type, size, or pretty-printed content of any Git object (commit,
tree, blob, tag), and can also check for object existence.

Libra extends the classic command with `--ai*` flags that inspect AI workflow
objects (Intent, Task, Run, Plan, PatchSet, Evidence, Session, etc.) stored on
the `libra/intent` orphan branch. This gives developers and agents a single
entry point for introspecting both version-control objects and AI process
artifacts.

Exactly one mode flag must be specified. Git modes (`-t`, `-s`, `-p`, `-e`)
require a positional `OBJECT` argument. AI modes (`--ai`, `--ai-type`,
`--ai-list`, `--ai-list-types`) ignore `OBJECT` and operate on the AI history
branch.

## Options

| Flag | Short | Description |
|------|-------|-------------|
| `-t` | | Print the object type (`commit`, `tree`, `blob`, `tag`). |
| `-s` | | Print the object size in bytes. |
| `-p` | | Pretty-print the object content. |
| `-e` | | Check if the object exists (exit status only, no stdout). Does not support `--json`. |
| `--batch-check[=<fmt>]` | | Read object names from stdin (one per line); print `<sha> <type> <size>` (or `<input> missing`). Optional format atoms `%(objectname)`/`%(objecttype)`/`%(objectsize)`. |
| `--batch[=<fmt>]` | | Like `--batch-check` plus the raw object contents and a trailing newline. |
| `--batch-command[=<fmt>]` | | Read commands from stdin: `info <object>` (header only) or `contents <object>` (header + contents). `flush` requires `--buffer`, which is not exposed. |
| `--ai <ID>` | | Pretty-print an AI object by ID. Accepts `TYPE:ID` to disambiguate. |
| `--ai-type <ID>` | | Print the AI object type for the given ID. |
| `--ai-list <TYPE>` | | List all AI objects of the given type (e.g., `intent`, `patchset`, `event`). |
| `--ai-list-types` | | List all AI object types present in the history branch. |
| `<OBJECT>` | | Git object hash or ref. Required for `-t`/`-s`/`-p`/`-e`; ignored for `--ai*` modes; batch modes read object names from stdin instead. |

### Examples

```bash
# Print the type of HEAD
libra cat-file -t HEAD

# Print the size of a specific object
libra cat-file -s 40d352ee7190f92dcf7883b8a81f2c730fd8a860

# Pretty-print HEAD commit
libra cat-file -p HEAD

# Check existence (exit code 0 = exists)
libra cat-file -e abc1234

# Structured JSON type query
libra cat-file -t HEAD --json

# List all AI intent objects
libra cat-file --ai-list intent

# Pretty-print an AI object (disambiguate with TYPE:ID)
libra cat-file --ai patchset:call_KjR3NB4cQaT5Rm1c7zXjsskQ

# Print the type of an AI object
libra cat-file --ai-type debug-local-1772707227

# List all AI object types in the repository
libra cat-file --ai-list-types --json
```

## Common Commands

```bash
libra cat-file -t HEAD
libra cat-file -s HEAD
libra cat-file -p HEAD
libra cat-file -t HEAD --json
libra cat-file --ai-list-types --json
libra cat-file --ai-list intent
libra cat-file --ai <session-id>
```

## Human Output

- `-t`: prints the object type on a single line (e.g., `commit`)
- `-s`: prints the size in bytes on a single line (e.g., `342`)
- `-p`: pretty-prints content depending on type:
  - Commit: header fields and message
  - Tree: `<mode> <type> <hash>\t<name>` per entry
  - Blob: raw text content
  - Tag: tag header and message
- `-e`: no output; exit code 0 if the object exists, non-zero otherwise
- `--ai <ID>`: prints a formatted summary (session summary for `ai_session` objects, full JSON for others)
- `--ai-list <TYPE>`: one object ID per line
- `--ai-list-types`: one type name per line

## Structured Output (JSON examples)

### Type mode (`-t`)

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "type",
    "object": "HEAD",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "object_type": "commit"
  }
}
```

### Size mode (`-s`)

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "size",
    "object": "HEAD",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "size": 342
  }
}
```

### Pretty-print mode (`-p`) -- commit

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "pretty",
    "object": "HEAD",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "object_type": "commit",
    "content": {
      "tree": "def456...",
      "parents": ["abc123..."],
      "author": "Alice <alice@example.com> 1711929600 +0000",
      "committer": "Alice <alice@example.com> 1711929600 +0000",
      "message": "feat: add new feature"
    }
  }
}
```

### AI list types

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "ai-list-types",
    "types": ["intent", "patchset", "plan", "run", "task"]
  }
}
```

Notes:

- `cat-file -e` does not support `--json` / `--machine` (it is purely an exit-code check)
- Blob/tag pretty-print JSON requires UTF-8 content; non-text payloads fail explicitly instead of returning lossy data

## Design Rationale

### Why add `--ai*` flags?

Libra's AI agent infrastructure stores process artifacts (intents, plans, tasks,
runs, patch sets, evidence, sessions) as Git objects on an orphan branch. Rather
than requiring a separate inspection tool, `cat-file` is the natural home
because it already handles "show me the raw content of an object by ID." The
`--ai*` flags extend this to the AI object namespace while keeping the familiar
interface. This means a single command can answer both "what type is this
commit?" and "what does this AI plan contain?" -- which is especially useful
during debugging of agent workflows.

### Batch modes and structured output

Git's batch modes read object IDs (or commands) from stdin for bulk inspection.
Libra exposes `--batch-check`, `--batch`, and `--batch-command` (the latter
dispatching `info`/`contents` per line), all sharing the same per-object
formatter with optional `=<format>` atom expansion. For agents, `--json` remains
the recommended interface — it returns typed fields in one call. Streaming
`--buffer`/`flush` and `--batch-all-objects` are not exposed; without `--buffer`,
the `flush` command is rejected exactly as Git does.

### Why does `-e` stay human-only?

The `-e` (existence check) mode communicates its result via exit code: 0 means
the object exists, non-zero means it does not. This is the Unix convention for
boolean predicates. Wrapping it in JSON would add no information (`{"exists":
true}`) while breaking the expectation that `-e` is a silent probe. Scripts
and agents that need a structured existence check can use `-t` with `--json`
instead -- if the object does not exist, the JSON response will contain an error.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Print object type | `-t` | `-t` | N/A (no direct equivalent) |
| Print object size | `-s` | `-s` | N/A |
| Pretty-print content | `-p` | `-p` | N/A (`jj file show` for blobs) |
| Check existence | `-e` | `-e` | N/A |
| Batch mode | `--batch[=<format>]`, `--batch-check[=<format>]`, `--batch-command[=<format>]` (info/contents; `%(objectname)`/`%(objecttype)`/`%(objectsize)` atoms; `--batch-all-objects` and `--buffer`/`flush` not exposed) | `--batch`, `--batch-check`, `--batch-command` | N/A |
| AI object inspection | `--ai`, `--ai-type` | N/A | N/A |
| AI object listing | `--ai-list`, `--ai-list-types` | N/A | N/A |
| JSON output | `--json` | No | No |
| Object resolution | SHA-1, refs, `HEAD~N` | SHA-1, refs, all rev-parse syntax | Change IDs, revsets |
| `--filters` | No | `--filters` (convert to/from external) | N/A |
| `--textconv` | No | `--textconv` | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid object / revision | `LBR-CLI-003` | 129 |
| Unsupported argument combination | `LBR-CLI-002` | 129 |
| Failed to read object data | `LBR-IO-001` / `LBR-REPO-002` | 128 |
