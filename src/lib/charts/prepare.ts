// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { sumDecimals } from "$lib/money.js";
import type { ChartConfig } from "$lib/components/ui/chart/index.js";
import { categoryLabels, describeSeries, moneyText, plotNumber } from "./format.js";
import type { ChartDatum, ChartSeries } from "./types.js";

/** The theme's chart colors, cycled when a series brings none of its own. */
const SERIES_COLORS = [
	"var(--chart-1)",
	"var(--chart-2)",
	"var(--chart-3)",
	"var(--chart-4)",
	"var(--chart-5)",
] as const;

/** One row of the tooltip: a series name, its swatch color, and the amount as written. */
export type TooltipRow = { name: string; color: string; units: string; currency: string };

/**
 * One row of the table layerchart draws from: the category label, then one
 * numeric column per series with its exact decimal string beside it.
 */
export type ChartRow = { label: string; [column: string]: string | number | undefined };

/** A layerchart series definition; `color` is a real color, so it survives portaling. */
export type ChartSeriesDef = { key: string; label: string; color: string };

/** What a tooltip or summary needs to know about a series besides its points. */
export type PreparedSeries = {
	/** Column holding the plotted number. */
	key: string;
	/** Column holding the exact decimal string; read instead of the plotted number. */
	textColumn: string;
	name: string;
	currency: string;
	color: string;
};

/** Everything a cartesian wrapper hands layerchart, prepared once per data change. */
export type PreparedChart = {
	rows: ChartRow[];
	series: ChartSeriesDef[];
	prepared: PreparedSeries[];
	config: ChartConfig;
	/** The chart in words, for the container's `aria-label`. */
	summary: string;
};

/**
 * Turns money series into the rows, series definitions and container config a
 * layerchart cartesian chart takes.
 *
 * The split between `key` (a number, for pixels) and `textColumn` (the exact
 * decimal string, for reading) is the whole point: the chart never formats an
 * amount, and nothing downstream ever sees a rounded one.
 */
export function prepareChart(series: readonly ChartSeries[], kind: string): PreparedChart {
	const labels = categoryLabels(series);
	const prepared: PreparedSeries[] = series.map((one, index) => ({
		key: `s${index}`,
		textColumn: `t${index}`,
		name: one.name,
		currency: one.points[0]?.currency ?? "",
		color: one.color ?? SERIES_COLORS[index % SERIES_COLORS.length],
	}));
	const pointsByLabel = series.map(
		(one) => new Map(one.points.map((point) => [point.label, point] as const))
	);

	const rows = labels.map((label) => {
		const row: ChartRow = { label };
		prepared.forEach((one, index) => {
			const point = pointsByLabel[index]?.get(label);
			if (!point) return;
			row[one.key] = plotNumber(point.value);
			row[one.textColumn] = point.value;
		});
		return row;
	});

	const config: ChartConfig = {};
	const defs: ChartSeriesDef[] = prepared.map((one) => {
		config[one.key] = { label: one.name, color: one.color };
		return { key: one.key, label: one.name, color: one.color };
	});

	return { rows, series: defs, prepared, config, summary: describeSeries(kind, series) };
}

/** One slice of a share chart: the amount it carries, as a number and as written. */
export type ShareRow = {
	label: string;
	/** Plotted size; the only float in the slice. */
	value: number;
	color: string;
	/** Exact decimal string, shown in the tooltip. */
	text: string;
	currency: string;
};

/** A donut's slices, config and summary, plus the total its center shows. */
export type PreparedShare = {
	rows: ShareRow[];
	config: ChartConfig;
	summary: string;
	/** Exact sum of the slices, for the center of the donut. */
	total: ChartDatum;
};

/**
 * Turns category amounts into donut slices.
 *
 * A share of a total is only meaningful for amounts in one currency and of one
 * sign, so a call is expected to pass spending in a single currency; zero and
 * negative amounts are left out rather than drawn as if they were positive.
 * The percentages in the summary are ratios, computed from the plotted numbers
 * — the amounts beside them stay exact decimal strings.
 */
export function prepareShare(data: readonly ChartDatum[]): PreparedShare {
	const drawn = data.filter((point) => (plotNumber(point.value) ?? 0) > 0);
	const currency = drawn[0]?.currency ?? "";
	const totalUnits = sumDecimals(drawn.map((point) => point.value));
	const total = plotNumber(totalUnits) ?? 0;

	const config: ChartConfig = {};
	const rows: ShareRow[] = drawn.map((point, index) => {
		const color = SERIES_COLORS[index % SERIES_COLORS.length];
		config[`c${index}`] = { label: point.label, color };
		return {
			label: point.label,
			value: plotNumber(point.value) ?? 0,
			color,
			text: point.value,
			currency: point.currency,
		};
	});

	const parts = rows.map((row) => {
		const amount = moneyText(row.text, row.currency);
		if (total <= 0) return `${row.label} ${amount}`;
		return `${row.label} ${Math.round((row.value / total) * 100)}% (${amount})`;
	});
	const summary =
		rows.length === 0
			? "Category share: no data yet."
			: `Category share, ${moneyText(totalUnits, currency)} in total. ${parts.join(". ")}.`;

	return { rows, config, summary, total: { label: "Total", value: totalUnits, currency } };
}

/**
 * A string column of a hovered row, or `""` — layerchart hands the tooltip
 * whatever datum sits under the pointer, typed loosely, so the read is checked
 * rather than trusted.
 */
export function fieldText(data: unknown, column: string): string {
	if (data === null || typeof data !== "object") return "";
	const value = (data as Record<string, unknown>)[column];
	return typeof value === "string" ? value : "";
}

/** The category label of a hovered row, for a tooltip's heading. */
export function rowLabel(data: unknown): string {
	return fieldText(data, "label");
}

/** A hovered cartesian row as tooltip lines, formatted from the exact decimal strings. */
export function tooltipRows(data: unknown, prepared: readonly PreparedSeries[]): TooltipRow[] {
	const rows: TooltipRow[] = [];
	for (const one of prepared) {
		const text = fieldText(data, one.textColumn);
		if (text === "") continue;
		rows.push({
			name: one.name,
			color: one.color,
			units: text,
			currency: one.currency,
		});
	}
	return rows;
}

/** A hovered donut slice as a single tooltip line. */
export function shareTooltipRows(data: unknown): TooltipRow[] {
	const units = fieldText(data, "text");
	if (units === "") return [];
	return [
		{
			name: fieldText(data, "label"),
			color: fieldText(data, "color"),
			units,
			currency: fieldText(data, "currency"),
		},
	];
}
