// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";
import { expect, test } from "./gate";
import { stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

/** The sidebar's own tree, which every page carries. */
const sidebarTree = (page: Page) =>
	page.locator('[data-slot="sidebar"] [data-slot="file-tree"]');

test("the sidebar draws the journal's files as folders and leaves", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	const tree = sidebarTree(page);

	const root = tree.locator('[data-path="/ledger"]');
	const accounts = tree.locator('[data-path="/ledger/accounts"]');
	const added = tree.locator('[data-path="/ledger/accounts/2026.bean"]');
	const untouched = tree.locator('[data-path="/ledger/accounts/2025.bean"]');
	const main = tree.locator('[data-path="/ledger/main.bean"]');
	const deep = tree.locator('[data-path="/ledger/Liabilities/TR:Ziraat/Bankkart/2026/2026-06.bean"]');

	// Every folder starts open: the deepest leaf is already on screen.
	await expect(root.getByRole("button")).toHaveAttribute("aria-expanded", "true");
	await expect(accounts.getByRole("button")).toHaveAttribute("aria-expanded", "true");
	await expect(deep).toBeVisible();

	// Counts belong to their own rows, and a folder's is its descendants'.
	await expect(root.locator('[data-slot="file-tree-count"]')).toHaveText("8");
	await expect(accounts.locator('[data-slot="file-tree-count"]')).toHaveText("4");
	await expect(added.locator('[data-slot="file-tree-count"]')).toHaveText("4");
	await expect(untouched.locator('[data-slot="file-tree-count"]')).toHaveText("0");
	await expect(main.locator('[data-slot="file-tree-count"]')).toHaveText("2");

	// Git letters: a leaf says its own change, a folder the strongest under it,
	// and a clean file says nothing.
	await expect(added.getByRole("img", { name: "New since the last commit" })).toBeVisible();
	await expect(accounts.getByRole("img", { name: "New since the last commit" })).toBeVisible();
	await expect(main.getByRole("img", { name: "Changed since the last commit" })).toBeVisible();
	await expect(untouched.getByRole("img")).toHaveCount(0);

	// Folding hides exactly the rows under that folder, and unfolding restores
	// them; the folder row itself stays.
	const toggle = accounts.getByRole("button");
	await toggle.click();
	await expect(toggle).toHaveAttribute("aria-expanded", "false");
	await expect(added).toHaveCount(0);
	await expect(untouched).toHaveCount(0);
	await expect(accounts).toBeVisible();
	await toggle.click();
	await expect(toggle).toHaveAttribute("aria-expanded", "true");
	await expect(added).toBeVisible();
});

test("a folder can be reached with the keyboard and toggled from it", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	const tree = sidebarTree(page);
	const toggle = tree.locator('[data-path="/ledger/accounts"]').getByRole("button");

	// A row is a button, so Tab reaches it and Enter or Space is the click.
	await toggle.focus();
	await page.keyboard.press("Enter");
	await expect(toggle).toHaveAttribute("aria-expanded", "false");
	await page.keyboard.press("Space");
	await expect(toggle).toHaveAttribute("aria-expanded", "true");

	// Leaves stay links, with the file's own path as their target.
	const leaf = tree.locator('[data-path="/ledger/accounts/2026.bean"]').getByRole("link");
	await expect(leaf).toHaveAttribute("href", "/files/%2Fledger%2Faccounts%2F2026.bean");
});

test("clicking a leaf opens that file's screen, and the sidebar marks it current", async ({
	page,
}) => {
	await page.goto("/account/Assets:Bank");
	const leaf = sidebarTree(page)
		.locator('[data-path="/ledger/accounts/2026.bean"]')
		.getByRole("link");
	await leaf.click();

	await expect(page).toHaveURL(/\/files\/%2Fledger%2Faccounts%2F2026\.bean$/);
	await expect(page.getByRole("heading", { name: "accounts/2026.bean" })).toBeVisible();
	await expect(page.getByText("weekly groceries").first()).toBeVisible();

	// The route names the file, so its row is the current one — and the folder
	// holding it is open even though the visit started from elsewhere.
	const tree = sidebarTree(page);
	const accounts = tree.locator('[data-path="/ledger/accounts"]');
	const active = tree.locator('[data-path="/ledger/accounts/2026.bean"]').getByRole("link");
	await expect(active).toHaveAttribute("aria-current", "page");
	await expect(accounts.getByRole("button")).toHaveAttribute("aria-expanded", "true");

	// Navigating back to the file opens its folder again: a folder the reader
	// closed earlier does not keep the route's own file hidden.
	await accounts.getByRole("button").click();
	await expect(active).toHaveCount(0);
	await tree.locator('[data-path="/ledger/main.bean"]').getByRole("link").click();
	await expect(page).toHaveURL(/\/files\/%2Fledger%2Fmain\.bean$/);
	await page.goBack();
	await expect(page).toHaveURL(/\/files\/%2Fledger%2Faccounts%2F2026\.bean$/);
	await expect(accounts.getByRole("button")).toHaveAttribute("aria-expanded", "true");
	await expect(active).toBeVisible();
});

test("the files screen carries the same tree at full width", async ({ page }) => {
	await page.goto("/account/Assets:Bank");

	// The section's label is the way in.
	await page
		.locator('[data-slot="sidebar"]')
		.getByRole("link", { name: "Journal files", exact: true })
		.click();
	await expect(page).toHaveURL(/\/files$/);
	await expect(page.getByRole("heading", { name: "Journal files", level: 1 })).toBeVisible();

	const tree = page.locator("main").locator('[data-slot="file-tree"]');
	await expect(tree).toBeVisible();
	await expect(tree.locator('[data-path="/ledger"]')).toContainText("8");
	await expect(
		tree.locator('[data-path="/ledger/Liabilities/TR:Ziraat/Bankkart/2026/2026-06.bean"]')
	).toBeVisible();

	// Folding works here too, with no sidebar in the way.
	const accounts = tree.locator('[data-path="/ledger/accounts"]');
	await accounts.getByRole("button").click();
	await expect(tree.locator('[data-path="/ledger/accounts/2026.bean"]')).toHaveCount(0);
});

test("the files screen says no ledger is open instead of reporting a failed read", async ({
	page,
}) => {
	await page.goto("/files?fail=1");
	await expect(page.getByRole("heading", { name: "Journal files", level: 1 })).toBeVisible();
	await expect(page.getByText("No ledger is open")).toBeVisible();
});
