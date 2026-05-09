// Server-only: Cloudflare bindings + env wrapper.
//
// Imported only by Next route handlers, server modules, and tests.
// React Client Components must NOT import this file (and the eslint
// `no-restricted-imports` rule below will catch accidental drift).

import "server-only";
import { getCloudflareContext } from "@opennextjs/cloudflare";

export type Bindings = {
  readonly db: D1Database;
  readonly bucket: R2Bucket;
  readonly accessTeamDomain: string | undefined;
  readonly accessAud: string | undefined;
  readonly requireAccessForPrivate: boolean;
};

/**
 * Resolve the live Cloudflare bindings + relevant env at request time.
 * `getCloudflareContext()` reads the per-request context that OpenNext
 * installs around Next route handlers, so it MUST NOT be called at
 * module top level.
 */
export function getBindings(): Bindings {
  const { env } = getCloudflareContext();
  return {
    db: env.LIBRA_PUBLISH_DB,
    bucket: env.LIBRA_PUBLISH_BUCKET,
    accessTeamDomain: env.CF_ACCESS_TEAM_DOMAIN,
    accessAud: env.CF_ACCESS_AUD,
    requireAccessForPrivate:
      (env.PUBLISH_REQUIRE_ACCESS_FOR_PRIVATE ?? "true").toLowerCase() !==
      "false",
  };
}
