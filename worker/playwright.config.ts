import { defineConfig, devices } from "@playwright/test";

/*
 * Phase 7 acceptance: the e2e runner starts a local Next dev server
 * with deterministic D1/R2 fixture bindings when BASE_URL is not set,
 * then exercises the public Worker pages on desktop and mobile
 * Chromium viewports. Set BASE_URL to point the same assertions at a
 * deployed preview instead.
 *
 * Run via:
 *   pnpm --dir worker e2e:install     # one-off Playwright browser install
 *   pnpm --dir worker e2e
 */
const baseURL = process.env.BASE_URL ?? "http://127.0.0.1:3127";
const shouldStartLocalServer = process.env.BASE_URL === undefined;

export default defineConfig({
  testDir: "tests/e2e",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  ...(shouldStartLocalServer
    ? {
        webServer: {
          command: "pnpm e2e:serve",
          env: { LIBRA_PUBLISH_E2E_FIXTURE: "1" },
          reuseExistingServer: false,
          timeout: 120_000,
          url: baseURL,
        },
      }
    : {}),
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"] } },
    {
      name: "mobile",
      use: { ...devices["Pixel 7"], browserName: "chromium" },
    },
  ],
});
