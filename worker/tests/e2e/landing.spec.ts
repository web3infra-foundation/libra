import { expect, test } from "@playwright/test";

// Phase 7 acceptance: desktop and mobile viewports must render the
// publish landing page without text overflow, occlusion, or
// unreachable controls.

test.describe("publish landing page", () => {
  test("desktop renders the headline + clone snippet", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: /libra/i })).toBeVisible();
    await expect(page.getByText("libra clone libra+cloud://")).toBeVisible();
    await expectNoDocumentOverflow(page);
  });

  test("api endpoint cards stay visible at narrow widths", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText("Browse a published site")).toBeVisible();
    await expect(page.getByText("Stable repo entry")).toBeVisible();
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
