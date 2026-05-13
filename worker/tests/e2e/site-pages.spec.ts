import { expect, test } from "@playwright/test";

// Codex pass-11 P3: cover the main Phase 7 pages (code browser,
// file viewer, AI object model, status) on both desktop and
// mobile viewports. Each test asserts the headline + a per-page
// signal element renders without text overflow or unreachable
// controls. The runner picks them up via `playwright.config.ts`
// projects = [desktop, mobile].
//
// The Playwright config starts a local Next dev server with the
// `libra-demo` fixture site unless BASE_URL points at a deployed
// preview.

const SLUG = process.env.LIBRA_E2E_SLUG ?? "libra-demo";

test.describe("publish site pages", () => {
  test("code page renders the tree listing", async ({ page }) => {
    await page.goto(`/sites/${SLUG}`);
    await expect(page.getByRole("link", { name: /Code/ })).toBeVisible();
    // Tree entries should appear; the demo fixture has README.md.
    await expect(page.getByText("README.md")).toBeVisible();
    await expectNoDocumentOverflow(page);
  });

  test("blob page renders a file's source", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/blob/README.md`);
    await expect(page.getByRole("heading", { name: "README.md" })).toBeVisible();
    await expectNoDocumentOverflow(page);
  });

  test("refs page lists branches and tags", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/refs`);
    await expect(page.getByRole("heading", { name: /Published refs/i })).toBeVisible();
    await expect(page.getByRole("heading", { name: /Branches/i })).toBeVisible();
    await expect(page.getByRole("heading", { name: /Tags/i })).toBeVisible();
    await expectNoDocumentOverflow(page);
  });

  test("AI object model page renders the browser shell", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/ai`);
    await expect(page.getByRole("heading", { name: /AI object model/i })).toBeVisible();
    await expectNoDocumentOverflow(page);
  });

  test("AI object model page does not expose public redaction leaks", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/ai`);
    const objectButton = page.getByRole("button", {
      name: /Intent[\s\S]*intent-2026-05-09-001/,
    });
    await expect(objectButton).toBeVisible();
    await objectButton.click();
    await expect(page.getByText("Publish demo intent")).toBeVisible();
    await expect(page.locator("body")).not.toContainText("sk-public-page-fixture");
    await expect(page.locator("body")).not.toContainText("/Users/alice/work/libra");
    await expect(page.locator("body")).not.toContainText("/Volumes/Data/GitMono/libra");
    await expect(page.locator("body")).not.toContainText("private system prompt");
    await expectNoDocumentOverflow(page);
  });

  test("status page renders the latest sync run + clone snippet", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/status`);
    await expect(page.getByRole("heading", { name: /Publish status/i })).toBeVisible();
    await expect(
      page.locator("pre").filter({ hasText: "libra clone libra+cloud://127.0.0.1/libra-demo" }),
    ).toBeVisible();
    await expectNoDocumentOverflow(page);
  });

  test("long paths and empty revision states stay usable", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/tree/src/components`);
    await expect(
      page.getByText("really-long-file-name-that-forces-truncation-in-mobile-publish-browser-view"),
    ).toBeVisible();
    await expectNoDocumentOverflow(page);

    await page.goto(`/sites/${SLUG}?ref=refs/heads/dev`);
    await expect(page.getByText("This folder is empty in the published revision.")).toBeVisible();
    await expectNoDocumentOverflow(page);

    await page.goto(`/sites/${SLUG}/ai?ref=refs/heads/dev`);
    await expect(page.getByText("No AI objects in this revision")).toBeVisible();
    await expectNoDocumentOverflow(page);
  });

  test("non-text file states explain binary and oversize content", async ({ page }) => {
    await page.goto(`/sites/${SLUG}/blob/assets/logo.png`);
    await expect(page.getByText("Binary file", { exact: true })).toBeVisible();
    await expectNoDocumentOverflow(page);

    await page.goto(`/sites/${SLUG}/blob/tests/data/big-blob.bin`);
    await expect(page.getByText("File exceeds preview cap", { exact: true })).toBeVisible();
    await expectNoDocumentOverflow(page);
  });
});

async function expectNoDocumentOverflow(page: import("@playwright/test").Page): Promise<void> {
  await expect
    .poll(
      async () =>
        page.evaluate(() => {
          const documentWidth = Math.max(
            document.documentElement.scrollWidth,
            document.body.scrollWidth,
          );
          return documentWidth <= document.documentElement.clientWidth + 1;
        }),
      { message: "page should not create horizontal document overflow" },
    )
    .toBe(true);
}
