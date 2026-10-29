// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "./gate";
import { invokes, stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

test("the folder picker walks folders, marks journals, and opens the choice", async ({ page }) => {
	await page.goto("/settings");

	// Settings has its own trigger; the sidebar's is a separate control.
	await page.locator("main").getByRole("button", { name: "Open folder…" }).click();
	const sheet = page.getByRole("dialog", { name: "Open ledger folder" });
	await expect(sheet).toBeVisible();
	await expect(sheet.getByText("/ledger", { exact: true })).toBeVisible();

	// Folders that hold a root journal say so; the others do not.
	const books = sheet.getByRole("button", { name: /Books/ });
	await expect(books).toContainText("ledger");
	await expect(sheet.getByRole("button", { name: /Archive/ })).not.toContainText("ledger");
	// The root holds no journal itself, so it cannot be opened as it stands.
	await expect(sheet.getByRole("button", { name: "Open this folder" })).toBeDisabled();

	// Walking into a folder replaces the listing (and its marked entry).
	await books.click();
	await expect(sheet.getByText("/ledger/Books", { exact: true })).toBeVisible();
	const open = sheet.getByRole("button", { name: "Open this folder" });
	await expect(open).toBeEnabled();
	await open.click();

	// The sheet closes on success, and the journal the backend resolved is the
	// one Settings now names.
	await expect(sheet).toBeHidden();
	await expect(page.getByText("/ledger/Books/main.bean")).toBeVisible();
	const calls = await invokes(page);
	expect(calls.filter((call) => call.cmd === "load_ledger").at(-1)?.args).toEqual({
		path: "/ledger/Books",
	});

	// Opening another ledger brings the sidebar's balances with it: the tree
	// still showing the previous journal's money makes the switch look like it
	// did nothing.
	const sidebar = page.locator('[data-slot="sidebar"]');
	await expect(sidebar).toContainText("999.00");
	await expect(sidebar).not.toContainText("1,500.00");
});

test("the desktop's own chooser opens the folder it returned, without the sheet", async ({ page }) => {
	// `?native=folder` models the Qt runtime: it draws a chooser and the user
	// picked `Books`.
	await page.goto("/settings?native=folder");
	await page.locator("main").getByRole("button", { name: "Open folder…" }).click();

	// No in-app sheet, and the chosen folder went through `load_ledger`.
	await expect(page.getByRole("dialog", { name: "Open ledger folder" })).toHaveCount(0);
	await expect(page.getByText("/ledger/Books/main.bean")).toBeVisible();
	const calls = await invokes(page);
	expect(calls.filter((call) => call.cmd === "pick_ledger_folder")).toHaveLength(1);
	expect(calls.filter((call) => call.cmd === "load_ledger").at(-1)?.args).toEqual({
		path: "/ledger/Books",
	});
});
