// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { addedAccount, addedEntry, argsOf, editArgument, invokes, stubIpc, suggestedFor } from "./stub";

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

test("git letters mark what is new and what was rewritten, and nothing else", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	const rewritten = page.getByRole("row", { name: /Fees and top-up/ }).first();
	const fresh = page.getByRole("row", { name: /Salary/ }).first();
	const clean = page.getByRole("row", { name: /Migros — weekly groceries/ }).first();

	// The letter a git client writes beside a file: M for an entry that was
	// rewritten, A for one the last commit does not have, nothing for one that is
	// as committed.
	await expect(rewritten.locator('[data-slot="git-status"]')).toHaveText("M");
	await expect(fresh.locator('[data-slot="git-status"]')).toHaveText("A");
	await expect(clean.locator('[data-slot="git-status"]')).toHaveCount(0);

	// The row's own left marker says the same thing, so the state is never read
	// from a colour alone.
	await expect(rewritten.getByRole("cell").first()).toHaveClass(/border-s-amber-600/);
	await expect(fresh.getByRole("cell").first()).toHaveClass(/border-s-added/);
	await expect(clean.getByRole("cell").first()).not.toHaveClass(/border-s-/);
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
	// The file is the standard select, showing the deduced one as its value.
	await expect(page.getByLabel("File")).toContainText("march.bean");

	await page.getByLabel("Narration").fill("Coffee");
	await page.getByLabel("Posting 1 amount").fill("-3.50");
	await page.getByLabel("Posting 2 account").fill("Expenses:Food");
	await page.getByLabel("Posting 2 amount").fill("3.50");
	// The accounts are what the deduction follows, so the ask follows them too.
	await expect
		.poll(async () => suggestedFor(await invokes(page)))
		.toEqual(["Assets:Bank", "Expenses:Food"]);

	// A file picked by hand beats the suggestion, and is what gets written to.
	await page.getByLabel("File").click();
	await page.getByRole("option", { name: "main.bean" }).click();
	await expect(page.getByLabel("File")).toContainText("main.bean");
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

test("the sidebar's file explorer is a tree, and opens a file as entries and as text", async ({ page }) => {
	await page.goto("/account/Assets:Bank");

	// The accounts and the files share the sidebar's list place, so they are a tab
	// each.
	await page.getByRole("radio", { name: "Files" }).click();

	// What the paths have in common is a folder, and it carries the strongest
	// change under it, so a folded folder still says something inside moved.
	const folder = page.getByRole("button", { name: /^2026/ });
	await expect(folder.locator('[data-slot="git-status"]')).toHaveText("M");
	const march = page.getByRole("link", { name: /march\.bean/ });
	await expect(march.locator('[data-slot="git-status"]')).toHaveText("M");
	await expect(march).toContainText("2");
	await expect(
		page.getByRole("link", { name: /april\.bean/ }).locator('[data-slot="git-status"]')
	).toHaveText("A");
	// A file git has and nothing has touched carries no letter at all.
	await expect(
		page.getByRole("link", { name: /main\.bean/ }).locator('[data-slot="git-status"]')
	).toHaveCount(0);

	// The folder folds and unfolds, the way an explorer's does. The fold is a
	// clipped grid track, so what is asserted is the control's own state.
	const toggle = page.getByRole("button", { name: "Toggle 2026" });
	await toggle.click();
	await expect(toggle).toHaveAttribute("aria-expanded", "false");
	await toggle.click();
	await expect(toggle).toHaveAttribute("aria-expanded", "true");

	// Opening one goes to its own page, which has already asked the backend for
	// that file's entries.
	await march.click();
	await expect(page.getByRole("heading", { name: "2026/march.bean" })).toBeVisible();
	await expect
		.poll(async () => argsOf(await invokes(page), "file_transactions")?.file)
		.toBe("/books/2026/march.bean");

	// The transactions view is the register's own, with every entry's legs
	// showing: a file is read, so it has no current entry to hide them behind.
	await expect(page.getByRole("row", { name: /Fees and top-up/ })).toBeVisible();
	await expect(page.getByRole("cell", { name: "Expenses:Fees", exact: true })).toBeVisible();
	await expect(page.getByRole("cell", { name: "Assets:Bank:Emergency", exact: true })).toBeVisible();

	// The text mode reads the file itself, and saving hands it back to the same
	// one-file pipeline — the text in the payload is the file's, which only the
	// backend ever held.
	await page.getByRole("radio", { name: "Text" }).click();
	await page.getByRole("button", { name: "Save" }).click();
	await expect
		.poll(async () => argsOf(await invokes(page), "update_file_text")?.file)
		.toBe("/books/2026/march.bean");
	expect(argsOf(await invokes(page), "update_file_text")?.text).toContain("weekly groceries");
});

test("an account is opened from the sidebar, in the file its family lives in", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	await page.getByRole("button", { name: "Add account" }).click();

	// The form is inline in the tree, on today's date, and the file follows the
	// name as it is typed.
	await expect(page.getByLabel("Opened on")).not.toHaveValue("");
	await page.getByLabel("Account name").fill("Expenses:Coffee");
	await expect
		.poll(async () => argsOf(await invokes(page), "suggest_account_file")?.name)
		.toBe("Expenses:Coffee");
	// The standard select shows the deduced file, and a picked one replaces it.
	await expect(page.getByLabel("File")).toContainText("march.bean");
	await page.getByLabel("File").click();
	await page.getByRole("option", { name: "main.bean" }).click();
	await expect(page.getByLabel("File")).toContainText("main.bean");
	await page.getByLabel("Commodities").fill("USD, EUR");
	await page.getByRole("button", { name: "Open account" }).click();

	await expect.poll(async () => addedAccount(await invokes(page))).toMatchObject({
		file: "/books/main.bean",
		name: "Expenses:Coffee",
		currencies: ["USD", "EUR"],
	});
	// The form is gone, and the tree it was opened from is still standing.
	await expect(page.getByLabel("Account name")).toHaveCount(0);
	await expect(page.getByRole("link", { name: "Diff", exact: true })).toBeVisible();
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

test("the budget shows a month's envelopes, and steps through the plugin's months", async ({ page }) => {
	await page.goto("/budget");
	// The load asks for the newest month the ledger has.
	await expect.poll(async () => (await invokes(page)).some((call) => call.cmd === "budget")).toBe(true);

	// The month, the figures it comes down to, and a group with its own totals
	// over one row per envelope, in the three columns Actual Budget uses.
	await expect(page.getByText("March 2026").first()).toBeVisible();
	await expect(page.getByText("To budget")).toBeVisible();
	await expect(page.getByText("In envelopes")).toBeVisible();
	await expect(page.getByRole("row", { name: /^Food/ })).toContainText("176.55");
	await expect(page.getByRole("row", { name: /Groceries/ })).toContainText("200.00");
	await expect(page.getByRole("row", { name: /Groceries/ })).toContainText("123.45");
	await expect(page.getByRole("row", { name: /Groceries/ })).toContainText("76.55");

	// The arrows step through the plugin's own months, one backend call each.
	await page.getByRole("button", { name: "Previous month" }).click();
	await expect.poll(async () => argsOf(await invokes(page), "budget")?.month).toBe("2026-02");
});

test("the dashboard registry lists its dashboards, and each one renders", async ({ page }) => {
	await page.goto("/dashboards");
	await expect(page.getByRole("link", { name: /Balances/ })).toBeVisible();
	await expect(page.getByRole("link", { name: /Activity/ })).toBeVisible();

	// A dashboard is data from the commands the app already has: the accounts,
	// and the whole ledger's transactions.
	await page.getByRole("link", { name: /Activity/ }).click();
	await expect(page.getByRole("heading", { name: "Activity" })).toBeVisible();
	await expect(page.getByRole("row", { name: /2026-01/ }).first()).toBeVisible();
	await expect(page.getByText("Fees and top-up")).toBeVisible();

	// An id the registry does not know reads as no such dashboard, not as a
	// broken page.
	await page.goto("/dashboards/nonsense");
	await expect(page.getByText(/No dashboard/)).toBeVisible();
	await expect(page.getByRole("link", { name: "All dashboards" })).toBeVisible();
});

test("the sidebar carries the views and survives a failing backend", async ({ page }) => {
	await page.goto("/account/Assets:Bank");
	// `exact` because the account rows' names now carry "Uncommitted", which
	// contains "Commit" — the sidebar's own link is the one named exactly that.
	await expect(page.getByRole("link", { name: "Budget", exact: true })).toBeVisible();
	await expect(page.getByRole("link", { name: "Dashboards", exact: true })).toBeVisible();
	await expect(page.getByRole("link", { name: "Diff", exact: true })).toBeVisible();
	await expect(page.getByRole("link", { name: "Commit", exact: true })).toBeVisible();

	// A backend that rejects must leave the app standing: no thrown render.
	await page.goto("/ledger?fail=1");
	await expect(page.getByText("no ledger")).toBeVisible();
	await expect(page.getByRole("link", { name: "Diff", exact: true })).toBeVisible();
});
