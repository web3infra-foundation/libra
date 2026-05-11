import { defineCloudflareConfig } from "@opennextjs/cloudflare";

export default defineCloudflareConfig({
  // Default: ISR/server runs in the Worker, static assets via the
  // ASSETS binding declared in wrangler.jsonc. The Libra publish API
  // route handlers run server-side and read D1/R2 via
  // `getCloudflareContext()`; client components only fetch /api/*.
});
