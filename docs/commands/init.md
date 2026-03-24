# `libra init`

`libra init` creates a new Libra repository, seeds the SQLite-backed metadata in
`.libra/libra.db`, configures `HEAD`, and optionally imports an existing local Git repository.

## Common Commands

```bash
libra init
libra init my-project
libra init --bare my-repo.git
libra init -b develop
libra init --object-format sha256
libra init --from-git-repository ../old-project
libra init --vault false
```

## Human Output

Default human mode writes staged progress to `stderr` and the final confirmation to `stdout`.

Phases include:

- `Creating repository layout ...`
- `Initializing database ...`
- `Setting up refs ...`
- `Converting from Git repository at ...` when `--from-git-repository` is used
- `Generating PGP signing key ...` when vault signing is enabled

Success output uses past tense:

```text
Initialized empty Libra repository in /path/to/repo/.libra
  branch: main
  signing: enabled
```

`--quiet` suppresses both progress and the final success summary.

## Structured Output

`libra init` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- both suppress progress output
- `stderr` stays clean on success, including `--from-git-repository`

Example:

```json
{
  "ok": true,
  "command": "init",
  "data": {
    "path": "/path/to/repo/.libra",
    "bare": false,
    "initial_branch": "main",
    "object_format": "sha1",
    "ref_format": "strict",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "converted_from": null,
    "ssh_key_detected": "/Users/alice/.ssh/id_ed25519",
    "warnings": []
  }
}
```

## Vault And Identity

- Vault-backed signing is enabled by default
- `--vault false` skips vault setup and writes `vault.signing=false`
- When vault signing is enabled, Libra resolves identity from:
  1. target repository local config
  2. global config
  3. `GIT_COMMITTER_*`, `GIT_AUTHOR_*`, `EMAIL`, `LIBRA_COMMITTER_*`
  4. built-in fallback: `Libra User <user@libra.local>`

This is intentionally less strict than `libra commit`: missing identity does not block repository creation.

## Git Import

`--from-git-repository <path>` fetches objects and refs from a local Git repository and configures
`origin` plus the imported branch layout.

- the source path must point to a valid local Git repository
- `converted_from` in JSON output reports the canonical source Git directory
- empty Git repositories fail with a repo-state error because there are no refs to import

## Compatibility Notes

- `--separate-libra-dir` and `--separate-git-dir` are removed
- non-bare repositories always use the standard `.libra/` layout inside the worktree
- historical repositories that used a `gitdir:` `.libra` link file are no longer detected

Migration for old separate-layout repositories:

```bash
rm .libra
mv /path/to/separate/storage .libra
```

## Feature Comparison: Libra vs Git vs jj

| Use Case | Git | jj | Libra |
|----------|-----|----|-------|
| Current directory init | `git init` | `jj git init` | `libra init` |
| New directory init | `git init my-project` | `jj git init my-project` | `libra init my-project` |
| Bare repo | `git init --bare repo.git` | No direct equivalent | `libra init --bare repo.git` |
| Initial branch flag | `git init -b main` | No direct init flag | `libra init -b main` |
| Object format flag | `git init --object-format=sha256` | No direct init flag | `libra init --object-format sha256` |
| Import existing Git repo | No single command | `jj git init --git-repo <path>` | `libra init --from-git-repository <path>` |
| Structured output | No | No | `--json` / `--machine` |
| Signing bootstrap | No | No | Vault + PGP key by default |
| SSH key behavior | Use system SSH config | Use system / Git config | Detect system key, do not generate during init |
| Separate storage dir | `--separate-git-dir` | Different colocate model | Removed |
