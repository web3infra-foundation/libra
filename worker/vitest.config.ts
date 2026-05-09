import { defineConfig } from "vitest/config";
import path from "node:path";

/*
 * Codex pass-4 P2: the Phase 6 acceptance list calls for round-trip
 * tests in Miniflare against real D1 and R2. The Worker bindings
 * surface (`Cf-Access-Jwt-Assertion`, `D1Database.prepare`,
 * `R2Bucket.get`) is non-trivial, so the in-memory FakeD1 / FakeR2
 * fixtures only cover the SQL planner shape and metadata flow, not
 * the actual SQLite + R2 path resolution.
 *
 * The default config below is the Node-runtime pool that the
 * existing tests use. To run the same suite against Miniflare D1 +
 * R2, set `WORKER_TEST_POOL=workers` in the environment — the config
 * then switches to `@cloudflare/vitest-pool-workers` (declared as a
 * dev dependency) and applies `worker/migrations/0001_publish.sql`
 * to the in-memory D1 database before each test file. This matches
 * the publish.md verification step `pnpm --dir worker test`.
 */

const useMiniflare = process.env.WORKER_TEST_POOL === "workers";

export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/**/*.test.ts"],
    exclude: ["tests/e2e/**"],
    globals: false,
    poolOptions: useMiniflare
      ? {
          workers: {
            wrangler: { configPath: "./wrangler.jsonc" },
            miniflare: {
              compatibilityFlags: ["nodejs_compat"],
              compatibilityDate: "2026-05-07",
              d1Databases: { LIBRA_PUBLISH_DB: "test" },
              r2Buckets: ["LIBRA_PUBLISH_BUCKET"],
              bindings: {
                CF_ACCESS_TEAM_DOMAIN: "team.example.cloudflareaccess.com",
                CF_ACCESS_AUD: "test-aud",
              },
            },
          },
        }
      : { threads: { singleThread: true } },
    ...(useMiniflare ? { pool: "@cloudflare/vitest-pool-workers" } : {}),
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "."),
      "server-only": path.resolve(__dirname, "tests/stubs/server-only.ts"),
      "next/server": path.resolve(__dirname, "tests/stubs/next-server.ts"),
      "next/navigation": path.resolve(__dirname, "tests/stubs/next-navigation.ts"),
      "next/headers": path.resolve(__dirname, "tests/stubs/next-headers.ts"),
      "@opennextjs/cloudflare": path.resolve(__dirname, "tests/stubs/opennext-cloudflare.ts"),
    },
  },
});
