import path from "node:path";
import type { NextConfig } from "next";

const useE2eFixture = process.env.LIBRA_PUBLISH_E2E_FIXTURE === "1";
const cloudflareFixture = path.resolve(process.cwd(), "tests/stubs/opennext-cloudflare.ts");

const nextConfig: NextConfig = {
  output: "standalone",
  reactStrictMode: true,
  typedRoutes: false,
  typescript: {
    ignoreBuildErrors: false,
  },
  ...(useE2eFixture
    ? {
        turbopack: {
          resolveAlias: {
            "@opennextjs/cloudflare": cloudflareFixture,
          },
        },
        webpack(config) {
          config.resolve ??= {};
          config.resolve.alias = {
            ...(config.resolve.alias ?? {}),
            "@opennextjs/cloudflare": cloudflareFixture,
          };
          return config;
        },
      }
    : {}),
};

export default nextConfig;
