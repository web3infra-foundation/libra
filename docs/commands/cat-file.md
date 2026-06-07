# `libra cat-file`

Inspect Git objects and Libra AI history objects stored in the repository.

## Synopsis

```
libra cat-file (-t | -s | -p | -e) <OBJECT>
libra cat-file (--batch | --batch-check)[=<format>] [-Z] [--buffer] [--follow-symlinks] < object-list
libra cat-file (--batch | --batch-check)[=<format>] --batch-all-objects [--unordered]
libra cat-file (--ai | --ai-type) <ID> | --ai-list <TYPE> | --ai-list-types
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
| `--batch[=<format>]` | | Read object refs from stdin; print metadata + raw payload per object. Optional `=<format>` (default `%(objectname) %(objecttype) %(objectsize)`). |
| `--batch-check[=<format>]` | | Like `--batch` but metadata only (no payload). |
| `--batch-all-objects` | | With `--batch`/`--batch-check`, iterate every local object instead of reading stdin (local loose + pack only). |
| `--unordered` | | With `--batch-all-objects`, emit in scan order instead of sorted by object id. |
| `--buffer` | | Buffer batch output and flush once at EOF (default flushes per object). |
| | `-Z` | Use NUL (`\0`) instead of newline to separate/terminate batch input and output records. |
| `--follow-symlinks` | | Follow in-tree symlinks when resolving `<tree-ish>:<path>` batch inputs (object graph only; never the working tree). |
| `--textconv` | | **Unsupported** (rejected, `LBR-UNSUPPORTED-001`): Libra has no textconv driver. |
| `--filters` | | **Unsupported** (rejected, `LBR-UNSUPPORTED-001`): Libra has no clean/smudge filter driver. |
| `--ai <ID>` | | Pretty-print an AI object by ID. Accepts `TYPE:ID` to disambiguate. |
| `--ai-type <ID>` | | Print the AI object type for the given ID. |
| `--ai-list <TYPE>` | | List all AI objects of the given type (e.g., `intent`, `patchset`, `event`). |
| `--ai-list-types` | | List all AI object types present in the history branch. |
| `<OBJECT>` | | Git object hash or ref. Required for `-t`/`-s`/`-p`/`-e`; ignored for `--ai*` modes. |

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

# Stream object metadata for refs read from stdin
printf 'HEAD\nabc1234\n' | libra cat-file --batch-check

# Stream metadata + raw content for each object
echo HEAD | libra cat-file --batch

# Custom batch format (note the =<format> syntax)
echo HEAD | libra cat-file --batch-check="%(objectname) %(objecttype)"

# NUL-separated batch I/O (robust against odd ref names)
printf 'HEAD\0' | libra cat-file --batch-check -Z

# Print metadata for every local object, sorted by id
libra cat-file --batch-all-objects --batch-check

# Follow an in-tree symlink to its target
echo 'HEAD:path/to/symlink' | libra cat-file --batch-check --follow-symlinks

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
echo HEAD | libra cat-file --batch-check
libra cat-file --batch-all-objects --batch-check
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

### Batch mode (and its scope)

`libra cat-file` implements the core of Git's batch protocol — `--batch`,
`--batch-check` (each with the optional `=<format>` syntax and the
`%(objectname)`/`%(objecttype)`/`%(objectsize)` atoms), `--batch-all-objects`,
`--unordered`, `-Z` (NUL records), `--buffer`, and `--follow-symlinks` — so
external scripts and tools can drive Libra as an object-metadata/content
backend. Batch output is a byte-exact text protocol, so `--json`/`--machine` is
not layered on top of it.

Differences from Git (intentional, see `COMPATIBILITY.md`):

- A single batch input record is capped at 4 KiB (DoS hardening); over-long
  input is a `LBR-CLI-003` (129) syntax error.
- Only the `missing` and `ambiguous` per-line states are emitted; Git's
  `filtered`/`submodule` states are deferred.
- `--follow-symlinks` resolves only within the object graph (never the working
  tree). A link that escapes the tree root resolves to `<path> missing` rather
  than Git's `symlink <size>` content line; loops are bounded at depth 32.
- `--batch-all-objects` covers only local (loose + pack) objects, not
  un-fetched cloud-tiered objects.
- `--batch-command`, `--filter`, `--use-mailmap`, `--path`, and the bare
  `<type> <object>` form are not yet implemented (deferred); `--textconv` /
  `--filters` are rejected as unsupported.

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
| Batch metadata/content | `--batch`, `--batch-check` (partial) | `--batch`, `--batch-check` | N/A |
| Batch all objects | `--batch-all-objects` (local only) | `--batch-all-objects` | N/A |
| Batch ordering | `--unordered` | `--unordered` | N/A |
| NUL records | `-Z` | `-Z` (and deprecated `-z`) | N/A |
| Batch buffering | `--buffer` | `--buffer` | N/A |
| Follow symlinks | `--follow-symlinks` (in-tree only; escape → `missing`) | `--follow-symlinks` | N/A |
| Batch command stream | Deferred | `--batch-command` | N/A |
| AI object inspection | `--ai`, `--ai-type` | N/A | N/A |
| AI object listing | `--ai-list`, `--ai-list-types` | N/A | N/A |
| JSON output | `--json` (single-object modes only) | No | No |
| Object resolution | SHA-1/SHA-256, refs, `HEAD~N` | SHA-1, refs, all rev-parse syntax | Change IDs, revsets |
| `--filters` | Rejected (`LBR-UNSUPPORTED-001`) | `--filters` (convert to/from external) | N/A |
| `--textconv` | Rejected (`LBR-UNSUPPORTED-001`) | `--textconv` | N/A |
| `--path`, `--filter`, `--use-mailmap` | Deferred | supported | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid object / revision (single-object modes) | `LBR-CLI-003` | 129 |
| Over-long (>4 KiB) batch input record | `LBR-CLI-003` | 129 |
| Unsupported argument combination (batch + `--json`, modifier without batch, batch + positional, unknown format atom) | `LBR-CLI-002` | 129 |
| Mode-group conflict (e.g. `--batch-check -p`) | `LBR-CLI-002` | 129 |
| `--textconv` / `--filters` | `LBR-UNSUPPORTED-001` | 128 |
| Failed to read object data | `LBR-IO-001` / `LBR-REPO-002` | 128 |
| Batch line — object missing / ambiguous short SHA | — | 0 (printed inline as `<object> missing` / `<object> ambiguous`) |
| Downstream pipe closed (`BrokenPipe`) | — | 0 (stream stops quietly) |
