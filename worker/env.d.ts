// LIBRA-MANAGED env augmentation.
//
// `pnpm cf-typegen` generates `cloudflare-env.d.ts` from
// `wrangler.jsonc` and is the binding source of truth for
// LIBRA_PUBLISH_DB, LIBRA_PUBLISH_BUCKET and ASSETS.
//
// This file only augments the generated CloudflareEnv with optional
// Access secret names. Wrangler does not emit types for secrets that
// are installed through `wrangler secret put`, so these fields stay
// small and hand-maintained while all concrete bindings come from
// generated types.
//
// Codex pass-4 P3 (closed): the publish.md acceptance step
// `pnpm --dir worker cf-typegen` MUST run before `build` /
// `deploy` so the deployed Worker reads only generated types.

declare global {
  interface CloudflareEnv {
    /** Cloudflare Access team domain, e.g. `your-team.cloudflareaccess.com`. */
    CF_ACCESS_TEAM_DOMAIN?: string;
    /** Cloudflare Access AUD tag for the application protecting this Worker. */
    CF_ACCESS_AUD?: string;
  }
}

export {};
