// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";
import { expect, test } from "./gate";
import { invokes, stubIpc } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

/** The write commands the screen is expected to send, in invocation order. */
async function calls(page: Page, cmd: string) {
	return (await invokes(page)).filter((call) => call.cmd === cmd);
}

test("set up writes the envelope configuration the form collected", async ({ page }) => {
	// Unconfigured first: the backend refuses the range read and budget_settings
	// has no config, which is the screen's setup state. The journal does not
	// declare the plugin yet, but its module is beside the journal, so the same
	// write adds the declaration and the sheet names the file it found.
	await page.goto("/budget?budget_setup=1&budget_no_plugin=1");
	await page.getByRole("button", { name: "Set up the envelope budget" }).click();

	const dialog = page.getByRole("dialog", { name: "Set up the envelope budget" });
	await expect(dialog).toBeVisible();
	const pluginNote = dialog.getByRole("alert").filter({ hasText: "rustledger_envelope.wasm" });
	await expect(pluginNote).toContainText("does not declare the envelope plugin");
	await expect(pluginNote).toContainText("/ledger/rustledger_envelope.wasm");
	await expect(pluginNote).toContainText("(found)");

	// The root journal is the loader's first file, and is preselected.
	await expect(dialog.getByRole("button", { name: "Target journal file" })).toContainText(
		"main.bean"
	);

	// The defaults the form promises: this month, twelve ahead, ^Assets:, assert
	// on, strict off.
	await expect(dialog.getByLabel("Months ahead")).toHaveValue("12");
	await expect(dialog.getByLabel("Emit month-end assertions")).toBeChecked();
	await expect(dialog.getByLabel("Warn about never-funded spending")).not.toBeChecked();

	await dialog.getByLabel("Start month").fill("2026-03");
	await dialog.getByLabel("Months ahead").fill("6");
	await dialog.getByRole("button", { name: "Add pattern" }).click();
	await dialog.getByRole("textbox", { name: "Budget account pattern 2" }).fill("^Liabilities:");
	await dialog.getByLabel("Emit month-end assertions").uncheck();
	await dialog.getByLabel("Warn about never-funded spending").check();
	await dialog.getByRole("button", { name: "Write configuration" }).click();

	await expect.poll(async () => (await calls(page, "budget_init")).length).toBe(1);
	const [init] = await calls(page, "budget_init");
	expect(init?.args).toEqual({
		file: "/ledger/main.bean",
		start: "2026-03",
		monthsAhead: 6,
		end: null,
		budgetAccounts: ["^Assets:", "^Liabilities:"],
		assert: false,
		strict: true,
	});

	// Success closes the sheet and the reloaded configuration brings the grid in.
	await expect(dialog).toHaveCount(0);
	await expect(
		page.getByRole("button", {
			name: "Edit budgeted amount for Expenses:Food:Groceries from 2026-01",
		})
	).toBeVisible();
});

test("a missing plugin module is refused and the journal is not written", async ({ page }) => {
	// The journal has no configuration and no module: setup must refuse before
	// writing anything, and the sheet keeps the draft with the sentence that
	// names the path.
	await page.goto("/budget?budget_setup=1&budget_no_plugin=1&budget_wasm_missing=1");
	await page.getByRole("button", { name: "Set up the envelope budget" }).click();

	const dialog = page.getByRole("dialog", { name: "Set up the envelope budget" });
	await expect(dialog).toBeVisible();
	await expect(
		dialog.getByRole("alert").filter({ hasText: "not found yet" })
	).toBeVisible();

	await dialog.getByLabel("Start month").fill("2026-03");
	await dialog.getByRole("button", { name: "Write configuration" }).click();

	await expect(
		dialog.getByRole("alert").filter({ hasText: "nothing was written" })
	).toBeVisible();
	await expect(dialog).toBeVisible();
	await expect.poll(async () => (await calls(page, "budget_init")).length).toBe(1);
});

test("edit rewrites the existing configuration in place", async ({ page }) => {
	await page.goto("/budget");
	await page.getByRole("button", { name: "Edit configuration" }).click();

	const dialog = page.getByRole("dialog", { name: "Edit the envelope budget" });
	await expect(dialog).toBeVisible();
	// The form opens on what the ledger already says, and the target file is
	// the one holding the block rather than a chooser.
	await expect(dialog.getByLabel("Start month")).toHaveValue("2025-12");
	await expect(dialog.getByLabel("Months ahead")).toHaveValue("12");
	await expect(dialog.getByText("/ledger/main.bean")).toBeVisible();

	await dialog.getByLabel("Start month").fill("2026-01");
	await dialog.getByRole("radio", { name: "End month" }).click();
	await dialog.getByLabel("End month").fill("2026-12");
	await dialog.getByRole("button", { name: "Save configuration" }).click();

	await expect.poll(async () => (await calls(page, "budget_config_set")).length).toBe(1);
	const [save] = await calls(page, "budget_config_set");
	expect(save?.args).toEqual({
		file: "/ledger/main.bean",
		start: "2026-01",
		monthsAhead: null,
		end: "2026-12",
		budgetAccounts: ["^Assets:"],
		assert: true,
		strict: false,
	});
	await expect(dialog).toHaveCount(0);
});

test("editing a Budgeted cell writes a dated fund directive for its month", async ({ page }) => {
	await page.goto("/budget");
	const cell = page.getByRole("button", {
		name: "Edit budgeted amount for Expenses:Food:Groceries from 2026-01",
	});
	await expect(cell).toBeVisible();
	await cell.click();

	const field = page.getByLabel("Budgeted from 2026-01 for Expenses:Food:Groceries");
	await expect(field).toHaveValue("1000.00");
	await field.fill("1250.50");
	await field.press("Enter");

	await expect.poll(async () => (await calls(page, "budget_fund_set")).length).toBe(1);
	const [call] = await calls(page, "budget_fund_set");
	expect(call?.args).toEqual({
		file: "/ledger/main.bean",
		account: "Expenses:Food:Groceries",
		units: "1250.50",
		currency: "TRY",
		month: "2026-01",
	});
	// The editor closes only after the write is accepted, and the reloaded grid
	// is what remains — no optimistic figure was written into the cell.
	await expect(field).toHaveCount(0);
});

test("Escape cancels a Budgeted draft without writing", async ({ page }) => {
	await page.goto("/budget");
	await page
		.getByRole("button", {
			name: "Edit budgeted amount for Expenses:Food:Groceries from 2026-01",
		})
		.click();

	const field = page.getByLabel("Budgeted from 2026-01 for Expenses:Food:Groceries");
	await field.fill("999.00");
	await field.press("Escape");

	await expect(field).toHaveCount(0);
	expect(await calls(page, "budget_fund_set")).toHaveLength(0);
});

test("a refused fund keeps the draft and shows the backend's sentence", async ({ page }) => {
	await page.goto("/budget?budget_fail=fund");
	await page
		.getByRole("button", {
			name: "Edit budgeted amount for Expenses:Food:Groceries from 2026-01",
		})
		.click();

	const field = page.getByLabel("Budgeted from 2026-01 for Expenses:Food:Groceries");
	await field.fill("42.00");
	await field.press("Enter");

	await expect(
		page.getByText("that fund would make the month's funding negative, which the plugin refuses")
	).toBeVisible();
	await expect(field).toHaveValue("42.00");
	expect(await calls(page, "budget_fund_set")).toHaveLength(1);
});

test("the rollover control sends carry, reset and a cap with its amount", async ({ page }) => {
	await page.goto("/budget");
	const control = page.getByRole("button", {
		name: "Rollover policy for Expenses:Food:Groceries: carry",
	});
	await expect(control).toBeVisible();

	// A category with no fund/rollover directive yet says so.
	await expect(
		page.getByRole("button", { name: "Rollover policy for Expenses:Food:Dining: not set" })
	).toBeVisible();

	await control.click();
	await page.getByRole("menuitem", { name: "Reset to zero" }).click();

	await expect.poll(async () => (await calls(page, "budget_rollover_set")).length).toBe(1);
	const [reset] = await calls(page, "budget_rollover_set");
	expect(reset?.args).toEqual({
		file: "/ledger/main.bean",
		account: "Expenses:Food:Groceries",
		kind: "reset",
		units: null,
		currency: null,
		month: "2026-01",
	});

	// A cap asks for its amount in the same menu before writing.
	await expect(control).toBeVisible();
	await control.click();
	await page.getByRole("menuitem", { name: "Carry up to…" }).click();
	await page
		.getByLabel("Cap amount from 2026-01 for Expenses:Food:Groceries")
		.fill("500.00");
	await page.getByRole("button", { name: "Save cap" }).click();

	await expect.poll(async () => (await calls(page, "budget_rollover_set")).length).toBe(2);
	const [, cap] = await calls(page, "budget_rollover_set");
	expect(cap?.args).toEqual({
		file: "/ledger/main.bean",
		account: "Expenses:Food:Groceries",
		kind: "cap",
		units: "500.00",
		currency: "TRY",
		month: "2026-01",
	});
});

test("phones get the same funding and rollover affordances on the card", async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await page.goto("/budget");

	const cell = page.getByRole("button", {
		name: "Edit budgeted amount for Expenses:Food:Groceries from 2026-01",
	});
	await expect(cell).toBeVisible();
	await expect(
		page.getByRole("button", { name: "Rollover policy for Expenses:Food:Groceries: carry" })
	).toBeVisible();
	await expect(
		page.getByRole("button", { name: "Rollover policy for Expenses:Food:Dining: not set" })
	).toBeVisible();

	await cell.click();
	const field = page.getByLabel("Budgeted from 2026-01 for Expenses:Food:Groceries");
	await field.fill("80.00");
	await field.press("Enter");

	await expect.poll(async () => (await calls(page, "budget_fund_set")).length).toBe(1);
	const [call] = await calls(page, "budget_fund_set");
	expect(call?.args).toMatchObject({
		account: "Expenses:Food:Groceries",
		units: "80.00",
		currency: "TRY",
		month: "2026-01",
	});
});
