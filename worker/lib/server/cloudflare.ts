// Server-only: Cloudflare bindings + env wrapper.
//
// Imported only by Next route handlers, server modules, and tests.
// React Client Components must NOT import this file. The
// `no-restricted-imports` rule in `eslint.config.mjs` catches the
// accidental case at edit time; the `import "server-only"` below
// catches it at build time when bundled for the browser.

import "server-only";
import { getCloudflareContext } from "@opennextjs/cloudflare";

export type Bindings = {
  readonly db: D1Database;
  readonly bucket: R2Bucket;
  readonly accessTeamDomain: string | undefined;
  readonly accessAud: string | undefined;
};

/**
 * Resolve the live Cloudflare bindings + relevant env at request time.
 * `getCloudflareContext()` reads the per-request context that OpenNext
 * installs around Next route handlers, so it MUST NOT be called at
 * module top level.
 *
 * Codex pass-1 P1: there is intentionally NO env var for "skip Access
 * on private sites". Private visibility is enforced unconditionally
 * by `enforceVisibility` whenever `site.visibility === "private"`.
 */
export function getBindings(): Bindings {
  const { env } = getCloudflareContext();
  return {
    db: env.LIBRA_PUBLISH_DB,
    bucket: env.LIBRA_PUBLISH_BUCKET,
    accessTeamDomain: env.CF_ACCESS_TEAM_DOMAIN,
    accessAud: env.CF_ACCESS_AUD,
  };
}
