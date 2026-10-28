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
							return Promise.resolve(fixtures.accounts);
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
							return path === "/ledger/Books"
								? Promise.resolve({ ...fixtures.summary, path: "/ledger/Books/main.bean" })
								: Promise.reject(`no root journal (main.bean, …) in ${path ?? "main.bean"}`);
						case "list_folders":
							return Promise.resolve(path === "/ledger/Books" ? fixtures.books : fixtures.folders);
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
		{ accounts, transactions, source, diff, summary, folders, books }
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
