# `libra ls-remote`

List references advertised by a remote repository without downloading objects or updating local refs.

```bash
libra ls-remote [OPTIONS] <repository> [patterns...]
```

`<repository>` can be a configured remote name when run inside a Libra repository, a URL, or a local Git/Libra repository path.

## Options

| Flag | Description | Example |
|------|-------------|---------|
| `--heads` | Show only `refs/heads/*` branch refs | `libra ls-remote --heads origin` |
| `-t`, `--tags` | Show only `refs/tags/*` tag refs | `libra ls-remote --tags origin` |
| `--refs` | Omit `HEAD` and peeled tag refs ending in `^{}` | `libra ls-remote --refs origin` |
| `--symref` | Print symbolic refs as `ref: <target>\t<name>` before the SHA rows | `libra ls-remote --symref origin` |
| `--get-url` | Print the resolved remote URL and exit, **without contacting the remote** | `libra ls-remote --get-url origin` |
| `--sort=<key>` | Sort by `refname`, `-refname`, `version:refname` / `v:refname` (prefix `-` to reverse) | `libra ls-remote --sort=version:refname --tags origin` |
| `--exit-code` | Exit with status `2` (silently) when no ref matches; `0` otherwise | `libra ls-remote --exit-code --heads origin topic` |
| `-o`, `--server-option=<opt>` | Accepted for compatibility; **not yet forwarded** to the server | `libra ls-remote -o key=value origin` |
| `patterns...` | Match full ref names or trailing path components; `*` and `?` follow Git-style glob behavior and can match `/` | `libra ls-remote origin main 'refs/heads/*'` |

### `--symref`

For HTTP/SSH/git remotes, the symbolic refs advertised in the protocol capabilities are
printed first, e.g. `ref: refs/heads/main\tHEAD`. A `ref:` line is printed only when its
**name** (e.g. `HEAD`) passes the active `--heads`/`--tags`/pattern filter.

> **Intentional difference:** for a **local repository path** remote, Libra advertises no
> capabilities, so `--symref` prints no `ref:` lines (unlike `git ls-remote --symref <path>`,
> which reads the local `HEAD`). Use an HTTP/SSH remote to see symrefs.

### `--get-url`

Resolves and prints the target URL using only local config (offline; no client is
constructed and no discovery handshake occurs), then exits `0`. The URL is
**credential-redacted** (a Libra-wide security invariant, unlike `git`'s verbatim print),
and `url.<base>.insteadOf` rewriting is **not** applied. An unconfigured, non-URL token is
echoed verbatim and exits `0` (git parity).

### `--sort=<key>`

Supports `refname`/`-refname` (lexical) and `version:refname`/`v:refname` (natural version
order, where digit runs compare numerically). Other git `for-each-ref` keys (e.g.
`objectname`, `creatordate`) are **unsupported** — ls-remote only has hash + refname — and
are rejected with `LBR-CLI-002` (exit 129). Without `--sort`, the remote's advertised order
is preserved (no implicit sorting).

## Human Output

Each matching ref is printed as:

```text
<object-id>	<refname>
```

Example:

```text
4f3c2d1a...	HEAD
4f3c2d1a...	refs/heads/main
```

## JSON Output

With `--json`, output uses the standard command envelope:

```json
{
  "ok": true,
  "command": "ls-remote",
  "data": {
    "remote": "origin",
    "url": "https://example.com/repo.git",
    "heads_only": false,
    "tags_only": false,
    "refs_only": false,
    "patterns": [],
    "entries": [
      {
        "hash": "4f3c2d1a...",
        "refname": "refs/heads/main"
      }
    ]
  }
}
```

## Examples

```bash
# List all refs from a named remote
libra ls-remote origin

# List all refs from a URL directly (no remote registration required)
libra ls-remote https://example.com/repo.git

# Restrict to branches matching a pattern
libra ls-remote --heads origin main

# Structured JSON envelope for agents, tags only
libra --json ls-remote --tags origin
```

The same banner is rendered by `libra ls-remote --help` so the doc and
the CLI surface stay in sync (cross-cutting `--help` EXAMPLES rollout,
see `docs/improvement/README.md` item B).

## Notes

- `ls-remote` performs only protocol discovery (`git-upload-pack --advertise-refs` equivalent for local Git repositories).
- It does not write objects, remote-tracking refs, config, or working-tree files.
- `--heads` and `--tags` can be combined to show both branch and tag refs while excluding `HEAD`.
- With `--symref`, the JSON `data` gains a `symrefs` map (`{"HEAD": "refs/heads/main"}`), omitted when empty.

## Compatibility

| Feature | Status |
|---------|--------|
| `--heads`/`--tags`/`--refs`/patterns | supported |
| `--exit-code` | supported (silent exit `2` on no match) |
| `--symref` | supported for HTTP/SSH/git remotes; **intentionally-different** for local paths (no `ref:` lines) |
| `--get-url` | partial — offline + credential-redacted; **no `insteadOf` rewriting** |
| `--sort` | partial — `refname`/`version:refname` subset only |
| `-o`/`--server-option` | partial — parsed but not yet forwarded to the server |
| `--upload-pack`, `-b/--branches`, per-command `-q`, default `<repository>` | unsupported / intentionally-different (use the documented equivalents) |
