# Changelog

## [0.1.6]

### Breaking Changes

- **`libra init --separate-libra-dir` and `--separate-git-dir` removed**: non-bare repositories now always use the standard `.libra/` directory inside the worktree. Historical repositories that still use a `.libra` `gitdir:` link file are no longer detected. Migration:
  ```bash
  rm .libra
  mv /path/to/separate/storage .libra
  ```

### Changed

- **`libra init` execution/render split**: init now uses a silent execution layer internally so `clone` and other callers no longer leak init progress or JSON envelopes.
- **Human progress output**: default `libra init` now reports major phases (`Creating repository layout`, `Initializing database`, `Setting up refs`, Git conversion, vault key generation) on `stderr`.
- **Structured success output**: `libra init` now supports stable `--json` / `--machine` success envelopes with path, branch, object/ref format, repo id, vault state, Git conversion source, and SSH-key detection.
- **Git import cleanup**: `--from-git-repository` now uses the safe fetch path and suppresses nested fetch progress/JSON noise from `stderr`.
- **Vault identity alignment**: init now resolves signing identity from target-local config, global config, and commit-compatible environment fallbacks before using the built-in default identity.
- **Explicit `vault.signing=false`**: `libra init --vault false` now records the disabled signing state in `config_kv` instead of leaving it implicit.
- **Canonical config seeding**: init continues to seed only `config_kv` canonical keys (`core.*`, `libra.repoid`) and no longer relies on legacy `config` table writes.

## [0.1.5]

### Breaking Changes

- **`libra vault` subcommand removed**: Vault functionality has been integrated into `libra config`. Migration guide:
  | Old command | New command |
  |-------------|------------|
  | `libra vault generate-ssh-key` | `libra config generate-ssh-key --remote <remote-name>` |
  | `libra vault generate-gpg-key` | `libra config generate-gpg-key` |
  | `libra vault gpg-public-key` | `libra config get vault.gpg.pubkey` |
  | `libra vault ssh-public-key` | `libra config get vault.ssh.<remote-name>.pubkey` |

  Note: `<remote-name>` should be replaced with your actual remote name (usually `origin`).

- **`--system` scope removed**: System-level configuration has been removed due to multi-user permission isolation issues. Migrate existing `--system` config to `--global`:
  | Old usage | New usage |
  |-----------|----------|
  | `libra config set --system key value` | `libra config set --global key value` |
  | `libra config --get --system key` | `libra config get --global key` |
  | `libra config --list --system` | `libra config list --global` |

- **`libra config edit` not supported**: Libra uses SQLite storage; multi-value key diff-based editing cannot guarantee data consistency. Use `libra config set`/`unset`/`list` to manage configuration.

- **Config storage backend migrated**: Configuration storage moved from three-column split table (`config`) to flat key/value table (`config_kv`) with optional vault encryption. Old `Config` API is deprecated.

### Added

- **Subcommand-style CLI**: `libra config set/get/list/unset/import/path/generate-ssh-key/generate-gpg-key` with Git-compatible flag aliases (`--get`, `--list`, `-l`, `--unset`, `--add`, etc.)
- **Vault-backed encryption**: Sensitive keys (`vault.env.*`, `*.privkey`, API keys, tokens, passwords) are automatically encrypted using AES-256-GCM
- **Environment variable vault**: `vault.env.*` namespace for storing API keys and secrets with `resolve_env()` priority chain (CLI args > system env > local config > global config)
- **Per-remote SSH keys**: `libra config generate-ssh-key --remote <name>` generates isolated SSH keys per remote
- **`--encrypt` flag**: Force encryption for any config value
- **`--stdin` flag**: Read values from stdin for CI/CD pipelines
- **`--show-origin` flag**: Show which scope (local/global) each config value comes from
- **`--vault` flag**: List vault environment variables across scopes
- **`config path` subcommand**: Show config database file path
- **`config import`**: Enhanced with `--no-includes` for global scope, multi-value key handling, auto-encryption of sensitive keys
- **Sensitive key auto-detection**: `is_sensitive_key()` classifies keys by naming patterns
