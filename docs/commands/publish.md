# `libra publish`

Prepare Libra's read-only Cloudflare Worker publish surface.

Current implementation status in v0.17.53:

- `libra publish init` materialises the embedded Worker template under
  `worker/` and records `.libra/publish/worker-template-manifest.json`.
- `libra publish status` reports the local Worker template state as
  `missing`, `current`, `modified`, `outdated`, or `conflicted`.
- `libra publish sync --dry-run` scans local branch/tag refs, validates
  `--ref`, reports dirty-tree warnings, and emits the local publish
  plan without Cloudflare credentials.
- `libra publish sync` without `--dry-run`, `deploy`, and `unpublish`
  are registered CLI surfaces, but they currently return
  `LBR-UNSUPPORTED-001` with a pointer to
  `docs/improvement/publish.md`.
- The full code/ref/AI snapshot upload, cloud status comparison, Worker
  deploy, and unpublish flows remain tracked in
  `docs/improvement/publish.md`.

## Synopsis

```
libra publish init      [OPTIONS]
libra publish sync      [OPTIONS]
libra publish status    [OPTIONS]
libra publish deploy    [OPTIONS]
libra publish unpublish [OPTIONS]
```

## Description

`libra publish` is being developed as the outward-facing counterpart
to `libra cloud`. The shipped slices are local Worker-template
initialisation, local Worker-template status, and offline sync
dry-runs; they do not upload repository snapshots, mutate Cloudflare
D1/R2 state, deploy a Worker, or implement Git protocol.

## Subcommands

### `libra publish init`

```
libra publish init \
    [--slug <slug>] \
    [--clone-domain <clone-domain>] \
    [--display-origin <origin>] \
    [--name <human-name>] \
    [--visibility public|private] \
    [--worker-name <name>] \
    [--max-preview-bytes <bytes>]
```

- Confirms the current directory is a Libra repo.
- Reuses the current repository root as the template target.
- Writes `.libra/publish/worker-template-manifest.json` with the
  embedded template version, render policy, and SHA-256 baseline for
  each managed file.
- Accepts the site-shaping flags listed above for forward
  compatibility. The current implementation does not persist those
  values to repository config; it only uses the CLI parser to validate
  flag shape.
- `--max-preview-bytes <bytes>`: must be `> 0`; the CLI rejects `0`
  before the template is written.
- Materialises `worker/` from the embedded Worker template. Missing
  files are written fresh, byte-identical template files are left as
  current, and user-modified or symlinked paths fail closed with
  `LBR-CONFLICT-002`; no conflict markers are written.
- Does **not** require Cloudflare connectivity.

### `libra publish sync`

```
libra publish sync [--ref <branch|tag|full-ref>]
                   [--dry-run]
                   [--fail-on-dirty]
                   [--ai-redaction default|strict]
                   [--allow-sensitive-path <path>]…
                   [--force]
                   [--json]
```

Current behavior:

- `--dry-run` scans local `refs/heads/*` and `refs/tags/*`, dedupes by
  revision oid, counts files in each unique commit tree, and emits a
  plan. It does not read or write Cloudflare D1/R2 and does not require
  Cloudflare credentials.
- `--ref <branch|tag|full-ref>` filters the dry-run to one branch or
  tag. If a short name exists as both a branch and a tag, the command
  fails with `LBR-CLI-003` and asks for `refs/heads/<name>` or
  `refs/tags/<name>`.
- Dirty worktrees emit a warning because the dry-run plans committed
  refs only. `--fail-on-dirty` converts that condition into
  `LBR-REPO-003`.
- `--json` returns `siteId` (`null` until cloud config lands),
  `refsCount`, `revisionCount`, `defaultRef`, `latestRevisionOid`,
  `fileCount`, `aiObjectCount`, `aiBundleCount`, `warnings`, and the
  selected ref/revision details.
- Without `--dry-run`, this subcommand still exits with
  `LBR-UNSUPPORTED-001`; the D1/R2 upload path remains tracked in
  `docs/improvement/publish.md`.

### `libra publish status`

```
libra publish status [--json]
```

Current behavior: this subcommand inspects only the local Worker
template and manifest. It does not inspect Cloudflare D1/R2 state.

The status is:

- `missing`: the manifest or one or more embedded template files are
  absent.
- `current`: every embedded template file matches the current Libra
  template and the manifest exists.
- `modified`: a managed template file differs from both the current
  embedded template and the manifest baseline.
- `outdated`: a managed template file still matches the manifest
  baseline, but Libra embeds a newer template version.
- `conflicted`: the `worker/` root or a managed template path is a
  symlink or non-file path.

`--json` returns counts for total, current, missing, modified,
outdated, and conflicted files.

### `libra publish deploy`

```
libra publish deploy [--skip-deploy]
```

Current behavior: this subcommand is not implemented. It exits with
`LBR-UNSUPPORTED-001` and does not run `pnpm`, `wrangler`, or D1
migrations.

### `libra publish unpublish`

```
libra publish unpublish [--yes]
```

Current behavior: this subcommand is not implemented. It exits with
`LBR-UNSUPPORTED-001` and does not mutate local config, Cloudflare D1,
R2, Worker routes, or Worker deployment state.

## Configuration

`libra publish init` currently does not write publish keys into
`ConfigKv`. It records only the Worker template manifest described in
the Files section.

The planned Phase 4 configuration keys are:

| Key | Description |
|-----|-------------|
| `publish.site_id` | UUIDv4 minted at init. Stable. |
| `publish.slug` | Human-readable slug; unique within a clone domain. |
| `publish.clone_domain` | Namespace inside which `slug` resolves. |
| `publish.display_origin` | HTTPS origin browsers visit (e.g. `https://code.example.com`). |
| `publish.name` | Display name for the site. |
| `publish.visibility` | `public` or `private`. |
| `publish.worker_name` | Wrangler worker name. |
| `publish.max_preview_bytes` | Per-file preview size cap. |

The planned sync/deploy implementation will read Cloudflare
credentials, account ids, and API tokens from the same `LIBRA_D1_*` /
`LIBRA_STORAGE_*` environment variables that `libra cloud` uses. They
are never written into the Worker template or to `ConfigKv`.

## Files

- `sql/publish/0001_publish.sql` — the D1 schema source of truth.
- `sql/publish/0002_publish_digest_check.sql` — additive trigger
  migration that enforces lowercase 64-char hex on every digest
  column for tenants who already applied 0001. Required because
  SQLite's `CREATE TABLE IF NOT EXISTS` is a no-op when the table
  already exists, so column-level CHECK additions in 0001 never
  reach existing databases.
- `worker/migrations/<NNNN>_*.sql` — byte-equal mirrors of every
  file under `sql/publish/`, reserved for the planned
  `wrangler d1 migrations apply` step in `libra publish deploy`. The
  `publish_schema_contract_worker_mirror_is_byte_equal` test walks
  both directories and refuses any drift.
- `worker/` — Next.js + React + OpenNext-for-Cloudflare project. Ships
  embedded in the Libra binary; `libra publish init` materialises it
  in the target repository's root.
- `.libra/publish/worker-template-manifest.json` — local manifest
  recording which template version was rendered and which files the
  user modified.
- `.librapublishignore` — per-repo ignore list applied on top of the
  built-in deny rules.

## D1 schema migrations

The publish D1 schema source already lives under `sql/publish/`, and
each `.sql` file has a byte-equal mirror under `worker/migrations/`
(the `publish_schema_contract_worker_mirror_is_byte_equal` Rust test
walks both directories and refuses any drift). Current
`libra publish deploy` does not apply these migrations yet; that
behavior remains part of the Phase 4 deploy plan.

Current chain:

1. `sql/publish/0001_publish.sql` — initial schema. Tables:
   `publish_sites`, `publish_revisions`, `publish_refs`,
   `publish_files`, `publish_ai_objects`, `publish_ai_versions`,
   `publish_sync_runs`. Adds composite FKs, ref-name shape CHECKs,
   sync-run state-machine CHECK, lowercase-hex digest CHECKs.
2. `sql/publish/0002_publish_digest_check.sql` — additive trigger
   migration. SQLite's `CREATE TABLE IF NOT EXISTS` is a no-op when
   the table already exists, so column-level CHECK additions in
   0001 do not reach existing databases. Migration 0002 adds
   `BEFORE INSERT`/`BEFORE UPDATE` triggers that re-enforce:
   * lowercase 64-char hex shape on `publish_files.content_sha256`,
     `publish_ai_objects.payload_sha256`, and
     `publish_ai_versions.bundle_sha256`;
   * `publish_sites.max_preview_bytes > 0` (the 0001 column-level
     CHECK only enforces `>= 0`; the 0002 trigger pins the
     stricter publish-semantic invariant on existing tenants and
     fires on row-level UPDATE so statements that omit the
     column from the SET list still re-validate).

   All triggers are idempotent (`CREATE TRIGGER IF NOT EXISTS`)
   and safe to re-apply.

Future schema changes go into `0003_<topic>.sql`, etc. Each
migration MUST be additive (`CREATE TABLE … IF NOT EXISTS`,
`ALTER TABLE … ADD COLUMN …`, `CREATE INDEX … IF NOT EXISTS`,
`CREATE TRIGGER … IF NOT EXISTS`) so reapplying on a fresh shard
yields the same end state as applying the chain incrementally.

## See also

- `libra clone` — the planned restore path for Cloudflare D1 / R2
  via the `libra+cloud://<clone-domain>/<slug>` source scheme. Current
  builds recognize that scheme but return `LBR-UNSUPPORTED-001`.
- `libra cloud` — private Cloudflare backup that `publish` builds on
  top of.
- `docs/improvement/publish.md` — internal design + phased rollout.
- `docs/agent/ai-object-model-reference.md` — the AI object model
  contract `publish` exports.
