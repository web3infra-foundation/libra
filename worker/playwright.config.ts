import { defineConfig, devices } from "@playwright/test";

/*
 * Codex pass-4 P3: the Phase 7 acceptance list calls for
 * desktop+mobile screenshot or e2e assertions on the main pages
 * (publish landing, code browser, file viewer, AI object model,
 * status) so a layout regression is caught before deploy.
 *
 * Run via:
 *   pnpm --dir worker e2e:install     # one-off Playwright browser install
 *   pnpm --dir worker dev &           # local Wrangler / Next dev server
 *   pnpm --dir worker e2e
 *
 * `BASE_URL` overrides the default if you point the runner at a
 * remote preview deployment.
 */
export default defineConfig({
  testDir: "tests/e2e",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: process.env.BASE_URL ?? "http://localhost:3000",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"] } },
    { name: "mobile", use: { ...devices["iPhone 14"] } },
  ],
});
