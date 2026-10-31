// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";
import { expect, test } from "./gate";
import { invokes, stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

/** Both panels stay mounted, so the hidden one is asserted through its own id. */
const panel = (page: Page, id: "accounts" | "files") => page.locator(`#sidebar-panel-${id}`);

test("the sidebar carries Accounts and Files as tabs, with Accounts first", async ({ page }) => {
	await page.goto("/ledger");

	const tablist = page.getByRole("tablist", { name: "Sidebar sections" });
	await expect(tablist).toBeVisible();
	await expect(tablist.getByRole("tab")).toHaveText(["Accounts", "Files"]);

	const accounts = tablist.getByRole("tab", { name: "Accounts" });
	const files = tablist.getByRole("tab", { name: "Files" });
	await expect(accounts).toHaveAttribute("aria-selected", "true");
	await expect(accounts).toHaveAttribute("aria-controls", "sidebar-panel-accounts");
	await expect(files).toHaveAttribute("aria-selected", "false");
	await expect(files).toHaveAttribute("aria-controls", "sidebar-panel-files");

	// The default tab is the one showing: the account tree, with its balances.
	await expect(page.getByRole("tabpanel", { name: "Accounts" })).toBeVisible();
	await expect(panel(page, "accounts").getByText("All accounts")).toBeVisible();
	await expect(panel(page, "accounts").getByText("Assets", { exact: true })).toBeVisible();
	await expect(panel(page, "files")).toBeHidden();
});

test("choosing a tab shows its list and hides the other", async ({ page }) => {
	await page.goto("/ledger");
	const accountsTab = page.getByRole("tab", { name: "Accounts" });
	const filesTab = page.getByRole("tab", { name: "Files" });

	await filesTab.click();
	await expect(filesTab).toHaveAttribute("aria-selected", "true");
	await expect(page.getByRole("tabpanel", { name: "Files" })).toBeVisible();
	await expect(panel(page, "files").locator('[data-slot="file-tree"]')).toBeVisible();
	await expect(panel(page, "files")).toContainText("main.bean");
	await expect(panel(page, "accounts")).toBeHidden();

	await accountsTab.click();
	await expect(accountsTab).toHaveAttribute("aria-selected", "true");
	await expect(panel(page, "accounts")).toBeVisible();
	await expect(panel(page, "accounts").getByText("All accounts")).toBeVisible();
	await expect(panel(page, "files")).toBeHidden();
});

test("the chosen tab survives navigation and a reload", async ({ page }) => {
	await page.goto("/ledger");
	await page.getByRole("tab", { name: "Files" }).click();

	// A navigation elsewhere in the app must not put the sidebar back on Accounts.
	await page.getByRole("link", { name: "Budget", exact: true }).click();
	await expect(page).toHaveURL(/\/budget$/);
	await expect(page.getByRole("tab", { name: "Files" })).toHaveAttribute("aria-selected", "true");
	await expect(panel(page, "files").locator('[data-slot="file-tree"]')).toBeVisible();

	// It is remembered, not merely kept alive: a reload reads it back.
	await page.reload();
	await expect(page.getByRole("tab", { name: "Files" })).toHaveAttribute("aria-selected", "true");
	await expect(panel(page, "files").locator('[data-slot="file-tree"]')).toBeVisible();
	await expect(panel(page, "accounts")).toBeHidden();
});

test("arrow keys move between the tabs, Home and End jump to the ends", async ({ page }) => {
	await page.goto("/ledger");
	const accountsTab = page.getByRole("tab", { name: "Accounts" });
	const filesTab = page.getByRole("tab", { name: "Files" });

	await accountsTab.focus();
	await page.keyboard.press("ArrowRight");
	await expect(filesTab).toBeFocused();
	await expect(filesTab).toHaveAttribute("aria-selected", "true");
	await expect(panel(page, "files")).toBeVisible();

	await page.keyboard.press("ArrowLeft");
	await expect(accountsTab).toBeFocused();
	await expect(accountsTab).toHaveAttribute("aria-selected", "true");
	await expect(panel(page, "accounts")).toBeVisible();

	await page.keyboard.press("Home");
	await expect(accountsTab).toBeFocused();
	await page.keyboard.press("End");
	await expect(filesTab).toBeFocused();

	// The arrows wrap, so a reader never gets stuck at an end.
	await page.keyboard.press("ArrowRight");
	await expect(accountsTab).toBeFocused();
	await expect(accountsTab).toHaveAttribute("aria-selected", "true");
});

test("each tab says it has nothing when the backend lists no rows", async ({ page }) => {
	await page.goto("/ledger?empty=1");

	// Let both listings land first: the empty messages are also the pre-fetch
	// state, so asserting before the answers arrive would prove nothing.
	await expect
		.poll(async () => {
			const calls = await invokes(page);
			return ["accounts", "journal_files"].every((cmd) => calls.some((call) => call.cmd === cmd));
		})
		.toBe(true);

	// The account tree keeps no empty "All accounts" row to click.
	await expect(panel(page, "accounts").getByText("No accounts")).toBeVisible();
	await expect(panel(page, "accounts").getByText("All accounts")).toHaveCount(0);

	await page.getByRole("tab", { name: "Files" }).click();
	await expect(panel(page, "files").getByText("No journal files")).toBeVisible();
	await expect(panel(page, "files").locator('[data-slot="file-tree"]')).toHaveCount(0);
});

test.describe("the phone sheet", () => {
	test.use({ viewport: { width: 390, height: 844 } });

	test("the tabs are usable in the sheet, and the sheet closes on navigation", async ({
		page,
	}) => {
		await page.goto("/account/Assets:Bank");
		const trigger = page.getByRole("button", { name: "Open navigation" });
		await trigger.click();

		const sheet = page.getByRole("dialog", { name: "Sidebar" });
		await expect(sheet).toBeVisible();
		const tabs = sheet.getByRole("tablist", { name: "Sidebar sections" });
		await expect(tabs).toBeVisible();

		const filesTab = tabs.getByRole("tab", { name: "Files" });
		await expect(filesTab).toBeVisible();
		await filesTab.click();
		await expect(filesTab).toHaveAttribute("aria-selected", "true");
		await expect(sheet.locator("#sidebar-panel-files")).toBeVisible();
		await expect(sheet.locator('#sidebar-panel-files [data-slot="file-tree"]')).toBeVisible();
		await expect(sheet.locator("#sidebar-panel-accounts")).toBeHidden();

		// A tap on a navigation row navigates and closes the sheet.
		await sheet.getByRole("link", { name: "Ledger", exact: true }).click();
		await expect(page).toHaveURL(/\/ledger$/);
		await expect(sheet).toBeHidden();

		// The next open lands on the tab the reader left it on.
		await trigger.click();
		await expect(sheet.getByRole("tab", { name: "Files" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
	});
});
