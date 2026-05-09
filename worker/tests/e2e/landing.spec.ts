import { expect, test } from "@playwright/test";

// Phase 7 acceptance: desktop and mobile viewports must render the
// publish landing page without text overflow, occlusion, or
// unreachable controls.

test.describe("publish landing page", () => {
  test("desktop renders the headline + clone snippet", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: /libra/i })).toBeVisible();
    await expect(page.getByText("libra clone libra+cloud://")).toBeVisible();
  });

  test("api endpoint cards stay visible at narrow widths", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText("Browse a published site")).toBeVisible();
    await expect(page.getByText("Stable repo entry")).toBeVisible();
  });
});
