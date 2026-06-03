# `libra config`

`libra config` manages repository-local and user-global configuration stored in SQLite-backed
`config_kv`, including vault-backed secrets and key management.

**Alias:** `cfg`

## Synopsis

```
libra config <subcommand> [options]
libra config set [--global] [--add] [--encrypt] [--plaintext] [--stdin] <key> [<value>]
libra config get [--global] [--all] [--reveal] [--regexp] [-d <default>] <key>
libra config list [--global] [--name-only] [--show-origin] [--vault] [--ssh-keys] [--gpg-keys]
libra config unset [--global] [--all] <key>
libra config import [--global]
libra config path [--global]
libra config generate-ssh-key --remote <name>
libra config generate-gpg-key [--name <name>] [--email <email>] [--usage <usage>]
```

Git-compatible flag style is also supported (hidden from help):

```
libra config [--get | --get-all | --unset | --unset-all | -l | --add | --import | --get-regexp | --show-origin] [--local | --global] [key] [value] [-d <default>]
```

## Description

`libra config` reads and writes configuration values across two scopes: **local** (repository-level, stored in `.libra/libra.db`) and **global** (user-level, stored in `~/.libra/config.db`). Both databases use SQLite with a `config_kv` table.

Unlike Git's plaintext INI files or jj's TOML files, Libra stores configuration in a transactional database with integrated vault encryption. Sensitive values (API keys, tokens, SSH private keys) are automatically encrypted at rest using AES-256-GCM.

The command supports two invocation styles:

1. **Subcommand style** (preferred): `libra config set key value`, `libra config get key`
2. **Git-compatible flag style** (hidden): `libra config --get key`, `libra config key value`

When reading a value with `get`, Libra cascades through scopes in precedence order: local, then global. The first match wins.

## Options

### Subcommands

#### `set <key> [<value>]`

Set a configuration value. If `<value>` is omitted and the key is sensitive, Libra prompts for interactive input (hidden echo). In non-interactive contexts (CI/CD), use `--stdin` to pipe the value.

| Flag | Description |
|------|-------------|
| `--add` | Add as an additional value for the key, allowing duplicates (like Git's multi-valued keys such as `remote.origin.fetch`) |
| `--encrypt` | Force vault encryption even if the key does not match sensitive-key heuristics |
| `--plaintext` | Force plaintext storage, skipping auto-encryption even for sensitive-looking keys |
| `--stdin` | Read the value from stdin instead of a positional argument (useful for piping secrets in CI/CD) |

```bash
# Basic set
libra config set user.name "Jane Doe"

# Set global config
libra config set --global user.email "jane@example.com"

# Force encryption
libra config set --encrypt custom.api_token "sk-abc123"

# Set from stdin (CI/CD)
echo "$SECRET" | libra config set --stdin vault.env.GEMINI_API_KEY

# Add multi-value key
libra config set --add remote.origin.fetch "+refs/heads/*:refs/remotes/origin/*"

# Sensitive key prompts interactively when value omitted
libra config set vault.env.GEMINI_API_KEY
```

#### `get <key>`

Retrieve a configuration value. Cascades from local to global scope, returning the first match.

| Flag | Description |
|------|-------------|
| `--all` | Return all values for this key (multi-valued keys) |
| `--reveal` | Show the actual decrypted value for encrypted entries (blocked for internal vault credentials like `vault.roottoken_enc`) |
| `--regexp` | Treat `<key>` as a regex pattern and return all matching entries |
| `-d`, `--default <value>` | Return this value if the key is not found (instead of an error) |

```bash
# Simple get
libra config get user.name

# Get with default fallback
libra config get -d "unknown" user.name

# Get all values for a multi-value key
libra config get --all remote.origin.fetch

# Reveal an encrypted value
libra config get --reveal vault.env.GEMINI_API_KEY

# Regex search
libra config get --regexp "user\\..*"
```

#### `list`

List all configuration entries in the active scope.

| Flag | Description |
|------|-------------|
| `--name-only` | Show only key names, not values |
| `--show-origin` | Prefix each entry with its `file:<path>` SQLite origin |
| `--show-scope` | Prefix each entry with its `local`/`global` scope label |
| `--null` / `-z` | NUL-delimit records (Git `key\nvalue\0` format) |
| `--vault` | Show only `vault.env.*` entries |
| `--ssh-keys` | Show SSH key entries |
| `--gpg-keys` | Show GPG key entries |

```bash
# List all local entries
libra config list

# List with scope labels
libra config list --show-origin

# List only vault environment entries
libra config list --vault

# List only key names
libra config list --name-only

# List SSH keys
libra config list --ssh-keys
```

#### `unset <key>`

Remove a configuration entry.

| Flag | Description |
|------|-------------|
| `--all` | Remove all values for this key (for multi-valued keys) |

```bash
# Remove a key
libra config unset user.signingkey

# Remove all values for a multi-valued key
libra config unset --all remote.origin.fetch
```

#### `import`

Import configuration from the user's Git config (`.gitconfig`). Copies relevant entries into Libra's config database.

```bash
# Import from Git global config into Libra global config
libra config import --global

# Import into local config
libra config import
```

#### `path`

Print the filesystem path of the config database for the active scope.

```bash
# Show local config path
libra config path
# Output: /path/to/repo/.libra/libra.db

# Show global config path
libra config path --global
# Output: /home/user/.libra/config.db
```

#### `edit`

Not supported. Libra uses SQLite storage, which cannot be safely round-tripped through a text editor. See [Design Rationale](#design-rationale-why-different-from-gitjj) for details.

#### `generate-ssh-key --remote <name>`

Generate an SSH key pair for the named remote. The private key is stored encrypted in the vault (`vault.ssh.<remote>.privkey`); the public key is stored at `vault.ssh.<remote>.pubkey`.

```bash
libra config generate-ssh-key --remote origin
libra config get vault.ssh.origin.pubkey
```

#### `generate-gpg-key`

Generate a GPG key pair for commit signing or encryption.

| Flag | Description |
|------|-------------|
| `--name <name>` | User name for the key (defaults to `user.name` config) |
| `--email <email>` | User email for the key (defaults to `user.email` config) |
| `--usage <usage>` | Key usage: `signing` (default) or `encrypt` |

```bash
# Generate signing key
libra config generate-gpg-key

# Generate encryption key with explicit identity
libra config generate-gpg-key --name "Jane Doe" --email "jane@example.com" --usage encrypt

# Retrieve the public key
libra config get vault.gpg.pubkey
```

### Scope Flags

These flags are global (apply to any subcommand):

| Flag | Description |
|------|-------------|
| `--local` | Use repository config (`.libra/libra.db`). This is the default for writes. |
| `--global` | Use global user config (`~/.libra/config.db`). |
| `--system` | **Removed.** Always produces an error. See Design Rationale. |

### Hidden Git-Compatible Flags

These flags provide backward compatibility with `git config` invocation patterns. They are hidden from `--help` and internally translated to the equivalent subcommand.

| Flag | Equivalent Subcommand |
|------|----------------------|
| `--get` | `get <key>` |
| `--get-all` | `get --all <key>` |
| `--unset` | `unset <key>` |
| `--unset-all` | `unset --all <key>` |
| `-l`, `--list` | `list` |
| `--add` | `set --add <key> <value>` |
| `--import` | `import` |
| `--get-regexp` | `get --regexp <key>` |
| `--show-origin` | `list --show-origin` |

### Other Flags

| Flag | Description |
|------|-------------|
| `-d`, `--default <value>` | Default value when key is not found (Git-compat positional mode) |
| `--json` | Emit structured JSON output |
| `--quiet` | Suppress human-readable output |

## Common Commands

```bash
libra config set user.name "Jane Doe"
libra config get user.name
libra config list
libra config list --show-origin
libra config unset user.signingkey
libra config import
libra config path
```

## Human Output

**`get`** prints the value on a single line:

```
Jane Doe
```

**`list`** prints key-value pairs:

```
user.name=Jane Doe
user.email=jane@example.com
core.editor=vim
```

With `--show-origin` (the `file:<path>` SQLite origin, tab-separated before the record):

```
file:/home/user/repo/.libra/libra.db	user.name=Jane Doe
file:/home/user/.libra/config.db	user.email=jane@example.com
```

With `--show-scope` (the `local`/`global` scope label):

```
local	user.name=Jane Doe
global	user.email=jane@example.com
```

With `--null` (Git record format `key\nvalue\0`, value NUL-terminated):

```
user.name\nJane Doe\0user.email\njane@example.com\0
```

With `--name-only`:

```
user.name
user.email
core.editor
```

**`set`** prints nothing on success (exit code 0).

**`path`** prints the database path:

```
/home/user/repo/.libra/libra.db
```

## Structured Output (JSON examples)

**`get`:**

```json
{
  "command": "config",
  "data": {
    "key": "user.name",
    "value": "Jane Doe",
    "origin": "local"
  }
}
```

**`list`:**

```json
{
  "command": "config",
  "data": {
    "entries": [
      { "key": "user.name", "value": "Jane Doe", "origin": "local" },
      { "key": "user.email", "value": "jane@example.com", "origin": "global", "encrypted": false }
    ]
  }
}
```

## Secrets And Vault Entries

Sensitive keys are stored encrypted when they match Libra's sensitive-key rules, including:

- `vault.env.*`
- `*.privkey`
- API keys, tokens, passwords, and similar secret-looking keys

Examples:

```bash
libra config set vault.env.GEMINI_API_KEY
echo "$SECRET" | libra config set --stdin vault.env.GEMINI_API_KEY
libra config set --encrypt custom.api_token "secret"
libra config get vault.env.GEMINI_API_KEY
libra config get --reveal vault.env.GEMINI_API_KEY
libra config list --vault
```

`--reveal` is blocked for internal vault credentials such as `vault.roottoken_enc` and
`vault.ssh.<remote>.privkey`.

## Key Management

SSH keys are generated per remote and stored in config:

```bash
libra config generate-ssh-key --remote origin
libra config get vault.ssh.origin.pubkey
libra config list --ssh-keys
```

GPG public keys are exposed through config, while private signing material stays inside `vault.db`:

```bash
libra config generate-gpg-key
libra config generate-gpg-key --usage encrypt
libra config get vault.gpg.pubkey
libra config list --gpg-keys
```

Supported `--usage` values are `signing` and `encrypt`.

## Scope

- Default scope is local (`.libra/libra.db`)
- `--global` uses `~/.libra/config.db`
- `--system` is removed (see Design Rationale); migrate old usages to `--global`

Resolution order for runtime config-backed environment variables is:

1. CLI arguments
2. Local config (`vault.env.<NAME>`)
3. Global config (`vault.env.<NAME>`)
4. Process environment variables

If no Vault entry or process environment variable supplies a required API key,
Libra reports the missing key and asks you to set `vault.env.<NAME>` or export
`<NAME>`.

## Design Rationale (Why different from Git/jj)

### Why SQLite instead of text files?

Git uses INI-format text files; jj uses TOML. Libra uses SQLite because:

1. **Transactional writes.** SQLite provides ACID guarantees. A crash mid-write cannot corrupt the configuration, unlike a partially-written text file. This is critical when multiple AI agents may write config concurrently.
2. **Structured queries.** Multi-valued keys, prefix searches, and regex matching are SQL queries rather than text parsing. This eliminates an entire class of escaping and parsing bugs.
3. **Integrated encryption.** Vault-encrypted values are stored as encrypted blobs alongside plaintext values in the same table. A text file format would need a separate encryption layer or inline encoding scheme.

### Why vault encryption?

Git stores configurations in plaintext INI files, which is inherently insecure for storing API keys, access tokens, and SSH/GPG private keys. Libra integrates Vault-backed encrypted storage natively. Sensitive keys (like `vault.env.*`, `*.privkey`, or keys containing substrings like `secret`/`token`) are automatically encrypted at rest using AES-256-GCM in both local and global scopes. This eliminates the "redacted in CLI but plaintext on disk" false sense of security, allowing developers to safely store environment overrides directly within the configuration.

### Why no `--system` scope?

System-level configuration (`--system`) is intentionally removed. In a multi-user OS environment, sharing an encrypted vault at the system level introduces severe permission isolation issues. For example, an unseal key readable only by `root` would cause cascaded config reading to fail for regular users, crashing their commands. The operational complexity and security risks far outweigh the benefits. System-wide defaults should be handled at the OS/environment level, while Libra uses `--global` for user-level defaults.

### Why no `config edit`?

Libra uses a SQLite database (`config_kv` table) instead of plaintext files. Exporting database rows to a text editor and parsing the unified diff back into SQL `UPDATE`/`DELETE` statements is dangerous. Specifically, for multi-value keys (e.g., `remote.origin.fetch`), the plaintext representation lacks row-level primary keys. Reordered, partially modified, or deleted lines would prevent Libra from accurately mapping text changes to database rows, inevitably leading to data loss or corruption. To guarantee data consistency, you must use the robust `set`, `--add`, `unset`, and `list` commands.

### Why built-in SSH/GPG key management?

Instead of scattering SSH private keys as plaintext files on the filesystem, Libra stores them encrypted inside the config vault (`vault.ssh.<remote>.privkey`). When an SSH transport is invoked, the key is dynamically decrypted to a temporary file (`chmod 600`), passed to the SSH client, and deleted immediately afterward. GPG private keys are managed exclusively by the vault's internal PKI engine and are never exported to the filesystem.

### Why subcommand style as the primary interface?

Git uses `git config key value` (implicit set) and `git config key` (implicit get), which is ambiguous: `git config foo` could be a get or an incomplete set. Libra follows jj's lead by requiring explicit subcommands (`set`, `get`, `list`, `unset`). The Git-compatible flag style (`--get`, `-l`, etc.) is preserved as hidden aliases for migration, but the subcommand style is the documented interface because it is unambiguous, discoverable via `--help`, and easier for AI agents to generate correctly.

### Why `--default` instead of exit-code differentiation?

Git exits with code 1 when a key is not found, which is indistinguishable from other errors in scripts. Libra's `--default` flag provides an explicit fallback value, allowing scripts and agents to handle missing keys without error-code parsing.

## Git Config Compatibility Matrix

This matrix tracks how `libra config` aligns with `git config` (Git 2.54.0 baseline). The
goal is **high-value script compatibility**, not a byte-for-byte clone: capabilities that
conflict with Libra's SQLite/vault-backed model are recorded as *intentional differences* or
*deferred* rather than silently emulated.

Status legend: **implemented** · **partial** · **deferred** (recognised, fails fast with a clear
message) · **intentional difference** (deliberately not emulated) · **not applicable**.

| Git capability | Libra status | Notes |
|---|---|---|
| `--get`, `--get-all`, `get`, `get --all` | implemented | Cascade local→global; last-one-wins for single get |
| `set`, legacy positional set | implemented | Plaintext, auto-encrypted, and `--stdin` values |
| `--list` / `-l`, `list` | implemented | Supports `--name-only` |
| `--unset`, `--unset-all` | implemented | Multi-value protection (exit 5 on ambiguity) |
| `--local`, `--global` | implemented | Local default; global at `~/.libra/config.db` |
| `--null` / `-z` | implemented | Git record format `key\nvalue\0` (text output only) |
| `--show-origin` | implemented | Emits `file:<path>` SQLite origin (not a scope label) |
| `--show-scope` | implemented | Emits `local`/`global` scope label |
| `--name-only` (list / legacy `--get-regexp`) | implemented | Single-key modern `get --name-only` → exit 129 |
| `--get-regexp` | implemented | Key regex; text output is Git-style `key value` |
| `--append` / legacy `--add` / `set --add` | implemented | Append a value without replacing |
| `--replace-all` / `set --all` | implemented | Transactional replace of all matching values |
| `--value=<pattern>` | implemented | Regex (or literal with `--fixed-value`) value filter for get/set/unset |
| `--fixed-value` | implemented | Literal `--value` matching; disables regex/negation parsing |
| `--ignore-case` / `-i` | implemented | Case-insensitive key/value regex matching |
| `rename-section`, `--rename-section` | implemented | Generic dotted-prefix move (no remote-specific cascade) |
| `remove-section`, `--remove-section` | implemented | Generic dotted-prefix delete |
| `--type=bool\|int\|path` (+ `--bool`/`--int`/`--path`) | implemented | Output canonicalisation + write validation |
| `--default` | partial | get / get-all / get-regexp only; mutation combos → exit 129 |
| `--import` (Libra extension) | implemented | Reads `git config --list -z`; not a Git capability |
| vault-backed secrets, SSH/GPG key generation | Libra-only | Not a Git capability |
| `--type=bool-or-int\|expiry-date\|color` | deferred | exit 129; semantics lower value for Libra core flows |
| `--type` + `--list` | deferred | exit 129; Git silently drops non-canonicalisable entries |
| `--no-type`, `--no-value`, `--show-names`/`--no-show-names` | deferred | exit 129; multi-flag clear/show state machines not implemented |
| `--get-color`, `--get-colorbool` | deferred | exit 129; legacy color helpers |
| `--url`, `--get-urlmatch` | deferred | exit 129; advanced URL matching |
| `--system` | intentional difference | exit 129; no privileged system writes (see Design Rationale) |
| `--worktree` | intentional difference | exit 129; worktree-scoped config model not designed |
| `--file <path>` / `-f` | intentional difference | exit 129; use `libra config --import` to ingest a Git file |
| `--blob <blob>` | intentional difference | exit 129; blob-backed config is not authoritative storage |
| `--includes` / `--no-includes` / `includeIf` | intentional difference | exit 129; include graphs conflict with SQLite-backed config |
| `edit` | intentional difference | SQLite store cannot be safely text-edited |

> **No SQL schema migration is required for this roadmap.** All new behaviour (multi-value
> filtering, section operations, typed values, output flags) uses the existing
> `config_kv(id, key, value, encrypted)` table; complex mutations are made atomic at the
> application layer via explicit sea-orm transactions, not new DDL.

## Decision Ledger

This ledger records the binding decision for each Git config capability. Script-visible
behaviour (stdout / stderr / exit codes) is contractually stable once marked *supported*.

| Capability | Decision | Behaviour |
|---|---|---|
| `--null` / `-z` | supported | Text output only. Value is NUL-terminated; key/value separated by `\n` (Git-style `key\nvalue\0`), **not** `key=value\0`. With JSON output → exit 129. With `--stdin` → exit 129 (does not parse stdin). |
| `--show-origin` | supported | Adds `file:<path>` origin (local → repo `.libra/libra.db`, global → resolved global DB). Never emits a `local`/`global` scope label as the origin. |
| `--show-scope` | supported | Adds a `local`/`global` scope label. Cascaded get reports the winning scope. |
| `--name-only` | supported (list + legacy regexp) | Records contain only keys. Legacy single-key `--get <key> --name-only` → exit 129; modern `get --name-only` deferred. |
| `--append` / `--add` | supported | Append a value; never replaces existing values. `set --add` remains as Libra subcommand UX. |
| `--replace-all` / `set --all` | supported | Transactionally replace all matching values for a key. No-match → insert and exit 0. |
| `--value=<pattern>` | supported | Regex value filter on get/set/unset. Invalid regex → exit 6. Leading `!` negates (unless `--fixed-value`). Honours the sensitive/encrypted interaction contract below. |
| `--ignore-case` / `-i` | supported | Case-insensitive key regex and `--value` regex matching (not literal `--fixed-value`). |
| `--fixed-value` | supported | `--value` is matched literally; value regex/negation parsing disabled. Key regex length limit still applies. |
| `rename-section` / `remove-section` | supported | Modern subcommands + legacy `--rename-section` / `--remove-section`. Section maps to a dotted prefix (`remote.origin` → `remote.origin.`). |
| `--type=bool\|int\|path` (+ `--bool`/`--int`/`--path`) | supported | Output canonicalisation; write validation for bool/int. Integers accept case-insensitive `k`/`m`/`g` suffixes (1024-based) and emit canonical decimal. |
| `--no-value` / `--show-names` / `--no-show-names` / `--no-type` | deferred | exit 129 with an explanatory message. |
| `--type=bool-or-int\|expiry-date\|color` | deferred | exit 129 (type not yet supported). |
| `--type` + `--list` | deferred | exit 129 (avoids silently dropping non-canonicalisable entries). |
| `--system` / `--worktree` | rejected | exit 129; no system/worktree config writes. |
| `--file` / `-f` | rejected | exit 129; suggests `libra config --import`. Never reads an arbitrary plaintext file. |
| `--blob` | rejected | exit 129; blob-backed config unsupported. |
| `--includes` / `--no-includes` | rejected | exit 129; include graphs are not SQLite-backed authoritative config. |
| `--url` / `--get-urlmatch` / `--get-color` / `--get-colorbool` | deferred | exit 129; advanced/legacy helpers. |
| `--default` combos | current scope | get / get-all / get-regexp only; mutation combos → exit 129. |
| Missing key exit code | Git-like exit 1 | `--get missing.key` (no `--default`) exits 1 with empty stdout. |
| JSON interaction | structured JSON | JSON rejects `--null` and `--name-only` (exit 129). Existing `origin` field keeps its scope meaning; `scope`/`origin_type`/`origin_path` carry precise data. |

### `--show-origin` vs `--show-scope`

These are **distinct** and must never be conflated:

- `--show-origin` emits a Git-style **origin path**: `file:<absolute-path>` where the path is
  the SQLite database file backing the value (`.libra/libra.db` for local, the resolved global
  DB for global). On Windows the `file:<path>` form is fixed by documentation and tests; if URI
  normalisation is not implemented, the platform difference is recorded rather than left to vary.
- `--show-scope` emits a **scope label**: `local` or `global`.

In text mode the prefix fields precede `key`/`value`, tab-separated (or NUL-separated under
`--null`). In JSON mode they appear as the `origin_type`/`origin_path` and `scope` fields.

### JSON backward compatibility

Existing JSON fields **do not change meaning**. In particular the `origin` field continues to
carry the **scope label** (`local`/`global`). Git-style origin paths are exposed only through the
new `origin_type` (`"file"`) and `origin_path` fields, and the scope label is also surfaced via
the new `scope` field. New fields appear only when `--show-origin`/`--show-scope` is requested
(`skip_serializing_if`). No existing field is renamed, removed, or repurposed.

## Value Filtering and Sensitive/Encrypted Values

`--value` / `--fixed-value` are intended for public multi-value config (e.g. `remote.*.fetch`
refspecs). For a **sensitive key** (`is_sensitive_key(key)` true, or a row stored with
`encrypted != 0`):

- Without `--reveal`, the filter matches against the **stored ciphertext** (or redacted form);
  output entries are still redacted (`<REDACTED>`) unless `--reveal` is given and the key is not a
  vault internal.
- With `--reveal` (and decryption permitted), the value is decrypted before pattern matching;
  decryption/canonicalisation error messages never include the secret or ciphertext.
- Section rename/remove may move ordinary auto-encrypted sensitive keys (ciphertext and the
  `encrypted` flag travel with the key name), but any `vault.*` section is rejected and managed
  only by dedicated vault/config commands.

> **Shell history warning:** a `--value` pattern typed on the command line may be recorded in
> shell history. Avoid `--value` on sensitive keys; prefer an exact key or a dedicated vault
> command. Libra emits a one-time stderr warning when `--value` is used on a sensitive key.

All user-supplied regexes (key and value) are bounded to **4 KiB** and evaluated by Rust's
linear-time `regex` engine (no catastrophic backtracking); an over-long or invalid regex exits 6.

## Transaction Boundaries and Helper Split

Complex mutations (`--value`-filtered replace/unset, `rename-section`, `remove-section`) run
inside an **explicit transaction** in the command layer (`conn.begin()` →
`*_with_conn(&txn, …)` → `txn.commit()`), so a validation failure, conflict, or write error
leaves **no partial change** (the transaction is dropped without commit). Value matchers, section
mutators, and type canonicalisers live in `src/internal/config.rs` as testable `_with_conn`
helpers (which never `begin`/`commit` themselves); the command layer only parses, orchestrates,
renders, and maps errors. SQLite write failures — including `database is locked`/`SQLITE_BUSY`
and `SQLITE_FULL` (disk full) — map to exit 4 with an actionable message and no committed change.
The 30-second busy-timeout configured by `establish_connection_with_busy_timeout` makes lock
contention rare.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | jj | Libra |
|---------|-----|-----|-------|
| Implicit set | `git config key val` | No (requires `set`) | `libra config set key val` plus compatible `libra config key val` |
| Subcommand style | No | Yes (`set/get/list/edit/path`) | Yes (`set/get/list/unset/import/path`) |
| Get value | `git config key` | `jj config get key` | `libra config get key` |
| List | `git config -l` | `jj config list` | `libra config list` |
| Edit in editor | `git config -e` | `jj config edit` | Not supported (SQLite storage) |
| Regex search | `git config --get-regexp` | No | `libra config get --regexp` |
| Show origin | `git config --show-origin` | No | `libra config list --show-origin` |
| Type coercion | `--type=bool\|int\|path` | No (TOML types) | **`--type=bool\|int\|path`** (+ `--bool`/`--int`/`--path`) |
| Default fallback | `--default value` | No | `--default value` |
| Null-delimited | `-z` | No | **`-z` / `--null`** (`key\nvalue\0`) |
| Rename/remove section | Yes | No | **`rename-section` / `remove-section`** (generic dotted prefix) |
| JSON output | No | No | **`--json`** |
| Secret redaction | No | No | **Auto-detect** |
| Import from Git | N/A | N/A | **`libra config import`** |
| Vault encryption | No | No | **AES-256-GCM (all scopes)** |
| Env var vault | No | No | **`vault.env.*`** |
| SSH key per remote | No | No | **`generate-ssh-key --remote`** |
| GPG key generation | No | No | **`generate-gpg-key`** |
| Env var resolution | No fallback | No fallback | **CLI -> env -> repo -> global** |
| Config file path | N/A | `jj config path` | **`libra config path`** |
| Conditional config | `includeIf` | `[[when]]` blocks | Not supported |
| Worktree scope | `--worktree` | `--workspace` | Not supported |
| Arbitrary file | `--file <path>` | No | Not supported |
| Storage format | INI text files | TOML text files | **SQLite + vault** |
| Scopes | system/global/local/worktree | user/repo/workspace | **global/local** (system removed) |
| Name-only listing | `--name-only` | No | **`--name-only`** |
| Multi-value add | `--add` | No | **`set --add`** |
| Stdin input | No | No | **`set --stdin`** |
| Force encrypt | No | No | **`set --encrypt`** |
| Force plaintext | No | No | **`set --plaintext`** |

## Error Handling

| Code | Condition | Hint |
|------|-----------|------|
| `LBR-REPO-001` | Not inside a libra repository (for local scope) | Initialize with `libra init` or use `--global` |
| `LBR-CLI-002` | `--system` scope used (removed) | Use `--global` for user-level defaults |
| `LBR-CLI-003` | Key not found and no `--default` provided | Check key name with `libra config list` |
| `LBR-CLI-002` | `edit` subcommand used (not supported) | Use `set`, `get`, `unset`, `list` subcommands |
| `LBR-IO-001` | Failed to read config database | Check file permissions on `.libra/libra.db` |
| `LBR-IO-002` | Failed to write config database | Check file permissions and disk space |

## Compatibility Notes

- `libra vault` has been removed. Use `libra config generate-ssh-key`,
  `libra config generate-gpg-key`, and `libra config get vault.*` instead.
- `libra config edit` is not supported (see Design Rationale above).
- Old repositories may still contain legacy `vault.gpg_pubkey` entries; new writes use
  `vault.gpg.pubkey`.
