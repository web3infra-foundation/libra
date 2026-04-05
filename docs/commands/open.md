# `libra open`

`libra open` resolves a configured remote or a direct URL into the corresponding
web URL and launches the system browser.

## Common Commands

```bash
libra open
libra open origin
libra open https://github.com/web3infra-foundation/libra
libra open --json
```

## Human Output

```text
Opening https://github.com/web3infra-foundation/libra
```

`--quiet` suppresses `stdout`.

## Structured Output

```json
{
  "ok": true,
  "command": "open",
  "data": {
    "remote": "origin",
    "remote_url": "git@github.com:web3infra-foundation/libra.git",
    "web_url": "https://github.com/web3infra-foundation/libra",
    "launched": true
  }
}
```

When the argument is a direct URL instead of a remote name, `remote` is `null`.

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Not in a repo and no explicit URL was provided | `LBR-REPO-001` | 128 |
| No remote configured | `LBR-REPO-003` | 128 |
| Unsupported / unsafe resolved URL | `LBR-CLI-003` | 129 |
| Failed to read remote config | `LBR-IO-001` | 128 |
| Failed to launch browser | `LBR-IO-002` | 128 |
