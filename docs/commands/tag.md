# `libra tag`

Create, list, or delete tags.

## Synopsis

```
libra tag [<name>] [-m <message> | -F <file>] [-f] [-s]
libra tag -l [-n <lines>] [--points-at <object>] [--contains <commit>] [--merged <commit>] [--sort <key>] [--column[=<mode>]]
libra tag -v <name>
libra tag -d <name>
```

## Description

`libra tag` manages lightweight and annotated tags. A lightweight tag is simply a named pointer to a commit, while an annotated tag stores a full tag object with a message, tagger identity, and timestamp.

Without arguments (or with `-l`), the command lists all tags. When given a name, it creates a new tag at HEAD. Adding `-m <message>` (or `-F <file>`, reading the message from a file or stdin) creates an annotated tag instead of a lightweight one. The `-f` flag allows overwriting an existing tag of the same name.

Tag references are stored in the SQLite database alongside branch references, providing the same transactional guarantees.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<name>` | positional (optional) | Tag name to create, show, or delete |
| `-l` | `--list` | | List all tags |
| `-d` | `--delete` | | Delete the named tag |
| `-m` | `--message` | `<msg>` | Create an annotated tag with the given message |
| `-F` | `--file` | `<file>` | Create an annotated tag, reading the message from a file (`-` for stdin). Conflicts with `-m`. |
| `-f` | `--force` | | Overwrite an existing tag |
| `-n` | `--n-lines` | `<lines>` | Number of annotation lines to display when listing (0 = names only) |
| | `--points-at` | `<object>` | List only tags pointing at the given object (peeled to its commit); implies list mode |
| `-s` | `--sign` | | Sign the annotated tag with a vault PGP key (requires `-m`; not Git GPG-interoperable) |
| `-v` | `--verify` | `<name>` | Verify a tag's vault PGP signature (exit 0 good, exit 1 bad) |
| | `--contains` | `<commit>` | List only tags whose tip has `<commit>` as an ancestor |
| | `--no-contains` | `<commit>` | List only tags whose tip does not have `<commit>` as an ancestor |
| | `--merged` | `<commit>` | List only tags reachable from `<commit>` |
| | `--no-merged` | `<commit>` | List only tags not reachable from `<commit>` |
| | `--sort` | `<key>` | Sort the listing by key (`refname`, `-refname`, `creatordate`) |
| | `--column` | `[mode]` | Lay out the tag list in columns. Modes `always`/`auto`/`never` (bare = `always`). Cannot be combined with `-n`. |

### Flag examples

```bash
# Create a lightweight tag at HEAD
libra tag v1.0

# Create an annotated tag with a message
libra tag -m "Release v1.1" v1.1

# Create an annotated tag, reading the message from a file (or stdin with -)
libra tag -F release-notes.txt v1.1
libra log -1 --format=%B | libra tag -F - v1.1

# Force-overwrite an existing tag
libra tag -f v1.0

# List all tags
libra tag -l

# List tags with annotation preview (2 lines)
libra tag -l -n 2

# List only tags pointing at HEAD's commit
libra tag --points-at HEAD

# Delete a tag
libra tag -d v1.0

# JSON output for agents
libra tag --json v1.0
```

## Common Commands

```bash
libra tag v1.0                        # Create a lightweight tag at HEAD
libra tag -m "Release v1.1" v1.1      # Create an annotated tag
libra tag -l -n 2                     # List tags with up to 2 annotation lines
libra tag --points-at HEAD            # List tags pointing at HEAD's commit
libra tag -d v1.0                     # Delete a tag
libra tag --json v1.0                 # Structured JSON output for agents
```

## Human Output

- `libra tag -l`: prints the tag list, one per line; with `-n` shows annotation lines indented
- `libra tag v1.0`: `Created lightweight tag 'v1.0' at abc1234`
- `libra tag -m "msg" v1.0`: `Created annotated tag 'v1.0' at abc1234`
- `libra tag -d v1.0`: `Deleted tag 'v1.0' (was abc1234)`
- The default create path preserves the current human-readable output

## Structured Output (JSON examples)

`--json` / `--machine` uses `action` to distinguish operations:

Create a tag:

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "create",
    "name": "v1.0",
    "hash": "abc123...",
    "tag_type": "lightweight",
    "message": null
  }
}
```

Create an annotated tag:

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "create",
    "name": "v1.1",
    "hash": "abc123...",
    "tag_type": "annotated",
    "message": "Release v1.1"
  }
}
```

List tags:

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "list",
    "tags": [
      { "name": "v1.0", "hash": "abc123...", "tag_type": "lightweight", "message": null },
      { "name": "v1.1", "hash": "def456...", "tag_type": "annotated", "message": "Release v1.1" }
    ]
  }
}
```

Delete a tag:

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "delete",
    "name": "v1.0",
    "hash": "abc123..."
  }
}
```

`action=list` returns a `tags` array; `action=delete` returns `name` and `hash`.
For recovery deletes of malformed tag refs, `hash` can be `null` when the stored target is missing.

## Design Rationale

### Why vault PGP signing instead of GPG?

`libra tag -s` signs an annotated tag and `libra tag -v` verifies it, but both go through a vault PGP key rather than a local GPG keyring. The mechanism differs from Git's GPG signing on purpose:

- **GPG key management is fragile**: developers frequently lose keys, let them expire, or misconfigure gpg-agent, leading to broken signing workflows. In CI/CD environments, managing GPG keyrings securely is an operational burden.
- **Vault-based signing is the intended path**: signing keys live in Libra's vault (see `--vault` on `libra init`) so cryptographic operations are delegated to a secure key store rather than requiring each developer to maintain local GPG keys. This centralizes trust and simplifies key rotation.
- **Not Git-interoperable**: because the armored signature is produced and checked through the vault PGP path, `libra tag -s` is *not* bit-compatible with `git tag -s`/`git tag -v`. A tag signed in Libra verifies with `libra tag -v`, not with Git's GPG verification, and vice versa.

Signing requires `-m` (Libra does not open an editor for tag bodies). `libra tag -v <name>` exits 0 for a good signature and 1 for a bad one; unsigned, non-annotated, or missing tags report a clear error.

### Why lightweight vs annotated distinction?

Libra preserves Git's two-tier tag model for on-disk format compatibility. Lightweight tags are simple ref pointers (ideal for temporary markers), while annotated tags store metadata useful for releases. A message source is the toggle: providing `-m` or `-F` creates an annotated tag, its absence creates a lightweight one. This matches Git's behavior exactly, keeping the mental model consistent for users migrating from Git.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | Libra | jj |
|---------|-----|-------|----|
| Create lightweight | `git tag <name>` | `libra tag <name>` | `jj tag create <name>` |
| Create annotated | `git tag -a -m "msg" <name>` | `libra tag -m "msg" <name>` | Not supported (lightweight only) |
| Annotated message from file | `git tag -F <file> <name>` | `libra tag -F <file> <name>` (`-` for stdin) | N/A |
| List tags | `git tag -l` | `libra tag -l` | `jj tag list` |
| List with message | `git tag -l -n3` | `libra tag -l -n 3` | N/A |
| List by target | `git tag --points-at <obj>` | `libra tag --points-at <obj>` | N/A |
| Column layout | `git tag --column[=<mode>]` | `libra tag --column[=<mode>]` (always/auto/never) | N/A |
| Delete | `git tag -d <name>` | `libra tag -d <name>` | `jj tag delete <name>` |
| Force overwrite | `git tag -f <name>` | `libra tag -f <name>` | `jj tag create <name>` (always overwrites) |
| Sign tag | `git tag -s <name>` | `libra tag -s -m "msg" <name>` (vault PGP; requires `-m`, not Git GPG-interoperable) | N/A |
| Verify tag | `git tag -v <name>` | `libra tag -v <name>` (vault PGP) | N/A |
| Structured output | No | `--json` / `--machine` | `--template` |

## Error Handling

| Scenario | Error Code | Hint |
|----------|-----------|------|
| Tag already exists | `LBR-CONFLICT-002` | "delete it first with 'libra tag -d <name>'." |
| HEAD has no commit to tag | `LBR-REPO-003` | "create a commit first before tagging HEAD." |
| Tag not found (delete/show) | `LBR-CLI-003` | "use 'libra tag -l' to list available tags." |
| Unresolvable `--points-at` object | `LBR-CLI-003` | "use 'libra log --oneline' to see available commits." |
| Missing tag name for --delete/--message/--file/--force | `LBR-CLI-002` | "use 'libra tag <name>' to create or update a tag" |
| `-m`/`-F` combined with a non-create mode (list/delete/verify/filters) | `LBR-CLI-002` | "-m/--message and -F/--file are only valid when creating a tag" |
| Failed to resolve HEAD | `LBR-IO-001` or `LBR-REPO-002` | -- |
| Failed to serialize annotated tag | `LBR-REPO-005` | -- |
| Failed to store object | `LBR-IO-002` | -- |
| Failed to persist reference | `LBR-IO-002` | -- |
| Failed to delete tag | `LBR-IO-002` | -- |
| Failed to list tags (DB error) | `LBR-IO-001` | -- |
| Failed to list tags (corrupt object) | `LBR-REPO-002` | -- |
