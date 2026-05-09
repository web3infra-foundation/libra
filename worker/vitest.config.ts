import { defineConfig } from "vitest/config";
import path from "node:path";

export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/**/*.test.ts"],
    globals: false,
    poolOptions: {
      threads: { singleThread: true },
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
});
