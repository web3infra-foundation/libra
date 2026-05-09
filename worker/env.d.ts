// LIBRA-MANAGED env types.
//
// `pnpm cf-typegen` generates `cloudflare-env.d.ts` from `wrangler.jsonc`
// and is the runtime source of truth. This file is kept in source control
// so `tsc --noEmit` succeeds before the user has run `cf-typegen`. If the
// generated file is present it shadows this one (matching declarations
// merge on `CloudflareEnv`).

interface CloudflareEnv {
  /** Static asset binding emitted by OpenNext / Next.js. */
  ASSETS: Fetcher;

  /** D1 database holding the publish schema (`sql/publish/0001_publish.sql`). */
  LIBRA_PUBLISH_DB: D1Database;

  /** R2 bucket holding publish manifests, file previews and AI bundles. */
  LIBRA_PUBLISH_BUCKET: R2Bucket;

  /** Cloudflare Access team domain, e.g. `your-team.cloudflareaccess.com`. */
  CF_ACCESS_TEAM_DOMAIN?: string;
  /** Cloudflare Access AUD tag for the application protecting this Worker. */
  CF_ACCESS_AUD?: string;
}
