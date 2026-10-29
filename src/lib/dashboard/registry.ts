// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type AreaChart from "$lib/charts/area-chart.svelte";
import type BarChart from "$lib/charts/bar-chart.svelte";
import type DonutChart from "$lib/charts/donut-chart.svelte";
import type LineChart from "$lib/charts/line-chart.svelte";
import type { JsonValue, WidgetConfig, WidgetKind, WidgetSpan } from "./types.js";

/** The chart wrappers a widget can draw, by the name this registry uses. */
export type ChartName = "line" | "bar" | "area" | "donut";

/** Any of the chart wrappers, as the component a loader hands back. */
export type ChartComponent =
	| typeof LineChart
	| typeof BarChart
	| typeof AreaChart
	| typeof DonutChart;

/** What loading a chart wrapper answers: its module, of which the widget uses `default`. */
export type ChartModule = { default: ChartComponent };

/**
 * The chart wrappers by name, loaded only when something draws them.
 *
 * A widget is a chart wrapper plus a config, so this table is all the wiring a
 * widget needs: the dashboard decides the data, the wrapper draws it, and this
 * registry only says which wrapper and with what defaults.
 */
export const chartLoaders: Record<ChartName, () => Promise<ChartModule>> = {
	line: () => import("$lib/charts/line-chart.svelte"),
	bar: () => import("$lib/charts/bar-chart.svelte"),
	area: () => import("$lib/charts/area-chart.svelte"),
	donut: () => import("$lib/charts/donut-chart.svelte"),
};

/** One kind of widget: what it is called, what draws it, and what its config means. */
export type WidgetDefinition = {
	kind: WidgetKind;
	/** Card heading. */
	title: string;
	/** One line under the title, and the blurb in a widget picker. */
	description: string;
	/** The chart wrappers its body draws, in order. Empty for a kind that is a table. */
	charts: readonly ChartName[];
	/** Columns it asks for by default in the four-column grid. */
	defaultSpan: WidgetSpan;
	/**
	 * The saved config with this kind's defaults filled in and unknown keys
	 * dropped. A saved dashboard can be older than the code that reads it, and a
	 * config is JSON read off disk, so nothing here trusts its input.
	 */
	normalise: (config: WidgetConfig) => WidgetConfig;
};

/** An integer within bounds, from a number or a numeric string; anything else falls back. */
function integerIn(
	value: JsonValue | undefined,
	fallback: number,
	min: number,
	max: number
): number {
	const parsed =
		typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN;
	if (!Number.isFinite(parsed)) return fallback;
	return Math.min(max, Math.max(min, Math.round(parsed)));
}

/** A trimmed string, or `null` for "no filter" — an empty setting means everything. */
function textOrNull(value: JsonValue | undefined): string | null {
	return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

/** The strings of an array, in order; anything else in it is dropped. */
function stringsOf(value: JsonValue | undefined): string[] {
	if (!Array.isArray(value)) return [];
	return value.filter((item): item is string => typeof item === "string");
}

/**
 * The widest month range a widget may ask for. A dashboard is read at a glance;
 * a range longer than a decade stops being one.
 */
const MAX_MONTHS = 120;

const definitions = {
	"net-worth": {
		kind: "net-worth",
		title: "Net worth",
		description: "What the accounts are worth over time, month by month.",
		charts: ["area"] as const,
		defaultSpan: 2,
		normalise: (config: WidgetConfig): WidgetConfig => ({
			account: textOrNull(config.account),
			currency: textOrNull(config.currency),
			months: integerIn(config.months, 12, 1, MAX_MONTHS),
			adjustForInflation:
				typeof config.adjustForInflation === "boolean" ? config.adjustForInflation : false,
		}),
	},
	"cash-flow": {
		kind: "cash-flow",
		title: "Cash flow",
		description: "Money in against money out, month by month.",
		charts: ["bar"] as const,
		defaultSpan: 2,
		normalise: (config: WidgetConfig): WidgetConfig => ({
			account: textOrNull(config.account),
			currency: textOrNull(config.currency),
			months: integerIn(config.months, 12, 1, MAX_MONTHS),
		}),
	},
	"spending-by-month": {
		kind: "spending-by-month",
		title: "Spending by month",
		description: "Total spending each month, and which months were the heavy ones.",
		charts: ["bar"] as const,
		defaultSpan: 2,
		normalise: (config: WidgetConfig): WidgetConfig => ({
			currency: textOrNull(config.currency),
			months: integerIn(config.months, 12, 1, MAX_MONTHS),
			categories: stringsOf(config.categories),
		}),
	},
	"budget-vs-actual": {
		kind: "budget-vs-actual",
		title: "Budget vs actual",
		description: "Each month's spending beside what was budgeted for it.",
		charts: ["bar"] as const,
		defaultSpan: 2,
		normalise: (config: WidgetConfig): WidgetConfig => ({
			budget: textOrNull(config.budget),
			currency: textOrNull(config.currency),
			months: integerIn(config.months, 6, 1, MAX_MONTHS),
		}),
	},
	"category-share": {
		kind: "category-share",
		title: "Category share",
		description: "Where the spending went, as a share of the whole.",
		charts: ["donut"] as const,
		defaultSpan: 1,
		normalise: (config: WidgetConfig): WidgetConfig => ({
			currency: textOrNull(config.currency),
			months: integerIn(config.months, 3, 1, MAX_MONTHS),
			// A donut with more than a dozen slices is a pie nobody can read.
			limit: integerIn(config.limit, 8, 1, 12),
			categories: stringsOf(config.categories),
		}),
	},
	"lifestyle-inflation": {
		kind: "lifestyle-inflation",
		title: "Lifestyle inflation",
		description: "Your spending, and the same spending restated in older money.",
		charts: ["line"] as const,
		defaultSpan: 2,
		normalise: (config: WidgetConfig): WidgetConfig => ({
			currency: textOrNull(config.currency),
			months: integerIn(config.months, 24, 1, MAX_MONTHS),
			// A month such as `2025-01`; null means the first month in the range.
			baseline: textOrNull(config.baseline),
			categories: stringsOf(config.categories),
		}),
	},
	"balances-table": {
		kind: "balances-table",
		title: "Balances",
		description: "Every account's balance as a table — exact amounts, no chart.",
		charts: [] as const,
		defaultSpan: 4,
		normalise: (config: WidgetConfig): WidgetConfig => ({
			accounts: stringsOf(config.accounts),
			depth: integerIn(config.depth, 2, 1, 6),
			showZero: typeof config.showZero === "boolean" ? config.showZero : false,
		}),
	},
} as const satisfies Record<WidgetKind, WidgetDefinition>;

/** Every widget kind, in the order a picker lists them. */
export const widgetDefinitions: readonly WidgetDefinition[] = Object.values(definitions);

/** The definition of one kind — total, because `definitions` covers every `WidgetKind`. */
export function definitionFor(kind: WidgetKind): WidgetDefinition {
	return definitions[kind];
}
