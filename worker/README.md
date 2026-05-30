# Libra Publish Worker

This directory holds the Next.js + OpenNext source for the **read-only**
Cloudflare Worker that serves Libra publish snapshots out of D1 / R2.
The worker reads `LIBRA_PUBLISH_DB` (D1) and `LIBRA_PUBLISH_BUCKET`
(R2) bindings to render repository browsers; it never writes back.

## Relationship to the rest of the repository

| Path | Role |
|------|------|
| `worker/` (this dir) | Worker source — Next.js routes, server-side D1/R2 helpers, wire types |
| `src/internal/publish/` | Rust side: builds publish snapshots and uploads them to D1/R2 |
| `src/command/publish.rs` | CLI entry: `libra publish init / sync / deploy / status / unpublish` |
| `sql/publish/` | D1 schema migrations consumed by the worker |
| `docs/improvement/publish.md` | Design / phased rollout for the publish surface, including the Phase 7 Worker-surface handoff |

## Scripts

```bash
pnpm install                # install deps (uses pnpm-lock.yaml)
pnpm dev                    # local dev server with HMR
pnpm lint                   # eslint --max-warnings=0
pnpm test                   # vitest (Node-side, no Workers runtime)
pnpm test:miniflare         # vitest under Miniflare to exercise D1/R2 bindings
pnpm test:watch             # vitest in watch mode
pnpm next:build             # plain Next.js build (no Workers bundling)
pnpm build                  # cf-typegen + opennextjs-cloudflare build → .open-next/
pnpm preview                # build + local Cloudflare preview
pnpm deploy                 # build + opennextjs-cloudflare deploy
pnpm e2e                    # playwright end-to-end tests
pnpm e2e:serve              # next dev for e2e on 127.0.0.1:3127
pnpm e2e:install            # playwright install chromium with deps
```

## Local dev

The worker is invoked through `libra publish` in the typical flow:

1. **Materialise the scaffold**: `libra publish init --slug my-site --clone-domain code.example.com`
   writes `wrangler.jsonc`, `migrations/`, and other LIBRA-MANAGED
   files into this directory (or a target dir you pass).
2. **Sync publish snapshots to D1/R2**: `libra publish sync --dry-run` to plan, then
   `libra publish sync` to upload.
3. **Build the Worker locally**: `pnpm build` (runs `cf-typegen` first so the
   generated `cloudflare-env.d.ts` matches the active `wrangler.jsonc`
   bindings).
4. **Preview**: `pnpm preview` for a Cloudflare-faithful local run, or
   `libra publish deploy --skip-deploy` to drive the same path from the
   CLI without mutating Cloudflare.
5. **Deploy**: `libra publish deploy` (canonical path) or `pnpm deploy`
   (direct). Both produce identical Cloudflare-side state.

`wrangler.jsonc` is **user-owned** after the first `init`. Subsequent
`libra publish init` runs only validate that the LIBRA-MANAGED bindings
(`LIBRA_PUBLISH_DB`, `LIBRA_PUBLISH_BUCKET`, `ASSETS`) still exist; user
edits to non-managed fields are preserved.

## Secrets and credentials

Cloudflare API tokens MUST NOT be written into `wrangler.jsonc`. Use:

- **Local dev**: `.dev.vars` (gitignored)
- **Deployed**: Cloudflare dashboard secrets or `wrangler secret put`

See `wrangler.jsonc`'s header comment for the canonical statement of this
contract.

## Tests

Two layers run from this directory:

- **`pnpm test`** — Node-side vitest covering `lib/server/*` helpers,
  wire-type round-trips, and component derivations. Does not require a
  Cloudflare runtime and is part of the offline CI gate.
- **`pnpm test:miniflare`** — vitest under Miniflare to exercise D1 / R2
  bindings against an in-memory Workers runtime. Tagged separately so
  the slower path can be opted into.

End-to-end Playwright tests live under `tests/e2e/`; they spin a
`next dev` server on `127.0.0.1:3127` and exercise the full
SiteShell → SitesPage → TreeListing / FileViewer / AiBrowser flow.

## Verification

The Rust `live_publish` gate in `src/internal/publish/preflight.rs`
exercises a deployed worker. Set `LIBRA_PUBLISH_LIVE_WORKER_ORIGIN` and
`LIBRA_ENABLE_TEST_LIVE_CLOUD=1` per `.env.test.example`, then run:

```bash
cargo test --features test-live-cloud publish_live -- --test-threads=1
```

Read-only verification keeps the worker honest about its contract: the
gate creates a fresh slug, syncs, fetches a known file, and asserts
the page shape matches `worker/lib/wire-types.ts`.
