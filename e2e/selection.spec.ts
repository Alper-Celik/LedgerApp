// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

test("a sidebar + starts a selection with that account", async ({ page }) => {
	await page.goto("/account/Assets:Bank");

	await page.getByRole("button", { name: "Add Assets:Bank to selection" }).click();

	await expect(page).toHaveURL(/\/register\?accounts=Assets%3ABank$/);
	await expect(page.getByRole("heading", { name: "Assets:Bank" })).toBeVisible();
});

test("the register merges selected accounts without duplicates", async ({ page }) => {
	// The stub answers every account with the same two transactions, so a
	// naive concatenation would show each entry twice; identity must dedupe.
	await page.goto("/register?accounts=Assets%3ABank,Liabilities%3AKYK");

	await expect(page.getByRole("heading", { name: "2 selected accounts" })).toBeVisible();
	await expect(page.getByRole("row", { name: /Fees and top-up/ })).toHaveCount(1);
	await expect(page.getByRole("row", { name: /Migros — weekly groceries/ })).toHaveCount(1);
});

test("a chip removes its account, Clear all empties the selection", async ({ page }) => {
	await page.goto("/register?accounts=Assets%3ABank,Liabilities%3AKYK");

	await page.getByRole("link", { name: "Remove Assets:Bank from selection" }).click();
	await expect(page).toHaveURL(/accounts=Liabilities%3AKYK$/);
	await expect(page.getByRole("heading", { name: "Liabilities:KYK" })).toBeVisible();

	// With an href the Button renders as a link.
	await page.getByRole("link", { name: "Clear all" }).click();
	await expect(page).toHaveURL(/\/register$/);
	await expect(page.getByText("No accounts selected.")).toBeVisible();
});

test("the picker adds an account to an existing selection", async ({ page }) => {
	await page.goto("/register?accounts=Assets%3ABank");

	await page.getByRole("combobox", { name: "Account to add" }).selectOption("Liabilities:KYK");
	await page.getByRole("button", { name: "Add", exact: true }).click();

	// URL normalization keeps %3A but leaves the separator comma bare.
	await expect(page).toHaveURL(/accounts=Assets%3ABank,Liabilities%3AKYK$/);
	await expect(page.getByRole("heading", { name: "2 selected accounts" })).toBeVisible();
});
