// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { addedEntry, editArgument, invokes, stubIpc, suggestedFor } from "./stub";

test.beforeEach(async ({ page }) => {
	await stubIpc(page);
});

test("the register renders the fixture ledger and its three styles", async ({ page }) => {
	await page.goto("/account/Assets:Bank");

	// Basic ledger: one line per entry, the counter-account named.
	await expect(page.getByRole("heading", { name: "Assets:Bank" })).toBeVisible();
	// Actions, Date, Description: the description cell carries the narration and,
	// for an uncommitted entry, the marker beside it — so it is matched on the
	// narration it contains rather than on its exact accessible name.
	const entry = page.getByRole("row", { name: /Fees and top-up/ }).first();
	await expect(entry.getByRole("cell").nth(2)).toContainText("Fees and top-up");
	await expect(page.getByRole("cell", { name: "Expenses:Fees" })).toBeVisible();

	await page.getByRole("radio", { name: "Splits", exact: true }).click();
	await expect(page.getByRole("columnheader", { name: "Tot Withdrawal" })).toBeVisible();
	await expect(page.getByText("₺100.00").first()).toBeVisible();

	await page.getByRole("radio", { name: "Full", exact: true }).click();
	await expect(page.getByRole("cell", { name: "Expenses:Food:Groceries" }).first()).toBeVisible();
});

test("an uncommitted entry is marked, and only that one", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	const markedRow = page.getByRole("row", { name: /Fees and top-up/ }).first();
	const cleanRow = page.getByRole("row", { name: /Migros — weekly groceries/ }).first();
	// "Marked" is the entry's own left marker; no colour is pinned here, so the
	// theme can move without this test having to be re-pinned to it.
	await expect(markedRow.getByRole("cell").first()).toHaveClass(/border-s-added/);
	await expect(cleanRow.getByRole("cell").first()).not.toHaveClass(/border-s-added/);
	// The marker proper: the dot the sidebar's account tree draws, one on the
	// uncommitted entry and none on the clean one.
	await expect(markedRow.locator('[data-slot="uncommitted-mark"]')).toHaveCount(1);
	await expect(cleanRow.locator('[data-slot="uncommitted-mark"]')).toHaveCount(0);
});

test("editing a cell in place sends exact decimal strings, never floats", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	// Amount fields live on the leg rows, which the split styles show.
	await page.getByRole("radio", { name: "Splits", exact: true }).click();
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

test("a new transaction is composed inline, filed where the backend suggests, and sent", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	await page.getByRole("button", { name: "New transaction" }).click();

	// The draft is a row of the register itself — no dialog — holding an entry
	// that is not in the ledger yet: the register's own account on the first
	// posting, and the file the backend deduced from the accounts, asked about
	// those accounts and no others.
	await expect(page.getByLabel("Posting 1 account")).toHaveValue("Assets:Bank");
	await expect
		.poll(async () => suggestedFor(await invokes(page)))
		.toEqual(["Assets:Bank"]);
	await expect(page.getByRole("combobox", { name: "File" })).toHaveValue("/books/2026.bean");

	await page.getByLabel("Narration").fill("Coffee");
	await page.getByLabel("Posting 1 amount").fill("-3.50");
	await page.getByLabel("Posting 2 account").fill("Expenses:Food");
	await page.getByLabel("Posting 2 amount").fill("3.50");
	// The accounts are what the deduction follows, so the ask follows them too.
	await expect
		.poll(async () => suggestedFor(await invokes(page)))
		.toEqual(["Assets:Bank", "Expenses:Food"]);

	// A file picked by hand beats the suggestion, and is what gets written to.
	await page.getByRole("combobox", { name: "File" }).selectOption("/books/main.bean");
	// Enter is the register's own commit key, and the draft answers to it too.
	await page.getByLabel("Narration").press("Enter");

	await expect.poll(async () => addedEntry(await invokes(page))).toMatchObject({
		file: "/books/main.bean",
		flag: "*",
		payee: null,
		narration: "Coffee",
		// The amounts are the strings that were typed, and the commodities the
		// fields were left empty on are the backend's to fill in.
		postings: [
			{ account: "Assets:Bank", units: "-3.50", currency: null },
			{ account: "Expenses:Food", units: "3.50", currency: null },
		],
	});
	// The draft is gone with the entry written, and the register still stands.
	await expect(page.getByLabel("Narration")).toHaveCount(0);
	await expect(page.getByRole("button", { name: "Add transaction" })).toHaveCount(0);
	await expect(page.getByRole("heading", { name: "Assets:Bank" })).toBeVisible();
});

test("an empty register still takes a new entry", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	// A filter that matches nothing hides every row, but the register — and with
	// it the draft — has to stay: an empty account is where an entry is most
	// likely to be added.
	await page.getByLabel("Search transactions").fill("nothing matches this");
	await expect(page.getByText("No transactions match the filter.")).toBeVisible();
	await page.getByRole("button", { name: "New transaction" }).click();
	await expect(page.getByLabel("Narration")).toBeVisible();
	await expect(page.getByLabel("Posting 1 amount")).toBeVisible();
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
	// `exact` because the account rows' names now carry "Uncommitted", which
	// contains "Commit" — the sidebar's own link is the one named exactly that.
	await expect(page.getByRole("link", { name: "Diff", exact: true })).toBeVisible();
	await expect(page.getByRole("link", { name: "Commit", exact: true })).toBeVisible();

	// A backend that rejects must leave the app standing: no thrown render.
	await page.goto("/ledger?fail=1");
	await expect(page.getByText("no ledger")).toBeVisible();
	await expect(page.getByRole("link", { name: "Diff", exact: true })).toBeVisible();
});
