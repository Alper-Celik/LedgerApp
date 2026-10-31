// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";
import type {
	AccountBalance,
	AiSettingsView,
	BudgetSettings,
	TransactionEdit,
	TransactionView,
} from "../src/bindings";

/**
 * The Tauri IPC stub the e2e tests run against: fixtures defined here, never a
 * real ledger, so the tests carry no private data and need no desktop shell.
 * Every invocation is recorded on `window.__invokes`, which is how a test checks
 * the payload the UI actually sent — those payloads are the contract with Rust.
 * An invocation no `case` below answers is recorded on `window.__stubUnknown`
 * (and logged) while still being served permissively, and `e2e/gate.ts` fails
 * the test that made it: a command the app calls but nobody stubbed must not be
 * able to look healthy.
 */

export type Invoke = { cmd: string; args: unknown };

declare global {
	interface Window {
		/** Calls the stub has served, in order. */
		__invokes: Invoke[];
		/**
		 * Commands the stub had no `case` for, first occurrence only. The stub
		 * still answers them permissively (a harmless read should not fail a
		 * test), so this array is how `e2e/gate.ts` turns the gap into a failure.
		 */
		__stubUnknown: string[];
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

/** The journal's files, as the file explorer lists them: a nested directory tree. */
export const journalFiles = [
	{ path: "/ledger/main.bean", label: "main.bean", transactions: 2, change: "modified" },
	{ path: "/ledger/accounts/2025.bean", label: "accounts/2025.bean", transactions: 0, change: "unchanged" },
	{ path: "/ledger/accounts/2026.bean", label: "accounts/2026.bean", transactions: 4, change: "added" },
	{
		path: "/ledger/Liabilities/TR:Ziraat/Bankkart/2026/2026-06.bean",
		label: "Liabilities/TR:Ziraat/Bankkart/2026/2026-06.bean",
		transactions: 2,
		change: "modified",
	},
];

/** The repository's history, for the branches screen. */
export const commits = [
	{
		id: "3f2a1c9d8b7e6a5f4c3d2e1b0a9f8e7d6c5b4a39",
		short: "3f2a1c9",
		summary: "weekly groceries for January",
		author: "Alper Çelik",
		email: "alper@example.com",
		time: 1767225600,
		files: 1,
	},
];

/** One commit's patch, in the diff view's own rendering. */
export const commitDiff = `=== main.bean ===
diff --git a/main.bean b/main.bean
--- a/main.bean
+++ b/main.bean
@@ -1 +1,2 @@
 2026-01-10 * "Migros" "weekly groceries"
+  Assets:Bank:Checking  -123.45 TRY
`;

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
							account: "Food:Groceries",
							budgeted: "1000.00",
							spent: "80.00",
							balance: "920.00",
							rollover: true,
						},
						{
							name: "Dining",
							full_name: "Equity:Envelope:Food:Dining",
							account: "Food:Dining",
							budgeted: "0",
							spent: "0",
							balance: "0",
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
							account: "Food:Groceries",
							budgeted: "1000.00",
							spent: "123.45",
							balance: "876.55",
							rollover: true,
						},
						{
							name: "Dining",
							full_name: "Equity:Envelope:Food:Dining",
							account: "Food:Dining",
							budgeted: "0",
							spent: "0",
							balance: "0",
							rollover: true,
						},
					],
				},
			],
		},
	],
};

/**
 * The budget's configuration before setup: no `start date`, so the screen is in
 * its empty state and offers the form. `budget_init` flips the stub to the
 * configured fixture below for the rest of that page's life.
 */
export const budgetSettings: BudgetSettings = {
	plugin_declared: true,
	config: null,
	envelopes: [],
};

/** The same ledger after setup: one fund and rollover on the Groceries envelope. */
export const budgetSettingsConfigured: BudgetSettings = {
	plugin_declared: true,
	config: {
		file: "/ledger/main.bean",
		start: "2025-12",
		end: null,
		months_ahead: 12,
		budget_accounts: ["^Assets:"],
		assert: true,
		strict: false,
	},
	envelopes: [
		{
			account: "Expenses:Food:Groceries",
			fund: { units: "1000.00", currency: "TRY" },
			since: "2025-12",
			rollover: { kind: "carry", amount: null },
			file: "/ledger/main.bean",
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

/** What `index_write_prices` answers: the stored series, now a ledger commodity. */
export const indexPriceWrite = {
	series: "CPI:TRY",
	commodity: "IDX.CPI-TRY",
	currency: "TRY",
	file: "/ledger/main.bean",
	written: 3,
	skipped: 0,
};

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

/**
 * What `fetch_prices` answers: one outcome per request, in request order, in
 * the backend's `PriceFetch` shape — `{ quote }` for an answer, `{ failed }`
 * for a refusal. The frontend reads the tag, so a bare quote here would be
 * reported as "returned no quote".
 */
export const priceFetches = [
	{
		quote: {
			commodity: "AFT",
			currency: "TRY",
			date: "2026-01-30",
			price: "1.2345",
			source: "tefas",
		},
	},
];

/**
 * The assistant, unconfigured: the endpoint's default root, no model chosen and
 * no key stored, which is what a fresh install reports (see `ai.rs`'s
 * `AiConfig::default`). The prompt is a stand-in for `SYSTEM_PROMPT`; nothing
 * under test reads its wording.
 */
export const aiSettings: AiSettingsView = {
	base_url: "https://api.openai.com/v1",
	model: "",
	max_tokens: null,
	temperature: null,
	has_key: false,
	system_prompt: "You are the ledger's assistant.",
};

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
			/** Commands that fell through the switch, in first-seen order. */
			const unknown: string[] = [];
			/** The last id handed out by `plugin:event|listen`. */
			let listener = 0;
			/**
			 * Whether another journal has been opened. `accounts` answers from the
			 * new journal's balances after that, the way the backend's own state
			 * has already swapped by the time the UI refetches.
			 */
			let opened = false;
			/** Whether `budget_init` has run: the setup spec starts before it, everything else after. */
			let budgetConfigured = false;
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
							// `?empty=1` models a journal with no accounts to list.
							return Promise.resolve(
								window.location.search.includes("empty=1")
									? []
									: opened
										? fixtures.booksAccounts
										: fixtures.accounts
							);
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
							// `?no_ledger=1` models the app's first state: no journal
							// open, so the listing is empty and the summary is null.
							return Promise.resolve(
								window.location.search.includes("no_ledger=1")
									? null
									: fixtures.summary
							);
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
							return Promise.resolve(
								window.location.search.includes("no_ledger=1") ||
									window.location.search.includes("empty=1")
									? []
									: fixtures.journalFiles
							);
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
						case "budget_range": {
							const setupPending =
								window.location.search.includes("budget_setup=1") && !budgetConfigured;
							return setupPending
								? Promise.reject(
										"this ledger has no envelope budget data — the envelope plugin has written nothing under `Equity:Envelope:`, so there is no month to show"
									)
								: Promise.resolve(fixtures.budgetRange);
						}
						case "budget_settings": {
							const setupPending =
								window.location.search.includes("budget_setup=1") && !budgetConfigured;
							const base = setupPending
								? fixtures.budgetSettings
								: fixtures.budgetSettingsConfigured;
							return Promise.resolve({
								...base,
								plugin_declared: !window.location.search.includes("budget_no_plugin=1"),
							});
						}
						case "budget_init":
							if (window.location.search.includes("budget_fail=init")) {
								return Promise.reject(
									"the ledger already declares a budget `start date`; edit that directive instead"
								);
							}
							budgetConfigured = true;
							return Promise.resolve(null);
						case "budget_fund_set":
							if (window.location.search.includes("budget_fail=fund")) {
								return Promise.reject(
									"that fund would make the month's funding negative, which the plugin refuses"
								);
							}
							return Promise.resolve(null);
						case "budget_rollover_set":
							if (window.location.search.includes("budget_fail=rollover")) {
								return Promise.reject("a cap needs both units and a currency");
							}
							return Promise.resolve(null);
						case "dashboards":
							return Promise.resolve(fixtures.dashboards);
						case "save_dashboard":
						case "delete_dashboard":
							return Promise.resolve(null);
						case "index_series":
							return Promise.resolve(fixtures.indexSeries);
						case "index_import":
							return Promise.resolve(fixtures.indexSeries[0]);
						case "index_refresh": {
							// A fetched series takes the id it was fetched under, which
							// is what the provider row and the new card are keyed by.
							const id = (args as { id?: string } | null)?.id ?? "";
							const base = fixtures.indexSeries[0];
							return Promise.resolve({
								...base,
								id,
								label: id === base.id ? base.label : `Index ${id}`,
							});
						}
						case "index_write_prices": {
							// The write is per stored id; the answer names the commodity
							// and how many points were new (a second run reports zero).
							const id = (args as { id?: string } | null)?.id ?? "";
							const commodity = `IDX.${id.toUpperCase().replace(/[^A-Z0-9'._-]/g, "-")}`;
							return Promise.resolve({ ...fixtures.indexPriceWrite, series: id, commodity });
						}
						case "lifestyle_report":
							return Promise.resolve(fixtures.lifestyle);
						case "price_sources":
							return Promise.resolve(fixtures.priceSources);
						case "price_targets":
							return Promise.resolve(fixtures.priceTargets);
						case "fetch_prices":
							return Promise.resolve(fixtures.priceFetches);
						case "write_prices":
							return Promise.resolve(null);
						case "git_branches":
							return Promise.resolve([
								{ name: "main", is_current: true, is_remote: false, upstream: null },
							]);
						case "git_log":
							return Promise.resolve(fixtures.commits);
						case "git_commit_diff":
							return Promise.resolve(fixtures.commitDiff);
						case "storage_kind":
							return Promise.resolve("local");
						case "storage_error":
							// The folder's last failure; `None` is the healthy answer.
							return Promise.resolve(null);
						case "sync_changes":
							// Files the folder changed behind the app: none in the fixture.
							return Promise.resolve([]);
						case "sync_conflicts":
							return Promise.resolve({ candidates: [], orphans: [] });
						case "ai_settings":
							return Promise.resolve(fixtures.aiSettings);
						case "ai_models":
							// Nothing downloaded from an endpoint yet: the store only reads
							// this once a key exists, and no fixture has one.
							return Promise.resolve([]);
						case "conversations":
							// No conversation files in the fixture ledger.
							return Promise.resolve({ sessions: [], errors: [] });
						case "plugin:event|listen":
							// Tauri's event registry: the plugin hands back the id `unlisten`
							// takes. `transformCallback` above keeps the handler as-is, and no
							// fixture emits an event, so the listener never fires.
							listener += 1;
							return Promise.resolve(listener);
						case "plugin:event|unlisten":
							return Promise.resolve(null);
						case "history":
							return Promise.resolve([]);
						case "index_provider_list":
							return Promise.resolve([
								{ id: "us-cpi", label: "US CPI", source: "FRED", unofficial: false, currency: "USD", downloadable: true, note: "" },
							]);
						case "report_net_worth":
						case "report_cashflow":
						case "report_categories":
							return Promise.resolve([]);
						case "pick_ledger_folder":
							// The desktop's own chooser (`typedError`: reject models
							// `Err`). `?native=folder` models a runtime that draws one and
							// the user picking `Books`; every other run models a runtime
							// without a chooser, which the in-app picker falls back on.
							return window.location.search.includes("native=folder")
								? Promise.resolve("/ledger/Books")
								: Promise.reject("this runtime has no native folder chooser");
						default:
							// Not a fixture the stub knows. Answering permissively keeps
							// harmless reads from failing a run, but silently answering a
							// command nobody stubbed is how a dead command shipped: name it
							// loudly and let `e2e/gate.ts` fail the test.
							if (!unknown.includes(cmd)) {
								unknown.push(cmd);
								console.error(
									`[e2e/stub] unhandled Tauri command "${cmd}": add a case to e2e/stub.ts`
								);
							}
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
			Object.defineProperty(window, "__stubUnknown", { value: unknown, writable: false });
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
			budgetSettings,
			budgetSettingsConfigured,
			dashboards,
			indexSeries,
			indexPriceWrite,
			lifestyle,
			priceSources,
			priceTargets,
			priceFetches,
			aiSettings,
			commits,
			commitDiff,
		}
	);
}

/** The recorded calls, in order. */
export function invokes(page: Page): Promise<Invoke[]> {
	return page.evaluate(() => window.__invokes);
}

/**
 * The commands the stub answered by falling through to its permissive default,
 * i.e. the ones the app called that `e2e/stub.ts` has no fixture for. Empty is
 * the only good answer; `e2e/gate.ts` asserts it after every test.
 */
export function unknownCommands(page: Page): Promise<string[]> {
	return page.evaluate(() => window.__stubUnknown ?? []);
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
