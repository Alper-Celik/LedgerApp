// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";
import { expect, test } from "./gate";
import { invokes, stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

/** The calls of one command, in invocation order. */
async function calls(page: Page, cmd: string) {
	return (await invokes(page)).filter((call) => call.cmd === cmd);
}

/**
 * An index point is a price: the series card writes it into the journal as one
 * `price` directive per month. The command takes the *id* — the store, not the
 * card, is the authority — and a null file means the root journal, which the
 * backend picks from the loaded ledger.
 */
test("the inflation card writes a stored series into the journal as prices", async ({ page }) => {
	await page.goto("/inflation");

	await page
		.getByRole("button", { name: "Add TÜİK CPI (TR) to the journal as prices" })
		.click();

	await expect.poll(async () => (await calls(page, "index_write_prices")).length).toBe(1);
	const [write] = await calls(page, "index_write_prices");
	expect(write?.args).toEqual({ id: "CPI:TRY", file: null });

	// The answer is shown where the button is: the commodity, the count, the file.
	await expect(page.getByText(/IDX\.CPI-TRY: wrote 3 prices into \/ledger\/main\.bean/)).toBeVisible();
});

/**
 * The prices screen lists the stored series too — the fetched-index row — and
 * offers the same write. A second press is the backend's no-op, which the
 * screen reports rather than pretending it wrote again.
 */
test("the prices screen offers the same write for a stored index", async ({ page }) => {
	await page.goto("/prices");

	await page.getByRole("button", { name: "Add TÜİK CPI (TR) to the journal as prices" }).click();

	await expect.poll(async () => (await calls(page, "index_write_prices")).length).toBe(1);
	const [write] = await calls(page, "index_write_prices");
	expect(write?.args).toEqual({ id: "CPI:TRY", file: null });

	await expect(page.getByText(/IDX\.CPI-TRY: wrote 3 prices/)).toBeVisible();
});

/**
 * The reader can add a series the store does not hold yet: the provider list
 * downloads and stores it, and the new card is then what can be written as
 * prices — the same write, with the new id.
 */
test("a series that is not stored yet is fetched from its provider, then written", async ({
	page,
}) => {
	await page.goto("/inflation");

	await page.getByRole("button", { name: "Fetch US CPI from FRED" }).click();

	await expect.poll(async () => (await calls(page, "index_refresh")).length).toBe(1);
	const [refresh] = await calls(page, "index_refresh");
	expect(refresh?.args).toEqual({ id: "us-cpi" });

	// The stored series is a card now, and the provider row is gone.
	await expect(page.getByRole("heading", { name: "Index us-cpi" })).toBeVisible();
	await expect(page.getByRole("button", { name: "Fetch US CPI from FRED" })).toHaveCount(0);

	await page
		.getByRole("button", { name: "Add Index us-cpi to the journal as prices" })
		.click();
	await expect.poll(async () => (await calls(page, "index_write_prices")).length).toBe(1);
	const [write] = await calls(page, "index_write_prices");
	expect(write?.args).toEqual({ id: "us-cpi", file: null });
});

/**
 * TEFAS is a source the reader can reach for a fund row: the selector offers it
 * for every kind, and the request names the source and the row's currency. No
 * `price:` metadata is needed — the backend falls back to the commodity code as
 * the TEFAS ticker, and TEFAS publishes in TRY.
 */
test("a fund can be fetched from TEFAS without price metadata", async ({ page }) => {
	await page.goto("/prices");

	const trigger = page.getByRole("button", { name: "Price source for AFT" });
	await expect(trigger).toContainText("yahoo");
	await trigger.click();
	await page.getByRole("option", { name: "tefas" }).click();
	await expect(trigger).toContainText("tefas");

	await page.getByRole("button", { name: "Fetch AFT" }).click();

	await expect.poll(async () => (await calls(page, "fetch_prices")).length).toBe(1);
	const [fetch] = await calls(page, "fetch_prices");
	expect(fetch?.args).toEqual({
		requests: [{ commodity: "AFT", currency: "TRY", source: "tefas", date: null }],
	});

	// The answer stays on its row, with the source that gave it.
	await expect(page.getByText("1.2345")).toBeVisible();
});
