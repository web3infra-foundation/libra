# `libra publish`

Publish a Libra repository as a read-only Cloudflare Worker site, and
sync code + AI object model snapshots to Cloudflare D1/R2 so the
companion `libra clone libra+cloud://...` flow can restore the full
repository elsewhere.

## Synopsis

```
libra publish init      [OPTIONS]
libra publish sync      [OPTIONS]
libra publish status    [OPTIONS]
libra publish deploy    [OPTIONS]
libra publish unpublish [OPTIONS]
```

## Description

`libra publish` is the outward-facing counterpart to `libra cloud`:

- `libra cloud sync` keeps a private Cloudflare D1/R2 backup of Git
  objects + agent capture for one repository; the data is *not*
  browsable by humans.
- `libra publish` builds an additional, read-only site on top of that
  backup. The site renders branch/tag/tree/file views, the redacted
  Libra AI object model, and the publish status — all served by a
  Cloudflare Worker compiled from the `worker/` Next.js + React
  template that ships with the Libra binary.

`libra publish` does **not** implement Git protocol. Restoring a full
repository from Cloudflare uses `libra clone libra+cloud://<clone-domain>/<slug>`;
see `libra clone --help` and `docs/commands/clone.md`.

## Subcommands

### `libra publish init`

```
libra publish init \
    --slug <slug> \
    --clone-domain <clone-domain> \
    [--display-origin <origin>] \
    [--name <human-name>] \
    [--visibility public|private] \
    [--worker-name <name>] \
    [--max-preview-bytes <bytes>]
```

- Confirms the current directory is a Libra repo.
- Reuses or generates `libra.repoid`.
- Writes `publish.*` keys into `ConfigKv`.
- Materialises `worker/` from the embedded Worker template using the
  `worker_template_manifest.json` ruleset: missing files are written
  fresh, unmodified template files may be patched, user-modified
  files are preserved with conflict markers.
- Does **not** require Cloudflare connectivity; D1 / R2 credentials
  are validated at sync time.

### `libra publish sync`

```
libra publish sync [--ref <branch|tag|full-ref>]
                   [--dry-run]
                   [--fail-on-dirty]
                   [--ai-redaction default|strict]
                   [--json]
```

- Default: scans `refs/heads/*` and `refs/tags/*`, dedupes by target
  revision, builds one snapshot per unique revision, and uploads code
  manifests + file previews + the full AI object model bundle.
- `--ref <branch|tag|full-ref>`: targeted sync. Branch/tag short
  names are accepted; an ambiguous short name (a branch and tag of
  the same name) fails with a hint to use the full ref. Targeted sync
  cannot advance the "all refs published" generation — use the
  default invocation for production releases.
- `--dry-run`: scans + plans without writing to D1 / R2.
- `--fail-on-dirty`: a dirty working tree fails the run instead of
  emitting a warning.
- `--ai-redaction strict`: removes prompt-like, tool-payload-like,
  path-like, and provider-detail-like fields from the AI object
  bundle while preserving every object envelope, relationship edge,
  and index entry. Visibility (public/private) and `--ai-redaction`
  compose; the AI object **type coverage** is fixed and not affected
  by either knob.
- `--json`: emits a stable machine-readable envelope: `site_id`,
  `refs_count`, `revision_count`, `default_ref`, `latest_revision_oid`,
  `file_count`, `ai_object_count`, `ai_bundle_count`, `warnings`.

Write order (so a partial publish is never visible to readers):

1. R2 file blobs + AI object JSONs + AI bundle.
2. D1 `publish_revisions` row marked `syncing`.
3. D1 `publish_files`, `publish_ai_objects`, `publish_ai_versions`
   rows.
4. CAS update of `publish_sites.latest_revision_oid` and
   `refs_generation`; `publish_revisions.status` flips to `published`
   atomically.

A failure at any point writes `publish_sync_runs.status = failed` and
leaves `latest_revision_oid` / `refs_generation` untouched.

### `libra publish status`

Reports:

- Local repo id, site id, slug, visibility.
- Clone domain, display origin, generated clone URL, stable repo
  clone URL.
- Per-ref diff between local `refs/heads/*` + `refs/tags/*` and the
  D1 `publish_refs` table — added, removed, moved, missing-snapshot.
- Most recent `publish_sync_runs` row: status, warnings, file count,
  AI object count, AI bundle count.
- Worker template state: `missing` / `current` / `modified` /
  `outdated` / `conflicted` (the last makes `publish deploy` fail
  closed).

### `libra publish deploy`

- Verifies the Worker template state is not `conflicted`.
- Runs `pnpm --dir worker build` (Next.js + OpenNext for Cloudflare).
- Applies D1 migrations from `worker/migrations/`.
- Invokes `wrangler deploy`.
- Prints the browse URL, the domain-qualified clone URL
  (`libra+cloud://<clone-domain>/<slug>`), and the stable
  `repo/<repo_id>` clone URL.

A failed deploy preserves all sync data; the next `publish sync` is
not rolled back.

### `libra publish unpublish`

- Sets `publish_sites.status = 'disabled'`. The Worker API
  immediately returns 410 for that site and skips R2 reads.
- Does **not** delete D1 rows or R2 objects; recovery via
  `libra clone libra+cloud://...` from the disabled site is not
  guaranteed and is out of scope for v1.
- Does **not** automatically revoke Cloudflare Worker routes; emits a
  hint for the operator to remove the route in Cloudflare's
  dashboard.

## Configuration

`libra publish init` writes the following keys into `ConfigKv`:

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

Cloudflare credentials, account ids, and API tokens are read from
the same `LIBRA_D1_*` / `LIBRA_STORAGE_*` environment variables that
`libra cloud` uses. They are never written into the Worker template
or to `ConfigKv`.

## Files

- `sql/publish/0001_publish.sql` — the D1 schema source of truth.
- `sql/publish/0002_publish_digest_check.sql` — additive trigger
  migration that enforces lowercase 64-char hex on every digest
  column for tenants who already applied 0001. Required because
  SQLite's `CREATE TABLE IF NOT EXISTS` is a no-op when the table
  already exists, so column-level CHECK additions in 0001 never
  reach existing databases.
- `worker/migrations/<NNNN>_*.sql` — byte-equal mirrors of every
  file under `sql/publish/`, applied by `wrangler d1 migrations
  apply` at deploy time. The
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

## See also

- `libra clone` — restore a Libra repository from Cloudflare D1 / R2
  via the `libra+cloud://<clone-domain>/<slug>` source scheme.
- `libra cloud` — private Cloudflare backup that `publish` builds on
  top of.
- `docs/improvement/publish.md` — internal design + phased rollout.
- `docs/agent/ai-object-model-reference.md` — the AI object model
  contract `publish` exports.
