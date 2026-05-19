import {
  defineCloudflareConfig,
  type OpenNextConfig,
} from "@opennextjs/cloudflare";

const config = {
  ...defineCloudflareConfig({
    // Default: ISR/server runs in the Worker, static assets via the
    // ASSETS binding declared in wrangler.jsonc. The Libra publish API
    // route handlers run server-side and read D1/R2 via
    // `getCloudflareContext()`; client components only fetch /api/*.
  }),
  // OpenNext invokes this command while `opennextjs-cloudflare build`
  // is running. Keep it separate from `pnpm build` so the Worker build
  // script can run OpenNext without recursively invoking itself.
  buildCommand: "pnpm next:build",
} satisfies OpenNextConfig;

export default config;
