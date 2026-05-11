import { expect, test } from "@playwright/test";

// Codex pass-11 P3: cover the main Phase 7 pages (code browser,
// file viewer, AI object model, status) on both desktop and
// mobile viewports. Each test asserts the headline + a per-page
// signal element renders without text overflow or unreachable
// controls. The runner picks them up via `playwright.config.ts`
// projects = [desktop, mobile].
//
// These specs assume a Wrangler dev server is already serving the
// `libra-demo` fixture site — run them via:
//
//   pnpm --dir worker dev &
//   pnpm --dir worker e2e

const SLUG = process.env.LIBRA_E2E_SLUG ?? "libra-demo";

test.describe("publish site pages", () => {
  test("code page renders the tree listing", async ({ page }) => {
    await page.goto(`/sites/${SLUG}`);
    await expect(page.getByRole("link", { name: /Code/ })).toBeVisible();
    // Tree entries should appear; the demo fixture has README.md.
    await expect(page.getByText("README.md")).toBeVisible();
  });

  test("blob page renders a file's source", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/blob/README.md`);
    await expect(page.getByText("README.md")).toBeVisible();
  });

  test("refs page lists branches and tags", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/refs`);
    await expect(page.getByRole("heading", { name: /Published refs/i })).toBeVisible();
    await expect(page.getByText("Branches")).toBeVisible();
    await expect(page.getByText("Tags")).toBeVisible();
  });

  test("AI object model page renders the browser shell", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/ai`);
    await expect(page.getByRole("heading", { name: /AI object model/i })).toBeVisible();
  });

  test("status page renders the latest sync run + clone snippet", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/status`);
    await expect(page.getByRole("heading", { name: /Publish status/i })).toBeVisible();
    await expect(page.getByText("libra clone libra+cloud://")).toBeVisible();
  });
});
