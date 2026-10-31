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

declare global {
	interface Window {
		/** Resolves the `accounts` answer the test below is holding back. */
		__releaseAccounts?: () => void;
	}
}

test("opening from the sidebar refreshes the balances it lists", async ({ page }) => {
	// The sidebar's own trigger, not Settings': it is the control on every
	// screen, and the tree beside it is the list that must follow the switch.
	await page.goto("/ledger?native=folder");
	const sidebar = page.locator('[data-slot="sidebar"]');
	await expect(sidebar).toContainText("1,500.00");
	await sidebar.getByRole("button", { name: "Open folder…" }).click();
	await expect(sidebar).toContainText("999.00");
	await expect(sidebar).not.toContainText("1,500.00");
});

test("balances still loading when a folder is opened cannot land after its refresh", async ({
	page,
}) => {
	// A large journal answers `accounts` slowly, and the pickup can start while
	// the startup answer is in flight. That answer describes the journal that
	// was open when the window loaded; arriving last, it must not put those
	// balances back. Held here until the test releases it, so the two answers
	// cannot race by accident.
	await page.addInitScript(() => {
		const internals = window.__TAURI_INTERNALS__;
		const invoke = internals.invoke;
		let held = false;
		internals.invoke = async (cmd, args) => {
			const answer = await invoke(cmd, args);
			if (cmd === "accounts" && !held) {
				held = true;
				const pending = Promise.withResolvers<void>();
				window.__releaseAccounts = pending.resolve;
				await pending.promise;
			}
			return answer;
		};
	});

	await page.goto("/ledger?native=folder");
	const sidebar = page.locator('[data-slot="sidebar"]');
	await sidebar.getByRole("button", { name: "Open folder…" }).click();
	// The open's own fetch adopted the new journal's balances.
	await expect(sidebar).toContainText("999.00");

	// Now the startup answer lands. One tick is all it needs to be assigned.
	await page.evaluate(() => window.__releaseAccounts?.());
	await page.waitForTimeout(300);
	expect(await sidebar.innerText()).toContain("999.00");
	expect(await sidebar.innerText()).not.toContain("1,500.00");
});

test("a phone opens a folder from the drawer and finds the new balances in it", async ({
	page,
}) => {
	// The drawer is the only way to the button on a phone, and the picker it
	// opens is a sheet of its own: one at a time, or the drawer disappears
	// under the picker's first tap.
	await page.setViewportSize({ width: 390, height: 844 });
	await page.goto("/ledger");
	await page.getByRole("button", { name: "Open navigation" }).click();
	const drawer = page.getByRole("dialog", { name: "Sidebar" });
	await expect(drawer).toContainText("1,500.00");

	await drawer.getByRole("button", { name: "Open folder…" }).click();
	await expect(drawer).toBeHidden();
	const picker = page.getByRole("dialog", { name: "Open ledger folder" });
	await expect(picker).toBeVisible();
	await picker.getByRole("button", { name: /Books/ }).click();
	await picker.getByRole("button", { name: "Open this folder" }).click();
	await expect(picker).toBeHidden();

	await page.getByRole("button", { name: "Open navigation" }).click();
	await expect(page.getByRole("dialog", { name: "Sidebar" })).toContainText("999.00");
});
