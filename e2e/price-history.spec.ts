// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "./gate";
import { stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

/**
 * The whole point of the screen: the same commodity, three source choices,
 * three different sets of rows — and it opens from the row on the Prices
 * screen, which is where a reader meets it.
 */
test("a commodity's history opens from the prices table and each source changes the rows", async ({
	page,
}) => {
	await page.goto("/prices");
	await page.getByRole("link", { name: "USD" }).click();
	await expect(page).toHaveURL(/\/prices\/USD$/);
	await expect(page.getByRole("heading", { name: "USD" })).toBeVisible();

	const rows = page.locator('[data-slot="price-history-points"] li');

	// Both: every day, from either source.
	await expect(rows).toHaveCount(3);
	await expect(rows.filter({ hasText: "42.10" })).toHaveCount(1);
	await expect(rows.filter({ hasText: "42.00" })).toHaveCount(1);
	await expect(rows.filter({ hasText: "42.00" }).getByText("transactions")).toBeVisible();

	await page.getByRole("radio", { name: "Explicit prices" }).click();
	await expect(rows).toHaveCount(2);
	await expect(rows.filter({ hasText: "42.10" })).toHaveCount(1);
	await expect(rows.filter({ hasText: "42.00" })).toHaveCount(0);
	await expect(rows.filter({ hasText: "0.92" })).toHaveCount(1);

	await page.getByRole("radio", { name: "From transactions" }).click();
	await expect(rows).toHaveCount(1);
	await expect(rows.filter({ hasText: "42.00" }).getByText("transactions")).toBeVisible();
	await expect(rows.filter({ hasText: "42.10" })).toHaveCount(0);

	await page.getByRole("radio", { name: "Both" }).click();
	await expect(rows).toHaveCount(3);
});

test("currency and month controls narrow the answer, and an empty range says why", async ({
	page,
}) => {
	await page.goto("/prices/USD");
	const rows = page.locator('[data-slot="price-history-points"] li');
	await expect(rows).toHaveCount(3);

	// The commodity is priced in two currencies, so the picker is there and
	// narrowing to one leaves that currency's points.
	await page.getByRole("combobox", { name: "Price currency" }).click();
	await page.getByRole("option", { name: "TRY" }).click();
	await expect(rows).toHaveCount(2);
	await expect(rows.filter({ hasText: "0.92" })).toHaveCount(0);

	// A range that starts after every point is an empty answer, not an error,
	// and the screen says the range is the reason.
	await page.getByLabel("From month").fill("2026-02");
	await page.getByLabel("From month").press("Tab");
	await expect(page.getByText(/No USD prices in TRY in this range\./)).toBeVisible();
	await expect(rows).toHaveCount(0);
});
