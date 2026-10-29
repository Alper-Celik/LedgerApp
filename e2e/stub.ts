// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";
import type { AccountBalance, TransactionEdit, TransactionView } from "../src/bindings";

/**
 * The Tauri IPC stub the e2e tests run against: fixtures defined here, never a
 * real ledger, so the tests carry no private data and need no desktop shell.
 * Every invocation is recorded on `window.__invokes`, which is how a test checks
 * the payload the UI actually sent — those payloads are the contract with Rust.
 */

export type Invoke = { cmd: string; args: unknown };

declare global {
	interface Window {
		/** Calls the stub has served, in order. */
		__invokes: Invoke[];
		/** Tauri's bridge, replaced by the stub. */
		__TAURI_INTERNALS__: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
	}
}

/** What `load_ledger` / `ledger_summary` answer: the journal the stub pretends is open. */
export const summary = {
	path: "/ledger/main.bean",
	directive_count: 12,
	error_count: 0,
};

/** The folders the picker walks: the root, then the ledger-bearing `Books`. */
export const folders = {
	path: "/ledger",
	parent: "/",
	ledger: false,
	folders: [
		{ name: "Archive", path: "/ledger/Archive", ledger: false },
		{ name: "Books", path: "/ledger/Books", ledger: true },
	],
};

export const books = {
	path: "/ledger/Books",
	parent: "/ledger",
	ledger: true,
	folders: [{ name: "2026", path: "/ledger/Books/2026", ledger: false }],
};

export const accounts: AccountBalance[] = [
	{ name: "Assets", balance: "60386.05", currency: "TRY", modified: true },
	{ name: "Assets:Bank", balance: "1500.00", currency: "USD", modified: true },
	{ name: "Assets:Bank:Checking", balance: "-123.45", currency: "TRY", modified: true },
	{ name: "Liabilities:KYK", balance: "-45000.00", currency: "TRY", modified: false },
];

/**
 * What `accounts` answers once another journal has been opened: the same
 * account names with different money, so a sidebar that keeps showing the old
 * balances is visibly stale rather than merely stale in principle.
 */
export const booksAccounts: AccountBalance[] = [
	{ name: "Assets:Bank", balance: "999.00", currency: "TRY", modified: false },
];

export const transactions: TransactionView[] = [
	{
		id: 5,
		date: "2026-01-14",
		flag: "*",
		payee: null,
		narration: "Fees and top-up",
		modified: true,
		postings: [
			{ account: "Assets:Bank:Checking", units: "-100.00", currency: "TRY" },
			{ account: "Assets:Bank:Emergency", units: "40.00", currency: "TRY" },
			{ account: "Expenses:Fees", units: "60.00", currency: "TRY" },
		],
	},
	{
		id: 3,
		date: "2026-01-10",
		flag: "*",
		payee: "Migros",
		narration: "weekly groceries",
		modified: false,
		postings: [
			{ account: "Assets:Bank:Checking", units: "-123.45", currency: "TRY" },
			{ account: "Expenses:Food:Groceries", units: "123.45", currency: "TRY" },
		],
	},
];

/** The month the budget screen shows, as the backend would compute it. */
export const budgetMonth = {
	month: "2026-01",
	months: ["2025-12", "2026-01"],
	groups: [
		{
			name: "Food",
			budgeted: "1000.00",
			spent: "123.45",
			balance: "876.55",
			envelopes: [
				{
					name: "Groceries",
					full_name: "Equity:Envelope:Food:Groceries",
					budgeted: "1000.00",
					spent: "123.45",
					balance: "876.55",
					rollover: true,
				},
			],
		},
	],
	to_budget: "250.00",
	currency: "TRY",
};

/** The journal's files, as the file explorer lists them. */
export const journalFiles = [
	{ path: "/ledger/main.bean", label: "main.bean", transactions: 2, change: "modified" },
	{ path: "/ledger/2026/01.bean", label: "2026/01.bean", transactions: 0, change: "unchanged" },
];

/** The root journal's text, which the editor's whole-file mode loads. */
export const fileText = `2026-01-10 * "Migros" "weekly groceries"
  Assets:Bank:Checking   -123.45 TRY
  Expenses:Food:Groceries  123.45 TRY
`;

/** The ledger's git repository, as the settings screen reads it. */
export const gitState = {
	repo: true,
	root: "/ledger",
	user_name: "Alper Çelik",
	user_email: "alper@example.com",
};

/** Two months of the same envelope budget, as the multi-month grid reads it. */
export const budgetRange = {
	from: "2025-12",
	to: "2026-01",
	currency: "TRY",
	months: [
		{
			month: "2025-12",
			to_budget: "150.00",
			budgeted: "1000.00",
			spent: "80.00",
			balance: "920.00",
			groups: [
				{
					name: "Food",
					budgeted: "1000.00",
					spent: "80.00",
					balance: "920.00",
					envelopes: [
						{
							name: "Groceries",
							full_name: "Equity:Envelope:Food:Groceries",
							budgeted: "1000.00",
							spent: "80.00",
							balance: "920.00",
							rollover: true,
						},
					],
				},
			],
		},
		{
			month: "2026-01",
			to_budget: "250.00",
			budgeted: "1000.00",
			spent: "123.45",
			balance: "876.55",
			groups: [
				{
					name: "Food",
					budgeted: "1000.00",
					spent: "123.45",
					balance: "876.55",
					envelopes: [
						{
							name: "Groceries",
							full_name: "Equity:Envelope:Food:Groceries",
							budgeted: "1000.00",
							spent: "123.45",
							balance: "876.55",
							rollover: true,
						},
					],
				},
			],
		},
	],
};

/** One saved dashboard, as the editor and the rendered view read it. */
export const dashboards = [
	{
		id: "overview",
		name: "Overview",
		widgets: [
			{ id: "w-net", kind: "net-worth", config: {}, span: 4, order: 0 },
			{ id: "w-cats", kind: "category-share", config: {}, span: 2, order: 1 },
			{ id: "w-life", kind: "lifestyle-inflation", config: {}, span: 2, order: 2 },
		],
	},
];

/** One price index, as the index store reports it. */
export const indexSeries = [
	{
		id: "CPI:TRY",
		label: "TÜİK CPI (TR)",
		source: "tuik",
		unofficial: false,
		frequency: "monthly",
		currency: "TRY",
		points: [
			{ date: "2025-11-01", value: "1000.0" },
			{ date: "2025-12-01", value: "1032.5" },
			{ date: "2026-01-01", value: "1071.9" },
		],
	},
];

/** Lifestyle inflation: nominal vs index-adjusted spending per category. */
export const lifestyle = {
	index: { id: "CPI:TRY", label: "TÜİK CPI (TR)", change: "3.81" },
	months: [
		{ month: "2025-12", index_value: "1032.5" },
		{ month: "2026-01", index_value: "1071.9" },
	],
	totals: [
		{ month: "2025-12", nominal: "80.00", real: "80.00", currency: "TRY" },
		{ month: "2026-01", nominal: "123.45", real: "118.93", currency: "TRY" },
	],
	categories: [
		{
			account: "Expenses:Food",
			label: "Food",
			currency: "TRY",
			months: [
				{ month: "2025-12", nominal: "80.00", real: "80.00" },
				{ month: "2026-01", nominal: "123.45", real: "118.93" },
			],
			nominal_change: "54.31",
			real_change: "48.66",
		},
	],
};

export const priceSources = [
	{ name: "ecb", description: "European Central Bank reference rates", official: true },
	{ name: "tefas", description: "Turkish fund NAVs (TEFAS)", official: false },
	{ name: "yahoo", description: "Yahoo Finance quotes", official: false },
];

export const priceTargets = [
	{
		commodity: "USD",
		currency: "TRY",
		kind: "currency",
		latest: { commodity: "USD", currency: "TRY", date: "2026-01-30", price: "43.12", source: "ecb" },
	},
	{ commodity: "AFT", currency: "TRY", kind: "fund", latest: null },
];

export const priceQuotes = [
	{ commodity: "AFT", currency: "TRY", date: "2026-01-30", price: "1.2345", source: "tefas" },
];

export const source = `2026-01-10 * "Migros" "weekly groceries"
  Assets:Bank:Checking   -123.45 TRY
  Expenses:Food:Groceries  123.45 TRY
`;

export const diff = `diff --git a/main.bean b/main.bean
index 1a2b3c4..5d6e7f8 100644
--- a/main.bean
+++ b/main.bean
@@ -41,6 +41,6 @@
 2026-01-10 * "Migros" "weekly groceries"
   Assets:Bank:Checking   -123.45 TRY
-2026-01-14 * "Fees and top-up"
+2026-01-14 * "Fees and top-up" "revised fee split"
   Assets:Bank:Checking   -100.00 TRY
`;

/** Installs the stub before any app script runs, and records every call. */
export async function stubIpc(page: Page) {
	await page.addInitScript(
		(fixtures) => {
			const log: Invoke[] = [];
			/**
			 * Whether another journal has been opened. `accounts` answers from the
			 * new journal's balances after that, the way the backend's own state
			 * has already swapped by the time the UI refetches.
			 */
			let opened = false;
			const internals = {
				invoke: (cmd: string, args: unknown) => {
					log.push({ cmd, args });
					const path = (args as { path?: string | null } | null)?.path ?? null;
					if (window.location.search.includes("fail=1")) {
						return Promise.reject(new Error("no ledger"));
					}
					// Commands returning `Result<T, E>` are wrapped by the generated
					// `typedError` helper, so the stub resolves with the *raw* value the
					// Rust command would produce; rejecting models `Err`.
					switch (cmd) {
						case "accounts":
							return Promise.resolve(opened ? fixtures.booksAccounts : fixtures.accounts);
						case "account_transactions":
							return Promise.resolve(fixtures.transactions);
						case "transaction_source":
							return Promise.resolve(fixtures.source);
						case "update_transaction":
						case "update_transaction_source":
							return Promise.resolve(null);
						case "ledger_diff":
							return Promise.resolve(fixtures.diff);
						case "commit_ledger":
							return Promise.resolve("abc1234");
						case "ledger_summary":
							return Promise.resolve(fixtures.summary);
						case "load_ledger":
							// `typedError`: reject to model `Err`; resolve the raw summary.
							if (path !== "/ledger/Books") {
								return Promise.reject(`no root journal (main.bean, …) in ${path ?? "main.bean"}`);
							}
							opened = true;
							return Promise.resolve({ ...fixtures.summary, path: "/ledger/Books/main.bean" });
						case "budget":
							return Promise.resolve(fixtures.budgetMonth);
						case "journal_files":
							return Promise.resolve(fixtures.journalFiles);
						case "file_transactions":
							return Promise.resolve(fixtures.transactions);
						case "file_text":
							return Promise.resolve(fixtures.fileText);
						case "update_file_text":
						case "add_transaction":
							return Promise.resolve(null);
						case "suggest_transaction_file":
							return Promise.resolve(fixtures.journalFiles[0].path);
						case "git_state":
							// `?norepo=1` models a ledger that is not under git yet.
							return Promise.resolve(
								window.location.search.includes("norepo=1")
									? { repo: false, root: null, user_name: null, user_email: null }
									: fixtures.gitState
							);
						case "git_init":
						case "git_set_identity":
							return Promise.resolve(null);
						case "lsp_start":
							return Promise.resolve(true);
						case "lsp_send":
							return Promise.resolve(null);
						case "lsp_file_path":
							return Promise.resolve(`file://${path}`);
						case "list_folders":
							return Promise.resolve(path === "/ledger/Books" ? fixtures.books : fixtures.folders);
						case "budget_range":
							return Promise.resolve(fixtures.budgetRange);
						case "dashboards":
							return Promise.resolve(fixtures.dashboards);
						case "save_dashboard":
						case "delete_dashboard":
							return Promise.resolve(null);
						case "index_series":
							return Promise.resolve(fixtures.indexSeries);
						case "index_import":
						case "index_refresh":
							return Promise.resolve(fixtures.indexSeries[0]);
						case "lifestyle_report":
							return Promise.resolve(fixtures.lifestyle);
						case "price_sources":
							return Promise.resolve(fixtures.priceSources);
						case "price_targets":
							return Promise.resolve(fixtures.priceTargets);
						case "fetch_prices":
							return Promise.resolve(fixtures.priceQuotes);
						case "write_prices":
							return Promise.resolve(null);
						case "pick_ledger_folder":
							// The desktop's own chooser (`typedError`: reject models
							// `Err`). `?native=folder` models a runtime that draws one and
							// the user picking `Books`; every other run models a runtime
							// without a chooser, which the in-app picker falls back on.
							return window.location.search.includes("native=folder")
								? Promise.resolve("/ledger/Books")
								: Promise.reject("this runtime has no native folder chooser");
						default:
							return Promise.resolve({ status: "ok", data: null });
					}
				},
				transformCallback: (cb: unknown) => cb,
			};
			// Non-writable so a dev-time init script cannot replace the stub.
			Object.defineProperty(window, "__TAURI_INTERNALS__", {
				value: internals,
				writable: false,
				configurable: false,
			});
			Object.defineProperty(window, "__invokes", { value: log, writable: false });
		},
		{
			accounts,
			booksAccounts,
			transactions,
			source,
			diff,
			summary,
			folders,
			books,
			budgetMonth,
			journalFiles,
			fileText,
			gitState,
			budgetRange,
			dashboards,
			indexSeries,
			lifestyle,
			priceSources,
			priceTargets,
			priceQuotes,
		}
	);
}

/** The recorded calls, in order. */
export function invokes(page: Page): Promise<Invoke[]> {
	return page.evaluate(() => window.__invokes);
}

/** Narrows a recorded `update_transaction` argument to the UI's payload type. */
export function isTransactionEdit(value: unknown): value is TransactionEdit {
	return (
		typeof value === "object" &&
		value !== null &&
		"id" in value &&
		typeof value.id === "number" &&
		"date" in value &&
		typeof value.date === "string" &&
		"narration" in value &&
		typeof value.narration === "string" &&
		"postings" in value &&
		Array.isArray(value.postings)
	);
}

/** The `edit` argument of the most recent call to `cmd`, validated. */
export function editArgument(calls: Invoke[], cmd: string): TransactionEdit | undefined {
	const call = [...calls].reverse().find((entry) => entry.cmd === cmd);
	if (!call || typeof call.args !== "object" || call.args === null || !("edit" in call.args)) return undefined;
	const edit = call.args.edit;
	return isTransactionEdit(edit) ? edit : undefined;
}
