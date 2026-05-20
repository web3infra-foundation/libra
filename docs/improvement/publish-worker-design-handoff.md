# Publish Worker Design Handoff

This file is the durable handoff record for the Phase 7 Claude Design acceptance item in [publish.md](publish.md). No separate Claude Design package is tracked in this repository; the accepted implementation baseline is the current `worker/` template plus the deviations recorded below.

## Accepted Worker Surface

- Routes: `worker/app/page.tsx`, `worker/app/sites/[slug]/page.tsx`, `worker/app/sites/[slug]/tree/[[...path]]/page.tsx`, `worker/app/sites/[slug]/blob/[...path]/page.tsx`, `worker/app/sites/[slug]/refs/page.tsx`, `worker/app/sites/[slug]/ai/page.tsx`, `worker/app/sites/[slug]/status/page.tsx`, `worker/app/sites/[slug]/publish/page.tsx`, and `worker/app/sites/repo/[repoId]/page.tsx`.
- API routes: `worker/app/api/sites/[slug]/**` covers site metadata, refs, revisions, tree, file, AI versions, AI objects, AI graph, and status.
- Components: `SiteShell`, `RefPicker`, `TreeListing`, `FileViewer`, `AiBrowser`, `ClonePanel`, `Breadcrumbs`, and `EmptyState`.
- Styling and static assets: `worker/app/globals.css`, `worker/app/fonts/*.woff2`, `worker/app/error.tsx`, `worker/app/forbidden.tsx`, `worker/app/not-found.tsx`, and `worker/public/robots.txt`.
- State and data flow: server routes load D1/R2-backed site context through `worker/lib/server/*`; client interactions use `worker/lib/client/api.ts` and typed wire contracts from `worker/lib/wire-types.ts`.

## Recorded Deviations

- The Worker is an operational repository browser, not a marketing landing page. The first screen prioritizes site metadata, refs, code browsing, AI model browsing, publish status, and clone commands.
- The Worker remains read-only. Repository restore stays in the local CLI through `libra clone libra+cloud://...`; the Worker does not expose a download endpoint or writable operations.
- Public/private behavior follows the publish access contract. Private pages require Cloudflare Access validation; public pages still use redacted AI payloads.
- AI browsing uses the published AI object model, graph, versions, and object endpoints rather than provider raw payloads or local-only mock data.

## Verification

- `worker/tests/e2e/site-pages.spec.ts` covers the core browser pages.
- `worker/tests/miniflare-api-routes.test.ts` covers the Worker API routes against D1/R2 fixtures.
- `worker/tests/publish-overview.test.ts` covers the page data aggregation used by the publish overview surface.
