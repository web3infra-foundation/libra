# `libra tag`

Create, list, or delete tags.

## Synopsis

```
libra tag [<name>] [-a] [-m <message> | -F <file> | -e] [-f]
libra tag -l [-n <lines>] [--points-at <object>] [--contains [<commit>]]
libra tag -l [--no-contains [<commit>]] [--merged [<commit>] | --no-merged [<commit>]]
libra tag -l [--sort=<key>]
libra tag -d <name> [<name>...]
```

## Description

`libra tag` manages lightweight and annotated tags. A lightweight tag is simply a named pointer to a commit, while an annotated tag stores a full tag object with a message, tagger identity, and timestamp.

Without arguments (or with `-l`), the command lists all tags. When given a name, it creates a new tag at HEAD. Adding `-m <message>`, `-F <file>`, `-e`, or `-a` creates an annotated tag instead of a lightweight one. The `-f` flag allows overwriting an existing tag of the same name.

When listing, `--points-at <object>` keeps only tags whose target peels to the
given commit. Lightweight tags point straight at a commit; annotated tags peel
through their tag object to the commit they reference, so both forms are matched.
Like `-n`, `--points-at`, `--contains`, `--no-contains`, `--merged`, `--no-merged`, and `--sort` force list mode.

`--contains` / `--no-contains` filter by whether a tag's target commit history includes the requested commit, defaulting to `HEAD` when the value is omitted. `--merged` / `--no-merged` filter by whether the tag target is reachable from the requested baseline, also defaulting to `HEAD`. `--sort` supports `refname`, `-refname`, `creatordate`, and `-creatordate`.

Tag references are stored in the SQLite database alongside branch references, providing the same transactional guarantees.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<name>` | positional (optional) | Tag name to create, show, or delete |
| `-l` | `--list` | | List all tags |
| `-d` | `--delete` | | Delete one or more named tags |
| `-a` | `--annotate` | | Create an annotated tag; opens the configured editor when no message is supplied |
| `-m` | `--message` | `<msg>` | Create an annotated tag with the given message |
| `-F` | `--file` | `<file>` | Create an annotated tag using the file contents as the message |
| `-e` | `--edit` | | Open the annotation message in an editor before creating the tag |
| `-f` | `--force` | | Overwrite an existing tag |
| `-n` | `--n-lines` | `<lines>` | Number of annotation lines to display when listing (0 = names only) |
| | `--points-at` | `<object>` | List only tags pointing at the given object (peeled to its commit); implies list mode |
| | `--contains` | `[commit]` | List only tags whose target commit history contains the commit; defaults to `HEAD` |
| | `--no-contains` | `[commit]` | List only tags whose target commit history does not contain the commit; defaults to `HEAD` |
| | `--merged` | `[commit]` | List only tags merged into the commit; defaults to `HEAD` |
| | `--no-merged` | `[commit]` | List only tags not merged into the commit; defaults to `HEAD` |
| | `--sort` | `<key>` | Sort by `refname`, `-refname`, `creatordate`, or `-creatordate` |

### Flag examples

```bash
# Create a lightweight tag at HEAD
libra tag v1.0

# Create an annotated tag with a message (-a is optional when -m is given)
libra tag -a -m "Release v1.1" v1.1

# Create an annotated tag from a file
libra tag -F RELEASE_NOTES.md v1.2

# Create an annotated tag by editing the message
libra tag -a -e v1.3

# Force-overwrite an existing tag
libra tag -f v1.0

# List all tags
libra tag -l

# List tags with annotation preview (2 lines)
libra tag -l -n 2

# List only tags pointing at HEAD's commit
libra tag --points-at HEAD

# List tags whose target history contains HEAD
libra tag --contains HEAD

# List tags merged into main in reverse refname order
libra tag --merged main --sort=-refname

# Delete one or more tags
libra tag -d v1.0 v1.1

# JSON output for agents
libra tag --json v1.0
```

## Common Commands

```bash
libra tag v1.0                        # Create a lightweight tag at HEAD
libra tag -m "Release v1.1" v1.1      # Create an annotated tag
libra tag -F RELEASE_NOTES.md v1.2    # Create an annotated tag from a file
libra tag -a -e v1.3                  # Edit an annotated tag message
libra tag -l -n 2                     # List tags with up to 2 annotation lines
libra tag --points-at HEAD            # List tags pointing at HEAD's commit
libra tag --contains HEAD             # List tags whose history contains HEAD
libra tag --merged main --sort=-refname # List merged tags in reverse refname order
libra tag -d v1.0 v1.1                # Delete one or more tags
libra tag --json v1.0                 # Structured JSON output for agents
```

## Human Output

- `libra tag -l`: prints the tag list, one per line; with `-n` shows annotation lines indented
- `libra tag v1.0`: `Created lightweight tag 'v1.0' at abc1234`
- `libra tag -m "msg" v1.0`: `Created annotated tag 'v1.0' at abc1234`
- `libra tag -F msg.txt v1.0`: `Created annotated tag 'v1.0' at abc1234`
- `libra tag -d v1.0`: `Deleted tag 'v1.0' (was abc1234)`
- `libra tag -d v1.0 v1.1`: prints one `Deleted tag '<name>'` line per successful deletion
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

Batch delete:

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "delete",
    "name": "v1.0",
    "hash": null,
    "deleted": ["v1.0", "v1.1"],
    "failed": [{ "name": "missing", "reason": "tag 'missing' not found" }]
  }
}
```

`action=list` returns a `tags` array; `action=delete` returns `name` and `hash`.
For recovery deletes of malformed tag refs, `hash` can be `null` when the stored target is missing.
For batch deletes, `name` is the first requested tag and the real per-tag result is in `deleted` / `failed`. If any batch item fails, Libra still writes this JSON to stdout and exits 128.

## Design Rationale

### Why no --sign/-s?

Git's `--sign` flag uses GPG to produce inline PGP signatures embedded in the tag object. Libra omits this for several reasons:

- **GPG key management is fragile**: developers frequently lose keys, let them expire, or misconfigure gpg-agent, leading to broken signing workflows. In CI/CD environments, managing GPG keyrings securely is an operational burden.
- **Vault-based signing is the intended path**: Libra's architecture is designed around a vault-based signing model (see `--vault` on `libra init`) where cryptographic operations are delegated to a secure key store rather than requiring each developer to maintain local GPG keys. This approach centralizes trust and simplifies key rotation.
- **Tag integrity through SQLite**: because tag references live in a transactional database rather than loose files, the tampering surface that GPG signing was designed to mitigate is already reduced. Unauthorized ref modification requires database access rather than just filesystem writes.

### Why no --verify?

Without `--sign`, there are no inline signatures to verify. Future verification will be handled at the vault/trust layer rather than through per-tag GPG checks. This avoids the situation in Git where `git tag -v` fails confusingly when the signer's public key is not in the local keyring.

### Why lightweight vs annotated distinction?

Libra preserves Git's two-tier tag model for on-disk format compatibility. Lightweight tags are simple ref pointers (ideal for temporary markers), while annotated tags store metadata useful for releases. The `-m` flag is the toggle: its presence creates an annotated tag, its absence creates a lightweight one. This matches Git's behavior exactly, keeping the mental model consistent for users migrating from Git.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | Libra | jj |
|---------|-----|-------|----|
| Create lightweight | `git tag <name>` | `libra tag <name>` | `jj tag create <name>` |
| Create annotated | `git tag -a -m "msg" <name>` | `libra tag -a -m "msg" <name>` (or just `-m`) | Not supported (lightweight only) |
| List tags | `git tag -l` | `libra tag -l` | `jj tag list` |
| List with message | `git tag -l -n3` | `libra tag -l -n 3` | N/A |
| Create from file/editor | `git tag -F <file>` / `git tag -e` | `libra tag -F <file>` / `libra tag -e` | N/A |
| List by target | `git tag --points-at <obj>` | `libra tag --points-at <obj>` | N/A |
| List by history | `git tag --contains/--merged` | `libra tag --contains/--merged` | N/A |
| Sort list | `git tag --sort=<key>` | `libra tag --sort=refname/-refname/creatordate/-creatordate` | N/A |
| Delete | `git tag -d <name>...` | `libra tag -d <name>...` | `jj tag delete <name>` |
| Force overwrite | `git tag -f <name>` | `libra tag -f <name>` | `jj tag create <name>` (always overwrites) |
| Sign tag | `git tag -s <name>` | Not supported (vault-based planned) | N/A |
| Verify tag | `git tag -v <name>` | Not supported (vault-based planned) | N/A |
| Structured output | No | `--json` / `--machine` | `--template` |

## Error Handling

| Scenario | Error Code | Hint |
|----------|-----------|------|
| Tag already exists | `LBR-CONFLICT-002` | "delete it first with 'libra tag -d <name>'." |
| HEAD has no commit to tag | `LBR-REPO-003` | "create a commit first before tagging HEAD." |
| Tag not found (delete/show) | `LBR-CLI-003` | "use 'libra tag -l' to list available tags." |
| Unresolvable `--points-at` object | `LBR-CLI-003` | "use 'libra log --oneline' to see available commits." |
| Unresolvable `--contains` / `--merged` commit | `LBR-CLI-003` | "use 'libra log --oneline' to see available commits." |
| Missing tag name for --delete/--message/--file/--edit/--force | `LBR-CLI-002` | "use 'libra tag <name>' to create or update a tag" |
| Multiple tag names outside `--delete` | `LBR-CLI-002` | "use 'libra tag -d <name> [<name>...]' to delete multiple tags." |
| Invalid tag name or unsupported `--sort` key | `LBR-CLI-002` | -- |
| `-a` / `-F` / `-e` combined with `-l`/`-d` | `LBR-CLI-002` | "create an annotated tag with 'libra tag -a -m <message> <name>'" |
| No editor configured for `-a`/`-e` without a message | `LBR-CLI-002` | "set $GIT_EDITOR, core.editor, $VISUAL, or $EDITOR." |
| Failed to read `-F <file>` | `LBR-IO-001` | -- |
| Failed to resolve HEAD | `LBR-IO-001` or `LBR-REPO-002` | -- |
| Failed to serialize annotated tag | `LBR-INTERNAL-001` | -- |
| Failed to store object | `LBR-IO-002` | -- |
| Failed to persist reference | `LBR-IO-002` | -- |
| Failed to delete tag | `LBR-IO-002` | -- |
| Failed to list tags (DB error) | `LBR-IO-001` | -- |
| Failed to list tags (corrupt object) | `LBR-REPO-002` | -- |
