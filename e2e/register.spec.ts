// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { editArgument, invokes, stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

test("the register renders the fixture ledger and its three styles", async ({ page }) => {
	await page.goto("/account/Assets:Bank");

	// Basic ledger: one line per entry, the counter-account named.
	await expect(page.getByRole("heading", { name: "Assets:Bank" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "Fees and top-up", exact: true })).toBeVisible();
	await expect(page.getByRole("cell", { name: "Expenses:Fees" })).toBeVisible();

	await page.getByRole("button", { name: "Splits", exact: true }).click();
	await expect(page.getByRole("columnheader", { name: "Tot Withdrawal" })).toBeVisible();
	await expect(page.getByText("₺100.00").first()).toBeVisible();

	await page.getByRole("button", { name: "Full", exact: true }).click();
	await expect(page.getByRole("cell", { name: "Expenses:Food:Groceries" }).first()).toBeVisible();
});

test("an uncommitted entry is marked, and only that one", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	const modified = page.getByRole("row", { name: /Fees and top-up/ }).first();
	const clean = page.getByRole("row", { name: /Migros — weekly groceries/ }).first();
	await expect(modified).toHaveCSS("background-color", "oklab(0.696 -0.162114 0.0511765 / 0.1)");
	expect(await clean.evaluate((row) => getComputedStyle(row).backgroundColor)).not.toContain("0.696");
});

test("editing a cell in place sends exact decimal strings, never floats", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	// Amount fields live on the leg rows, which the split styles show.
	await page.getByRole("button", { name: "Splits", exact: true }).click();
	await page.getByRole("row", { name: /Migros — weekly groceries/ }).first().dblclick();

	// The row's own cells became fields.
	const narration = page.getByLabel("Narration");
	await expect(narration).toHaveValue("weekly groceries");
	const amount = page.getByLabel("Amount on Assets:Bank:Checking");
	await expect(amount).toHaveValue("-123.45");

	await narration.fill("groceries and coffee");
	await page.getByLabel("Amount on Assets:Bank:Checking").fill("-124.45");
	await narration.press("Enter");

	await expect
		.poll(async () => editArgument(await invokes(page), "update_transaction"))
		.toMatchObject({
			id: 3,
			date: "2026-01-10",
			flag: "*",
			payee: "Migros",
			narration: "groceries and coffee",
		});
	const edit = editArgument(await invokes(page), "update_transaction");
	expect(edit?.postings).toEqual([
		{ index: 0, units: "-124.45" },
		{ index: 1, units: "123.45" },
	]);
	// Decimal strings all the way: nothing was parsed into a float.
	expect(JSON.stringify(edit)).not.toMatch(/\d+\.\d{3,}/);
});

test("the diff and commit views use the backend's diff, and commit the message", async ({ page }) => {
	await page.goto("/diff");
	await expect(page.getByText('"revised fee split"')).toBeVisible();

	await page.goto("/commit");
	await expect(page.getByText(/lines staged/)).toBeVisible();
	await page.getByLabel("Commit message").fill("revise the fee split");
	await page.getByRole("button", { name: "Commit staged changes" }).click();

	await expect
		.poll(async () => (await invokes(page)).some((call) => call.cmd === "commit_ledger"))
		.toBe(true);
	await expect(page.getByText("committed abc1234")).toBeVisible();
});

test("the sidebar carries the git views and survives a failing backend", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	await expect(page.getByRole("link", { name: "Diff" })).toBeVisible();
	await expect(page.getByRole("link", { name: "Commit" })).toBeVisible();

	// A backend that rejects must leave the app standing: no thrown render.
	await page.goto("/ledger?fail=1");
	await expect(page.getByText("no ledger")).toBeVisible();
	await expect(page.getByRole("link", { name: "Diff" })).toBeVisible();
});
