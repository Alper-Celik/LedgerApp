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
	CashflowMonth,
	CategoryMonths,
	Dashboard,
	DashboardWidget,
	NetWorthMonth,
} from "../../../bindings";

/**
 * One command's answer, kept apart from the others.
 *
 * A dashboard reads several sources, and a failure in one is not a failure of
 * the page: the widget that needed it says what went wrong while every other
 * widget still draws. Routes therefore carry these values instead of throwing.
 */
export type Loaded<T> = { ok: true; data: T } | { ok: false; error: string };

/**
 * What the dashboard route fetches once and hands to every widget.
 *
 * Each source is one command, read once for the widest window any widget on
 * the page asks for ([`reportWindow`]); a widget slices its own window off the
 * end rather than asking again:
 *
 * - `netWorth` — `reportNetWorth`, read by the **net-worth** widget.
 * - `cashflow` — `reportCashflow`, read by the **cash-flow** widget.
 * - `categories` — `reportCategories`, read by **spending-by-month** and
 *   **category-share**.
 * - `accounts` — `accounts`, read by the **balances-table** widget.
 * - `budget` — `budgetRange`, read by **budget-vs-actual**.
 *
 * Every figure inside these sources is the backend's exact decimal string; no
 * widget sums a ledger it fetched itself.
 */
export type DashboardData = {
	accounts: Loaded<AccountBalance[]>;
	netWorth: Loaded<NetWorthMonth[]>;
	cashflow: Loaded<CashflowMonth[]>;
	categories: Loaded<CategoryMonths[]>;
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

/** Whether a widget kind reads one of the backend's month reports. */
const READS_REPORT: Record<WidgetKind, boolean> = {
	"net-worth": true,
	"cash-flow": true,
	"spending-by-month": true,
	"category-share": true,
	"budget-vs-actual": false,
	"balances-table": false,
	"lifestyle-inflation": false,
};

/**
 * The widest month window this dashboard's widgets ask a report for.
 *
 * The route reads each report once for the page, so the window has to cover
 * every widget on it; a widget then takes its own, shorter window off the end
 * of what arrived. Zero means no widget on the page reads a report, and the
 * route asks for none at all.
 */
export function reportWindow(widgets: readonly PlacedWidget[]): number {
	let months = 0;
	for (const widget of widgets) {
		if (!isWidgetKind(widget.kind) || !READS_REPORT[widget.kind]) continue;
		months = Math.max(months, Math.round(numberSetting(widget.config, "months", 12)));
	}
	return months;
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
 * The report rows a widget works with: the newest `months` of them, and only
 * those in the configured commodity when one is set.
 *
 * The page reads each report once over the widest window any widget asks for,
 * so a widget narrows its own window here instead of asking again. The report
 * already uses one commodity throughout; a configured one filters it to that
 * commodity (and to nothing when the report is written in another).
 */
function reportedWindow<T extends { month: string; currency: string }>(
	rows: readonly T[],
	config: WidgetConfig
): T[] {
	const wanted = Math.max(1, Math.round(numberSetting(config, "months", 12)));
	const currency = stringSetting(config, "currency");
	return rows.filter((row) => currency === null || row.currency === currency).slice(-wanted);
}

/**
 * The net-worth widget's one series: the ledger's month-end net worth.
 *
 * Read from `reportNetWorth` — the dashboard's `netWorth` source — so the
 * figure is the backend's own valuation of assets plus liabilities for each
 * month, an exact decimal string per point.
 */
export function netWorthSeries(
	months: readonly NetWorthMonth[],
	config: WidgetConfig
): ChartSeries[] {
	const rows = reportedWindow(months, config);
	if (rows.length === 0) return [];
	return [
		{
			name: "Net worth",
			points: rows.map((row) => ({ label: row.month, value: row.value, currency: row.currency })),
		},
	];
}

/**
 * Money in against money out, month by month, as the backend's cash-flow
 * report draws it (`reportCashflow` — the dashboard's `cashflow` source).
 *
 * Both series are the report's own positive decimal strings: `inflow` is
 * income signed to read positive, `outflow` is expenses. The registry's
 * `account` setting is not applied — the report is the ledger's Income/Expenses
 * split, and narrowing it to one account is not part of that command.
 */
export function cashFlowSeries(
	months: readonly CashflowMonth[],
	config: WidgetConfig
): ChartSeries[] {
	const rows = reportedWindow(months, config);
	if (rows.length === 0) return [];
	return [
		{
			name: "Income",
			points: rows.map((row) => ({ label: row.month, value: row.inflow, currency: row.currency })),
		},
		{
			name: "Expenses",
			points: rows.map((row) => ({ label: row.month, value: row.outflow, currency: row.currency })),
		},
	];
}

/**
 * What was spent each month, as a positive figure: the backend's per-branch
 * spending added up across the branches the widget includes
 * (`reportCategories` — the dashboard's `categories` source).
 *
 * The report carries every month of its range for every branch, zero
 * included, so a month without spending stays a zero point rather than a gap.
 * The sum is exact: `sumDecimals` adds the strings without a float, and a
 * branch's refunds keep their sign and pull their month down.
 */
export function spendingSeries(
	branches: readonly CategoryMonths[],
	config: WidgetConfig
): ChartSeries[] {
	const wanted = Math.max(1, Math.round(numberSetting(config, "months", 12)));
	const wantedCurrency = stringSetting(config, "currency");
	const categories = stringListSetting(config, "categories");
	const included = branches.filter(
		(branch) =>
			(wantedCurrency === null || branch.currency === wantedCurrency) &&
			withinCategories(branch.account, categories)
	);
	const months = (included[0]?.months ?? []).slice(-wanted).map((month) => month.month);
	if (included.length === 0 || months.length === 0) return [];

	const byMonth = new Map<string, string[]>(months.map((month) => [month, []]));
	for (const branch of included) {
		for (const month of branch.months) {
			const target = byMonth.get(month.month);
			if (target) target.push(month.spent);
		}
	}

	return [
		{
			name: "Spent",
			points: months.map((month) => ({
				label: month,
				value: sumDecimals(byMonth.get(month) ?? []),
				currency: included[0]?.currency ?? "",
			})),
		},
	];
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
 * Where the spending went over the newest few months, one slice per reported
 * branch, biggest first.
 *
 * Read from the same `reportCategories` source as [`spendingSeries`]: each
 * branch's `spent` totals over the widget's own window, added as exact decimal
 * strings. Only positive sums become slices — a branch whose refunds outweigh
 * its spending has no share of a total to show — and the limit is applied
 * after sorting, so it keeps the branches that matter.
 */
export function categoryShare(
	branches: readonly CategoryMonths[],
	config: WidgetConfig
): ChartDatum[] {
	const wanted = Math.max(1, Math.round(numberSetting(config, "months", 3)));
	const wantedCurrency = stringSetting(config, "currency");
	const categories = stringListSetting(config, "categories");
	const limit = clamp(numberSetting(config, "limit", 8), 1, 12);
	return branches
		.filter(
			(branch) =>
				(wantedCurrency === null || branch.currency === wantedCurrency) &&
				withinCategories(branch.account, categories)
		)
		.map((branch) => ({
			label: branch.label,
			value: sumDecimals(branch.months.slice(-wanted).map((month) => month.spent)),
			currency: branch.currency,
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
