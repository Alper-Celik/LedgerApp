// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
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
});
