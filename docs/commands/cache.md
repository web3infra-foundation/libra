# `libra cache`

Inspect Libra's tiered-storage / LRU cache configuration (lore.md §0.10). This
is a diagnostic helper that surfaces the existing `LIBRA_STORAGE_*` tunables so
you can confirm what the running storage backend would apply.

## Synopsis

```
libra cache info
```

## Description

`cache info` reports the resolved storage/cache tunables, in the same way the
tiered storage backend resolves them (environment first, then the global config
DB), so the reported values match what the backend uses:

- **storage** — the raw `LIBRA_STORAGE_TYPE` value (`local` only when unset;
  otherwise your exact value, e.g. `s3` / `r2` — a wrong-case `R2` is shown
  verbatim and reports non-tiered).
- **tier** — whether the config selects a durable tier: `LIBRA_STORAGE_TYPE` is
  a case-sensitive `s3` / `r2` that also passes every static check the backend
  applies before connecting (non-empty bucket, parseable `LIBRA_STORAGE_ENDPOINT`,
  non-empty `LIBRA_STORAGE_ACCESS_KEY` / `LIBRA_STORAGE_SECRET_KEY`). So a
  wrong-case `R2`, an empty key, or a malformed endpoint reports non-tiered
  rather than misleading you. The cache tunables only take effect when tiered; a
  local-only repository caches nothing. An actual connection additionally needs
  valid credentials, which this static report does not validate.
- **threshold** — the small/large object threshold in bytes
  (`LIBRA_STORAGE_THRESHOLD`, default 1 MiB). Objects at or above this size are
  LRU-cached rather than stored permanently.
- **cache** — the local LRU disk budget in bytes (`LIBRA_STORAGE_CACHE_SIZE`,
  default 200 MiB) for large cached objects.

An unparseable numeric value falls back to the default (mirroring the storage
backend's lenient parse), so `cache info` never fails on a bad value. It needs no
repository.

### Storage / cache environment variables

| Variable | Meaning |
|----------|---------|
| `LIBRA_STORAGE_TYPE` | Backend type. Unset → local-only; `s3` / `r2` → tiered (durable tier + local LRU cache). |
| `LIBRA_STORAGE_THRESHOLD` | Small/large object threshold in bytes (default `1048576`). Objects `>=` this are LRU-cached; smaller ones are stored permanently locally. |
| `LIBRA_STORAGE_CACHE_SIZE` | Local LRU disk budget in bytes for large cached objects (default `209715200`). |

## Options

| Option | Description | Example |
|--------|-------------|---------|
| `info` | Show the resolved storage/cache configuration. | `libra cache info` |
| `--json` / `--machine` | Structured `{ storage_type, tiered, threshold_bytes, cache_size_bytes }`. | `libra --json cache info` |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Configuration was reported. |
| non-zero | A storage config value could not be resolved (e.g. an unreadable global config DB); the failure is surfaced rather than silently reporting a default. |

## Examples

```bash
# Show the resolved storage/cache tunables with the current environment.
libra cache info

# Inspect a tiered (R2) configuration with a custom LRU budget.
LIBRA_STORAGE_TYPE=r2 LIBRA_STORAGE_CACHE_SIZE=536870912 libra cache info

# Structured output for tooling.
libra --json cache info
```

## Comparison with Git

Git has no equivalent; this is a Libra diagnostic extension for its tiered
object store, classified `intentionally-different` in
[`COMPATIBILITY.md`](../../COMPATIBILITY.md).
