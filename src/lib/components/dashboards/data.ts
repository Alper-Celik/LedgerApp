// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { ChartDatum, ChartSeries } from "$lib/charts/types.js";
import { isSubAccountOf } from "$lib/accounts.js";
import {
	definitionFor,
	isWidgetSpan,
	widgetDefinitions,
	type JsonValue,
	type WidgetConfig,
	type WidgetKind,
	type WidgetSpan,
} from "$lib/dashboard/index.js";
import { signOf, sumDecimals } from "$lib/money.js";
import type {
	AccountBalance,
	BudgetRange,
	Dashboard,
	DashboardWidget,
	PostingView,
	TransactionView,
} from "../../../bindings";

/**
 * One command's answer, kept apart from the others.
 *
 * A dashboard reads three sources, and a failure in one is not a failure of the
 * page: the widget that needed it says what went wrong while every other
 * widget still draws. Routes therefore carry these values instead of throwing.
 */
export type Loaded<T> = { ok: true; data: T } | { ok: false; error: string };

/** What the dashboard route fetches once and hands to every widget. */
export type DashboardData = {
	accounts: Loaded<AccountBalance[]>;
	transactions: Loaded<TransactionView[]>;
	budget: Loaded<BudgetRange>;
};

/** A saved widget with its config validated and its span made safe for the grid. */
export type PlacedWidget = {
	id: string;
	kind: string;
	config: WidgetConfig;
	span: WidgetSpan;
	order: number;
};

/** One row of the balances widget. */
export type BalanceRow = {
	account: string;
	balance: string;
	currency: string;
};

/**
 * One widget while the editor holds it: the saved fields a layout can change,
 * without `order` — the array's position is the order, assigned on save.
 */
export type DraftWidget = {
	id: string;
	kind: string;
	config: WidgetConfig;
	span: WidgetSpan;
};

/** Whether a saved kind is one this build's registry knows. */
export function isWidgetKind(value: string): value is WidgetKind {
	return widgetDefinitions.some((definition) => definition.kind === value);
}

/** A JSON value, checked rather than trusted: a config is read off disk. */
function isJsonValue(value: unknown): value is JsonValue {
	if (value === null) return true;
	const kind = typeof value;
	if (kind === "string" || kind === "number" || kind === "boolean") return true;
	if (Array.isArray(value)) return value.every(isJsonValue);
	if (kind === "object") return Object.values(value as Record<string, unknown>).every(isJsonValue);
	return false;
}

/**
 * A widget's saved config as a JSON object, with anything that is not JSON
 * dropped. The backend stores the object as given, so the shape has to be
 * checked here rather than assumed.
 */
export function asWidgetConfig(value: unknown): WidgetConfig {
	const config: WidgetConfig = {};
	if (typeof value !== "object" || value === null || Array.isArray(value)) return config;
	for (const [key, entry] of Object.entries(value)) {
		if (isJsonValue(entry)) config[key] = entry;
	}
	return config;
}

/**
 * A dashboard's widgets in reading order, each kind's config normalised through
 * the registry and each span coerced to one the grid understands.
 *
 * An unknown kind is preserved as it was saved rather than dropped: the renderer
 * can say the widget is unknown, and the editor can still let the user remove
 * or reorder it.
 */
export function prepareWidgets(dashboard: Dashboard): PlacedWidget[] {
	return [...dashboard.widgets]
		.sort((left, right) => left.order - right.order)
		.map((widget: DashboardWidget): PlacedWidget => {
			const definition = isWidgetKind(widget.kind) ? definitionFor(widget.kind) : null;
			const config = asWidgetConfig(widget.config);
			return {
				id: widget.id,
				kind: widget.kind,
				config: definition ? definition.normalise(config) : config,
				span: isWidgetSpan(widget.span) ? widget.span : (definition?.defaultSpan ?? 1),
				order: widget.order,
			};
		});
}

/** A trimmed string setting, or `null` for "not set". */
export function stringSetting(config: WidgetConfig, key: string): string | null {
	const value = config[key];
	return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

/** A finite number setting, from a number or a numeric string. */
export function numberSetting(config: WidgetConfig, key: string, fallback: number): number {
	const value = config[key];
	const parsed =
		typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN;
	return Number.isFinite(parsed) ? parsed : fallback;
}

/** A boolean setting. */
export function booleanSetting(config: WidgetConfig, key: string, fallback = false): boolean {
	const value = config[key];
	return typeof value === "boolean" ? value : fallback;
}

/** The string entries of a list setting, in order; anything else is dropped. */
export function stringListSetting(config: WidgetConfig, key: string): string[] {
	const value = config[key];
	return Array.isArray(value)
		? value.filter((entry): entry is string => typeof entry === "string")
		: [];
}

/** An integer within bounds; a config can carry a fractional number. */
function clamp(value: number, min: number, max: number): number {
	return Math.min(max, Math.max(min, Math.round(value)));
}

const MONTH = /^\d{4}-(0[1-9]|1[0-2])$/;

/** The `YYYY-MM` a date falls in, or `null` for a date the ledger cannot have written. */
function monthOf(date: string): string | null {
	const month = date.slice(0, 7);
	return MONTH.test(month) ? month : null;
}

/**
 * The newest `count` months any transaction touches, ascending.
 *
 * Only months the ledger actually has entries in become labels; a widget with a
 * gap in its range draws no bar for the missing month rather than inventing a
 * zero for a month the ledger was not read over.
 */
function windowMonths(transactions: readonly TransactionView[], count: number): string[] {
	const wanted = Math.max(1, Math.round(count));
	const months = new Set<string>();
	for (const transaction of transactions) {
		const month = monthOf(transaction.date);
		if (month) months.add(month);
	}
	return [...months].sort().slice(-wanted);
}

/**
 * Whether an expense account falls under one of the configured categories.
 *
 * A setting may name the full account (`Expenses:Food`) or just the branch the
 * budget screen shows (`Food`); both are matched against the account path.
 */
function withinCategories(account: string, categories: readonly string[]): boolean {
	return (
		categories.length === 0 ||
		categories.some((category) =>
			isSubAccountOf(account, isSubAccountOf(category, "Expenses") ? category : `Expenses:${category}`)
		)
	);
}

/**
 * The commodity a widget works in: the configured one when there is one, else
 * the commodity most of the candidate postings are written in.
 *
 * A sum of mixed commodities would not be money, so every series below filters
 * to this one currency before adding anything up.
 */
function pickCurrency(postings: readonly PostingView[], wanted: string | null): string | null {
	if (wanted !== null) {
		return postings.some((posting) => posting.currency === wanted) ? wanted : null;
	}
	const counts = new Map<string, number>();
	for (const posting of postings) {
		counts.set(posting.currency, (counts.get(posting.currency) ?? 0) + 1);
	}
	let best: string | null = null;
	let bestCount = 0;
	for (const [currency, count] of counts) {
		if (count > bestCount) {
			best = currency;
			bestCount = count;
		}
	}
	return best;
}

/** The negation of an exact decimal string, without a `-0`. */
function negate(units: string): string {
	const text = units.trim();
	if (signOf(text) === 0) return "0";
	return text.startsWith("-") ? text.slice(1) : `-${text}`;
}

/**
 * Money in against money out, month by month.
 *
 * With an account configured, the widget reads that account's own legs: what
 * arrived is positive, what left is negative. Without one it reads the ledger's
 * Income and Expenses accounts, which is the whole household's cash flow.
 */
export function cashFlowSeries(
	transactions: readonly TransactionView[],
	config: WidgetConfig
): ChartSeries[] {
	const account = stringSetting(config, "account");
	const wantedCurrency = stringSetting(config, "currency");
	const months = windowMonths(transactions, numberSetting(config, "months", 12));
	if (months.length === 0) return [];

	const window = new Set(months);
	const byMonth = new Map<string, PostingView[]>(months.map((month) => [month, []]));
	for (const transaction of transactions) {
		const month = monthOf(transaction.date);
		if (!month || !window.has(month)) continue;
		const target = byMonth.get(month);
		if (!target) continue;
		for (const posting of transaction.postings) {
			const candidate = account
				? isSubAccountOf(posting.account, account)
				: isSubAccountOf(posting.account, "Income") || isSubAccountOf(posting.account, "Expenses");
			if (candidate) target.push(posting);
		}
	}

	const candidates = [...byMonth.values()].flat();
	const currency = pickCurrency(candidates, wantedCurrency);
	if (currency === null) return [];

	const incoming: ChartDatum[] = [];
	const outgoing: ChartDatum[] = [];
	for (const month of months) {
		const postings = (byMonth.get(month) ?? []).filter(
			(posting) => posting.currency === currency
		);
		const inUnits = account
			? postings.filter((posting) => signOf(posting.units) > 0).map((posting) => posting.units)
			: postings
					.filter((posting) => isSubAccountOf(posting.account, "Income"))
					.map((posting) => negate(posting.units));
		const outUnits = account
			? postings.filter((posting) => signOf(posting.units) < 0).map((posting) => posting.units)
			: postings
					.filter((posting) => isSubAccountOf(posting.account, "Expenses"))
					.map((posting) => negate(posting.units));
		incoming.push({ label: month, value: sumDecimals(inUnits), currency });
		outgoing.push({ label: month, value: sumDecimals(outUnits), currency });
	}

	return [
		{ name: account ? "In" : "Income", points: incoming },
		{ name: account ? "Out" : "Expenses", points: outgoing },
	];
}

/**
 * What was spent each month, as a positive figure.
 *
 * Every month in the window gets a point, zero included: a month without
 * spending is a fact about the ledger, not a gap in the chart. Refunds stay
 * negative and pull their month down — the exact sum is the ledger's, not a
 * rounded reading of it.
 */
export function spendingSeries(
	transactions: readonly TransactionView[],
	config: WidgetConfig
): ChartSeries[] {
	const months = windowMonths(transactions, numberSetting(config, "months", 12));
	if (months.length === 0) return [];

	const categories = stringListSetting(config, "categories");
	const window = new Set(months);
	const byMonth = new Map<string, PostingView[]>(months.map((month) => [month, []]));
	for (const transaction of transactions) {
		const month = monthOf(transaction.date);
		if (!month || !window.has(month)) continue;
		const target = byMonth.get(month);
		if (!target) continue;
		for (const posting of transaction.postings) {
			if (isSubAccountOf(posting.account, "Expenses") && withinCategories(posting.account, categories)) {
				target.push(posting);
			}
		}
	}

	const currency = pickCurrency([...byMonth.values()].flat(), stringSetting(config, "currency"));
	if (currency === null) return [];

	const points = months.map((month) => ({
		label: month,
		value: sumDecimals(
			(byMonth.get(month) ?? [])
				.filter((posting) => posting.currency === currency)
				.map((posting) => posting.units)
		),
		currency,
	}));
	return [{ name: "Spent", points }];
}

/** An exact comparison of two decimal strings, by magnitude — no float involved. */
export function compareMagnitude(left: string, right: string): number {
	const a = magnitudeParts(left);
	const b = magnitudeParts(right);
	if (a.whole.length !== b.whole.length) return a.whole.length - b.whole.length;
	if (a.whole !== b.whole) return a.whole < b.whole ? -1 : 1;
	const width = Math.max(a.fraction.length, b.fraction.length);
	const fa = a.fraction.padEnd(width, "0");
	const fb = b.fraction.padEnd(width, "0");
	if (fa === fb) return 0;
	return fa < fb ? -1 : 1;
}

/** The digits of an amount, sign and leading zeroes set aside. */
function magnitudeParts(units: string): { whole: string; fraction: string } {
	const text = units.trim().replace(/^[+-]/, "");
	const [whole = "0", fraction = ""] = text.split(".");
	return { whole: whole.replace(/^0+(?=\d)/, ""), fraction };
}

/**
 * Where the spending went over the newest few months, one slice per top-level
 * expense category, biggest first.
 *
 * Only positive sums become slices: a category whose refunds outweigh its
 * spending has no share of a total to show. The limit is applied after sorting,
 * so it keeps the categories that matter rather than the first ones saved.
 */
export function categoryShare(
	transactions: readonly TransactionView[],
	config: WidgetConfig
): ChartDatum[] {
	const months = windowMonths(transactions, numberSetting(config, "months", 3));
	if (months.length === 0) return [];

	const categories = stringListSetting(config, "categories");
	const window = new Set(months);
	const byCategory = new Map<string, PostingView[]>();
	for (const transaction of transactions) {
		const month = monthOf(transaction.date);
		if (!month || !window.has(month)) continue;
		for (const posting of transaction.postings) {
			if (!isSubAccountOf(posting.account, "Expenses") || !withinCategories(posting.account, categories)) continue;
			const parts = posting.account.split(":");
			const label = parts[1] ?? "Uncategorised";
			const target = byCategory.get(label);
			if (target) target.push(posting);
			else byCategory.set(label, [posting]);
		}
	}

	const currency = pickCurrency(
		[...byCategory.values()].flat(),
		stringSetting(config, "currency")
	);
	if (currency === null) return [];

	const limit = clamp(numberSetting(config, "limit", 8), 1, 12);
	return [...byCategory.entries()]
		.map(([label, postings]) => ({
			label,
			value: sumDecimals(
				postings
					.filter((posting) => posting.currency === currency)
					.map((posting) => posting.units)
			),
			currency,
		}))
		.filter((point) => signOf(point.value) > 0)
		.sort((left, right) => compareMagnitude(right.value, left.value))
		.slice(0, limit);
}

/**
 * Each budgeted month's funding and spending, for the widget's bar chart.
 *
 * `budgetRange(null, null)` answers the newest budgeted months; the config's
 * `months` only narrows that window, it cannot ask for months the range did not
 * return. Both series are the backend's exact totals, not a frontend sum.
 */
export function budgetSeries(range: BudgetRange, config: WidgetConfig): ChartSeries[] {
	const wanted = Math.max(1, Math.round(numberSetting(config, "months", 6)));
	const months = range.months.slice(-wanted);
	return [
		{
			name: "Budgeted",
			points: months.map((month) => ({
				label: month.month,
				value: month.budgeted,
				currency: range.currency,
			})),
		},
		{
			name: "Spent",
			points: months.map((month) => ({
				label: month.month,
				value: month.spent,
				currency: range.currency,
			})),
		},
	];
}

/**
 * The balances table's rows: accounts truncated to the configured depth, each
 * carrying the backend's own subtree balance for that node.
 *
 * The backend reports every ancestor's whole-subtree balance, so truncating an
 * account to the depth and reading that node's figure counts each unit once. A
 * sum here would double-count a parent and its children; this does none.
 */
export function balanceRows(
	accounts: readonly AccountBalance[],
	config: WidgetConfig
): BalanceRow[] {
	const depth = clamp(numberSetting(config, "depth", 2), 1, 6);
	const showZero = booleanSetting(config, "showZero", false);
	const wanted = stringListSetting(config, "accounts");
	const reported = new Map(accounts.map((account) => [account.name, account] as const));
	const seen = new Set<string>();
	const rows: BalanceRow[] = [];

	for (const account of accounts) {
		const parts = account.name.split(":");
		const key = parts.slice(0, Math.min(depth, parts.length)).join(":");
		if (seen.has(key)) continue;
		seen.add(key);
		if (wanted.length > 0 && !wanted.some((prefix) => isSubAccountOf(key, prefix))) continue;
		const source = reported.get(key);
		if (!source) continue;
		if (!showZero && signOf(source.balance) === 0) continue;
		rows.push({ account: key, balance: source.balance, currency: source.currency });
	}

	return rows.sort((left, right) => left.account.localeCompare(right.account));
}
