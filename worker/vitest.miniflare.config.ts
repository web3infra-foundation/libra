import path from "node:path";
import { defineWorkersConfig, readD1Migrations } from "@cloudflare/vitest-pool-workers/config";

export default defineWorkersConfig(
  readD1Migrations(path.join(__dirname, "migrations")).then((migrations) => ({
    test: {
      include: ["tests/miniflare-api-routes.test.ts"],
      globals: false,
      pool: "@cloudflare/vitest-pool-workers" as const,
      poolOptions: {
        workers: {
          main: "./tests/miniflare-worker.ts",
          miniflare: {
            compatibilityDate: "2026-05-07",
            d1Databases: { LIBRA_PUBLISH_DB: "libra-publish-test" },
            r2Buckets: ["LIBRA_PUBLISH_BUCKET"],
            bindings: {
              TEST_MIGRATIONS: migrations,
              CF_ACCESS_TEAM_DOMAIN: "team.example.cloudflareaccess.com",
              CF_ACCESS_AUD: "test-aud",
            },
          },
        },
      },
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
  })),
);
