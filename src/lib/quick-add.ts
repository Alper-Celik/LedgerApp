// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { AccountBalance, TransactionView } from "../bindings";

/**
 * The quick-add sheet's two pieces of thinking, kept out of the component so
 * both can be read on their own: the keypad's decimal accumulator, and the
 * guesses the sheet opens with.
 *
 * Money never becomes a `number` here. The keypad collects the characters the
 * user pressed and the sign is glued on at the end, so the figure reaches the
 * ledger exactly as typed — no float round trip, no locale formatting.
 */

/** Which way the money went: out of the wallet, or into it. */
export type QuickAddSign = "expense" | "income";

/** Decimals the keypad accepts; a cash amount is the case this sheet is for. */
export const MAX_DECIMALS = 2;

/** How far back "recent" reaches for the category and free-text counts. */
export const RECENT_DAYS = 90;

/** How many entries each suggestion list carries. */
export const MAX_SUGGESTIONS = 8;

/** Accounts money comes *from*: the leg the backend interpolates. */
const FUNDING_ROOTS: ReadonlySet<string> = new Set(["Assets", "Liabilities"]);

/** Accounts a payment is *for*: the leg that carries the figure. */
const CATEGORY_ROOTS: ReadonlySet<string> = new Set(["Expenses", "Income"]);

/** What the keypad guessed for this sheet: the four presets and two lists. */
export type QuickAddSuggestions = {
	/** The funding account to preselect: the last pair's, else a cash account. */
	from: string;
	/** The category to preselect: the last pair's, else the most used recently. */
	to: string;
	/**
	 * Free text to offer under the entry's one field, most used first. Payees
	 * and narrations are the same proposal there — one tap fills the field — so
	 * both are listed, deduplicated.
	 */
	payees: string[];
	/** Category accounts, most used in the window first, then the rest by use. */
	categories: string[];
};

/** One keypad press: a digit, the decimal point, or a backspace. */
export type QuickAddKey = string;

/**
 * The keypad after one press. Digits accumulate; a second decimal point is
 * ignored, and so is a decimal beyond `MAX_DECIMALS`. A lone leading zero is a
 * placeholder, so the next digit replaces it rather than extending it.
 */
export function appendKey(current: string, key: QuickAddKey): string {
	if (key === "backspace") return current.slice(0, -1);
	if (key === ".") {
		if (current.includes(".")) return current;
		return current === "" ? "0." : `${current}.`;
	}
	if (key.length !== 1 || key < "0" || key > "9") return current;
	const point = current.indexOf(".");
	if (point >= 0 && current.length - point - 1 >= MAX_DECIMALS) return current;
	if (point < 0 && current === "0") return key;
	return current + key;
}

/**
 * The amount to write, as the ledger wants it: a signed decimal string, or `""`
 * when the keypad holds nothing worth posting (`0`, `0.00`, or empty).
 *
 * The sign is the whole meaning of the expense/income toggle. An expense is
 * written positive on the category leg — `Expenses:Food 12.50` — and beancount
 * interpolates the negative on the funding leg; income is written negative on
 * its leg (`Income:Salary -12.50`), which interpolates the deposit.
 */
export function amountOf(digits: string, sign: QuickAddSign): string {
	// A figure with no non-zero digit is not an amount: `0`, `0.`, `0.00`.
	if (!/[1-9]/.test(digits)) return "";
	const trimmed = digits.endsWith(".") ? digits.slice(0, -1) : digits;
	return sign === "income" ? `-${trimmed}` : trimmed;
}

/** A date part as two digits. */
const pad = (value: number) => String(value).padStart(2, "0");

/** Today as the register spells a date: local, not UTC, `YYYY-MM-DD`. */
export function todayIso(): string {
	const now = new Date();
	return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/**
 * The presets and lists the sheet opens with, derived from the transactions the
 * register already has.
 *
 * One pass over `transactions`, no sorting of the ledger itself, so a book with
 * tens of thousands of entries costs the same kind of walk as the register:
 *
 * - **The pair**: the transaction with the greatest date — ties broken by
 *   `TransactionView.id`, the ledger stream position, so the answer never
 *   depends on the array's order — that has both a funding leg (an
 *   `Assets`/`Liabilities` posting) and a category leg (`Expenses`/`Income`).
 *   Its two accounts are `from` and `to`, which is how "last used" is read.
 * - **Categories**: how often each category account appears, counted once per
 *   transaction, inside the last `RECENT_DAYS` days. Ties go to the earlier
 *   name in plain code-unit order, so no locale changes the answer; the counts
 *   over the whole book fill in behind them when the window is empty.
 * - **Free text**: the payee, or the narration when there is no payee, counted
 *   inside the same window; ties go to whichever was seen most recently, then
 *   to the text itself.
 * - **Funding fallback**: `Assets:Cash` when the ledger has it, else the first
 *   `Assets` account, else the first funding-root account at all.
 */
export function suggestions(
	transactions: TransactionView[],
	accounts: AccountBalance[],
	today: string = todayIso()
): QuickAddSuggestions {
	const cutoff = dayBefore(today, RECENT_DAYS);

	let pairFrom = "";
	let pairTo = "";
	let pairDay = "";
	let pairId = -1;

	/** Category use inside the window, and over the whole book as the fallback. */
	const recentCategories = new Map<string, number>();
	const allCategories = new Map<string, number>();
	/** Free text inside the window, with the newest day it was used. */
	const recentText = new Map<string, { count: number; day: string }>();

	for (const transaction of transactions) {
		let funding = "";
		let category = "";
		const touched = new Set<string>();
		for (const posting of transaction.postings) {
			const root = rootOf(posting.account);
			if (funding === "" && FUNDING_ROOTS.has(root)) funding = posting.account;
			if (CATEGORY_ROOTS.has(root)) {
				// A split names several categories; the count is once per entry,
				// and the first one is what the pair is read as.
				touched.add(posting.account);
				if (category === "") category = posting.account;
			}
		}

		if (funding !== "" && category !== "") {
			const newer =
				transaction.date > pairDay ||
				(transaction.date === pairDay && transaction.id > pairId);
			if (newer) {
				pairFrom = funding;
				pairTo = category;
				pairDay = transaction.date;
				pairId = transaction.id;
			}
		}

		for (const account of touched) {
			allCategories.set(account, (allCategories.get(account) ?? 0) + 1);
		}

		if (transaction.date < cutoff) continue;

		for (const account of touched) {
			recentCategories.set(account, (recentCategories.get(account) ?? 0) + 1);
		}

		const text = (transaction.payee ?? transaction.narration).trim();
		if (text === "") continue;
		const seen = recentText.get(text);
		if (seen === undefined) {
			recentText.set(text, { count: 1, day: transaction.date });
		} else {
			seen.count += 1;
			if (transaction.date > seen.day) seen.day = transaction.date;
		}
	}

	const categories = rankedCategories(recentCategories, allCategories);
	const payees = [...recentText.entries()]
		.sort(
			([leftText, left], [rightText, right]) =>
				right.count - left.count ||
				(left.day < right.day ? 1 : left.day > right.day ? -1 : 0) ||
				compare(leftText, rightText)
		)
		.slice(0, MAX_SUGGESTIONS)
		.map(([text]) => text);

	const known = new Set(accounts.map((account) => account.name));
	const rememberedFrom = known.has(pairFrom) ? pairFrom : "";
	const rememberedTo = known.has(pairTo) ? pairTo : "";

	return {
		from: rememberedFrom !== "" ? rememberedFrom : firstFundingAccount(accounts),
		to: rememberedTo !== "" ? rememberedTo : (categories[0] ?? ""),
		payees,
		categories,
	};
}

/** The first segment of a Beancount account: `Assets` in `Assets:Bank:Checking`. */
function rootOf(account: string): string {
	const end = account.indexOf(":");
	return end === -1 ? account : account.slice(0, end);
}

/** Plain code-unit order: stable everywhere, unlike a locale collation. */
function compare(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

/** Categories by recent use, then by their use over the whole book. */
function rankedCategories(
	recent: Map<string, number>,
	all: Map<string, number>
): string[] {
	const byCount = (counts: Map<string, number>) =>
		[...counts.entries()].sort(
			([leftName, leftCount], [rightName, rightCount]) =>
				rightCount - leftCount || compare(leftName, rightName)
		);
	const ranked = byCount(recent).map(([account]) => account);
	const seen = new Set(ranked);
	for (const [account] of byCount(all)) {
		if (!seen.has(account)) ranked.push(account);
	}
	return ranked.slice(0, MAX_SUGGESTIONS);
}

/** `Assets:Cash`, else the first `Assets` account, else the first funding one. */
function firstFundingAccount(accounts: AccountBalance[]): string {
	const named = accounts.find((account) => account.name === "Assets:Cash");
	if (named) return named.name;
	const asset = accounts.find((account) => rootOf(account.name) === "Assets");
	if (asset) return asset.name;
	const funding = accounts.find((account) => FUNDING_ROOTS.has(rootOf(account.name)));
	return funding?.name ?? "";
}

/** `day` moved back by `days`, as `YYYY-MM-DD`; UTC arithmetic, no DST steps. */
function dayBefore(day: string, days: number): string {
	const [year, month, date] = day.split("-").map(Number);
	if (!Number.isFinite(year) || !Number.isFinite(month) || !Number.isFinite(date)) {
		return day;
	}
	const moved = new Date(Date.UTC(year, month - 1, date) - days * 86_400_000);
	return `${moved.getUTCFullYear()}-${pad(moved.getUTCMonth() + 1)}-${pad(moved.getUTCDate())}`;
}
