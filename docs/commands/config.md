# `libra config`

`libra config` manages repository-local and user-global configuration stored in SQLite-backed
`config_kv`, including vault-backed secrets and key management.

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
| `--show-origin` | Prefix each entry with its scope (`local` or `global`) |
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

With `--show-origin`:

```
local   user.name=Jane Doe
global  user.email=jane@example.com
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
2. Process environment variables
3. Local config (`vault.env.<NAME>`)
4. Global config (`vault.env.<NAME>`)

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
| Type coercion | `--type=bool\|int\|path` | No (TOML types) | Not supported (this batch) |
| Default fallback | `--default value` | No | `--default value` |
| Null-delimited | `-z` | No | Not supported (this batch) |
| Rename/remove section | Yes | No | Not supported (this batch) |
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
