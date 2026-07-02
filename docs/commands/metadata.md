# `libra metadata`

Branch/repo metadata key-value store (a Libra extension, lore.md §1.5 — the
foundation for branch protect/archive/lineage). The nearest Git analogue is
`git config branch.<name>.*`.

## Synopsis

```
libra metadata get   <key>         (--branch <name> | --repo)
libra metadata set   <key> <value> (--branch <name> | --repo)
libra metadata unset <key>         (--branch <name> | --repo)   # alias: clear
libra metadata list                (--branch <name> | --repo) [--prefix <p>]
```

## Description

Metadata is scoped: exactly one of `--branch <name>` / `--repo` /
`--revision <rev>` is required.

- **Branch scope** (`--branch <name>`): key-values attached to a LOCAL branch,
  stored in the `metadata_kv` SQLite table. Metadata follows the branch through
  its lifecycle: `branch -m` moves it, `branch -c`/`-C` copies it (a forced copy
  replaces the destination's metadata, matching the ref overwrite), and
  deleting the branch removes it. The branch must exist for every verb.
  Remote-tracking branches carry no metadata.
- **Repo scope** (`--repo`): repository-level key-values, stored in the
  `config` store under the `metadata.*` namespace — so
  `libra config --get metadata.<key>` sees the same value (an intended dual
  surface; `libra metadata --repo` is the recommended door for plain values).
  Sensitive-looking keys (e.g. `metadata.apitoken`) and keys whose existing
  value is encrypted are **refused by `set --repo`** with a hint to use the
  config door instead (`libra config metadata.<key> <value>`), which owns the
  vault-encryption decision — writing here would either store a secret
  unencrypted or corrupt an encrypted row. `get`/`list` render encrypted
  values as `<REDACTED>` (use `libra config --get --reveal metadata.<key>` to
  decrypt); `unset` works on any key. A key given multiple values via
  `config --add` is refused by `set`/`unset` with a hint to `config unset-all`
  first; `get` returns the most recent value.

- **Revision scope** (`--revision <rev>`): metadata on a commit. Commits are
  immutable, so this scope merges two layers: the commit message's **trailer
  block** (read-only, parsed with Git's rules — the same engine as
  `log --trailer`) and a mutable **notes layer** (one JSON document per commit
  under `refs/notes/metadata`; `libra notes --ref metadata` is an intended
  dual surface). Reads prefer the notes layer; `get`/`list` report a `source`
  (`note`/`trailer`) in JSON. Writes (`set`/`unset`) touch only the notes
  layer — unsetting a trailer-only key exits 1 with an amend/reword hint, and
  removing a note entry that shadowed a trailer prints a notice that the
  trailer value is visible again. Key matching is ASCII **case-insensitive**
  in this scope (the trailer convention; branch/repo stay exact). Note-layer
  values are **local-only** (notes are never pushed) — another clone sees the
  commit's trailers but not your overrides. The JSON `target` is always the
  full resolved commit OID. The whole per-commit document is bounded at 1 MiB.

Well-known branch keys — `protect`, `archive`, and the `lineage.*` prefix — are
**recorded but not yet enforced**: setting them prints a notice. Enforcement
lands once, in the future branch-policy layer (lore.md 1.13), which will read
these keys fail-closed (a corrupted value counts as protected, never silently
unprotected).

Values are text by default; `set --branch` also accepts **typed values**
(lore.md 1.10): `--numeric` (an integer or finite decimal — no surrounding
whitespace; validated at set time, stored exactly as given) and `--binary` (the
VALUE argument is standard base64 — the encoded text is stored, so raw
payloads cap at ~3/4 of the 1 MiB value limit; decode with `| base64 -d`).
`get`/`list`/JSON report the stored `value_type`. Typed flags are refused for
`--repo` (the config store is text-only; a documented follow-up). The empty
string is a legal value, distinct from an absent key. Keys are exact and
case-sensitive (max 256 bytes, no whitespace); values are capped at 1 MiB.

## Options

| Option | Description |
|--------|-------------|
| `get <key>` | Print the value. Exits 1 when the key is absent (like `config` key misses). |
| `set <key> <value>` | Create or overwrite. `--json` reports the `previous` value on overwrite and the `value_type`. |
| `--numeric` / `--binary` | (`set --branch` only) Declare the value's type; mutually exclusive. Validation failures exit 129. |
| `unset <key>` | Remove the key (alias: `clear`). Exits 1 when nothing was removed. |
| `list` | Print `key=value` lines, key-ordered. |
| `--branch <NAME>` | Operate on a local branch's metadata. |
| `--repo` | Operate on repository-level metadata (`config` `metadata.*`). |
| `--revision <REV>` | Operate on a commit's metadata (immutable trailers + a mutable local notes layer; see above). |
| `--prefix <P>` | (`list` only) Only keys starting with the prefix, e.g. `lineage.`. |
| `--json` / `--machine` | Structured envelope: `{ action, scope, target, key, value, ... }`. |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. |
| `1` | `get`/`unset` on an absent key. |
| `128` | Not a repository. |
| `129` | Usage errors: missing/duplicate scope, invalid key, oversized value, unknown branch (`LBR-CLI-002`/`LBR-CLI-003`). |

## Examples

```bash
# Record branch protection (not yet enforced) and read it back.
libra metadata set protect true --branch main
libra metadata get protect --branch main

# Lineage records under a key prefix.
libra metadata set lineage.parent dev --branch feature
libra metadata list --branch feature --prefix lineage.

# Repo-level metadata, visible through the config surface too.
libra metadata set owner platform-team --repo
libra config --get metadata.owner

# Structured output for agents.
libra --json metadata list --branch main
```

## Comparison with Git

Git has no first-class metadata store; the closest analogues are
`git config branch.<name>.*` (per-branch config) and `git notes` (per-object
annotations). `libra metadata` is classified `intentionally-different` in
[`COMPATIBILITY.md`](../../COMPATIBILITY.md). Metadata is local-only: it is
never pushed, pulled, or published.
