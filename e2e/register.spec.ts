// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "./gate";
import { editArgument, invokes, stubIpc } from "./stub";

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

	// A backend that rejects must leave the app standing: no thrown render. The
	// git views are gone in that state on purpose — `git_state` failed, so the
	// sidebar cannot claim the ledger is a repository and offers no diff/commit.
	await page.goto("/ledger?fail=1");
	// The page's own alert, not the sync bar's status line, which names the
	// same failure in the banner above it.
	await expect(page.getByRole("alert").filter({ hasText: "no ledger" })).toBeVisible();
	await expect(page.getByRole("link", { name: "Diff", exact: true })).toHaveCount(0);
	await expect(page.getByRole("link", { name: "Budget", exact: true })).toBeVisible();
});

test("phones get a card per entry, with the register's work still there", async ({ page }) => {
	// A phone: the table's tracks cannot fit, so the register renders cards.
	await page.setViewportSize({ width: 390, height: 844 });
	await page.goto("/account/Assets:Bank");

	await expect(page.getByRole("columnheader")).toHaveCount(0);
	const card = page.getByRole("main").locator('ul > li[data-entry-id="5"]');
	await expect(card).toContainText("60.00");
	await expect(card).toContainText("Expenses:Fees");

	// The card's own head opens the legs under it.
	const head = card.locator("button").first();
	await expect(head).toHaveAttribute("aria-expanded", "false");
	await head.click();
	await expect(head).toHaveAttribute("aria-expanded", "true");
	await expect(card).toContainText("Assets:Bank:Emergency");

	// Editing in place sends the same exact decimal strings the table sends.
	await card.getByRole("button", { name: "Edit Fees and top-up" }).click();
	await expect(card.getByLabel("Narration")).toHaveValue("Fees and top-up");
	await card.getByLabel("Narration").fill("fees, revised");
	await card.getByRole("button", { name: "Save", exact: true }).click();

	await expect
		.poll(async () => editArgument(await invokes(page), "update_transaction"))
		.toMatchObject({ id: 5, narration: "fees, revised" });
	const edit = editArgument(await invokes(page), "update_transaction");
	expect(edit?.postings).toEqual([
		{ index: 0, units: "-100.00" },
		{ index: 1, units: "40.00" },
		{ index: 2, units: "60.00" },
	]);
});

test("the ledger-wide views the sidebar links to render", async ({ page }) => {
	await page.goto("/ledger");

	// Budget: the month's envelopes and the pool left to assign.
	await page.getByRole("link", { name: "Budget", exact: true }).click();
	await expect(page.getByText("Food")).toBeVisible();
	// The group total, the envelope row and the balance pill all carry it.
	await expect(page.getByText("876.55").first()).toBeVisible();

	// Dashboards: the list of them.
	await page.getByRole("link", { name: "Dashboards", exact: true }).click();
	await expect(page.getByRole("heading", { name: "Dashboards" })).toBeVisible();

	// Files: the sidebar's Files tab lists the journal's files; the root one opens.
	await page.getByRole("tab", { name: "Files" }).click();
	await expect(page.getByRole("link", { name: /main\.bean/ })).toBeVisible();
	await page.getByRole("link", { name: /main\.bean/ }).click();
	await expect(page.getByRole("heading", { name: "main.bean" })).toBeVisible();
	await expect(page.getByText("weekly groceries").first()).toBeVisible();
});

test("the new screens mount with their empty states", async ({ page }) => {
	// Branches: the stub reports a repository, so the screen mounts.
	await page.goto("/branches");
	await expect(page.getByRole("heading", { name: "Branches", level: 1 })).toBeVisible();

	// Inflation: the fixture's stored series is listed, with its own controls.
	await page.goto("/inflation");
	await expect(page.getByRole("heading", { name: /TÜİK CPI/ })).toBeVisible();
	await expect(page.getByRole("button", { name: /Refresh/ }).first()).toBeVisible();

	// Conflicts: the sync client left nothing behind.
	await page.goto("/conflicts");
	await expect(page.getByText("No conflict copies")).toBeVisible();

	// History: the operation log is empty on a fresh ledger.
	await page.goto("/history");
	await expect(page.getByText("No writes recorded yet")).toBeVisible();
});

test("a commit in the branches screen opens its patch", async ({ page }) => {
	await page.goto("/branches");

	// The log row is the fixture commit; clicking it loads the patch.
	const row = page.getByText("weekly groceries for January").first();
	await expect(row).toBeVisible();
	await row.click();
	await expect(page.getByText(/Assets:Bank:Checking\s+-123\.45 TRY/)).toBeVisible();

	// Both branches the fixture reports are listed, the current one marked. Scoped
	// to the screen: the sidebar's file tree also holds a "main.bean", hidden
	// behind its Files tab.
	await expect(page.getByRole("main").getByText("main", { exact: true }).first()).toBeVisible();
});

test("the python sandbox boots in the app's origin and runs a script", async ({ page }) => {
	// Pyodide's first boot downloads the runtime into the worker (app assets, ~20 MB
	// with the vendored wheels) and parses it: minutes, not seconds, on a cold cache.
	test.setTimeout(180_000);
	// The sandbox is exercised directly rather than through a model turn: what
	// this proves is that Pyodide's assets resolve from the app itself, the
	// worker runs under this origin, and the `ledger` bridge is reachable.
	await page.goto("/assistant");

	const run = await page.evaluate(async () => {
		const sandbox = await import("/src/lib/assistant/python/sandbox.ts");
		return sandbox.runPython(
			[
				"import ledger",
				"accounts = await ledger.accounts()",
				"ledger.table('accounts', accounts)",
				"print('python-ok', len(accounts))",
			].join("\n"),
			{ timeoutMs: 120_000, maxOutput: 10_000 },
			// The bridge answers `{ ok, text }`; text is the JSON the Python side decodes.
			async () => ({
				ok: true,
				text: JSON.stringify([{ name: "Assets:Bank", balance: "1.00", currency: "TRY" }]),
			})
		);
	});

	expect(run.error).toBe("");
	expect(run.stdout).toContain("python-ok 1");
	expect(run.tables).toHaveLength(1);
	expect(run.tables[0].rows[0]).toContain("Assets:Bank");
});
