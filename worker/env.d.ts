// LIBRA-MANAGED env types.
//
// `pnpm cf-typegen` generates `cloudflare-env.d.ts` from
// `wrangler.jsonc` and is the runtime source of truth. The build
// scripts in `package.json` ensure `cf-typegen` runs before `build`
// and `deploy` so the deployed Worker always uses the generated
// types.
//
// This file stays committed so `tsc --noEmit` succeeds for any
// developer who clones the repo and runs `pnpm test` / lint before
// `pnpm cf-typegen`. When the generated `cloudflare-env.d.ts` is
// present it shadows this stub via declaration merging; the
// generated declarations win and any drift between the two appears
// as a TS error in the Worker tree.
//
// Codex pass-4 P3 (closed): the publish.md acceptance step
// `pnpm --dir worker cf-typegen` MUST run before `build` /
// `deploy` so the deployed Worker reads only generated types.

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
