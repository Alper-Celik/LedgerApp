// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { moneySegments } from "$lib/money.js";
import type { ChartDatum, ChartSeries } from "./types.js";

/**
 * `-1234.5` + `EUR` → `-1.234,50 €`.
 *
 * Grouping, decimal mark and the position of the currency symbol all come from
 * `$lib/money.js`, which formats the digits themselves: no amount is ever read
 * into a float to be displayed. The sign is kept, because a chart of net worth
 * or cash flow routinely runs through zero and the sign is the point.
 */
export function moneyText(units: string, currency: string): string {
	return moneySegments(units, currency)
		.map((segment) => segment.text)
		.join("");
}

/**
 * The pixel coordinate of an exact amount, or `undefined` for a value that is
 * not a number (an empty cell, which leaves a gap in the series).
 *
 * This is the one place an amount becomes a float, and it only ever feeds a
 * scale: every label the reader sees is formatted from the decimal string.
 */
export function plotNumber(value: string): number | undefined {
	if (value.trim() === "") return undefined;
	const parsed = Number(value);
	return Number.isFinite(parsed) ? parsed : undefined;
}

const compact = new Intl.NumberFormat(undefined, {
	notation: "compact",
	maximumFractionDigits: 1,
});

/**
 * Default value-axis tick label: `1500000` → `1.5M`.
 *
 * Axis ticks are positions on a scale, not amounts — they are rounded by the
 * scale itself and would read as a false precision in currency. Each chart
 * takes a `tickFormat` prop, so a screen that wants money on the axis can say
 * so; the tooltip always carries the exact amount either way.
 */
export function compactNumber(value: number): string {
	return compact.format(value);
}

/** The distinct category labels of every series, in the order they first appear. */
export function categoryLabels(series: readonly ChartSeries[]): string[] {
	const labels: string[] = [];
	const seen = new Set<string>();
	for (const one of series) {
		for (const point of one.points) {
			if (seen.has(point.label)) continue;
			seen.add(point.label);
			labels.push(point.label);
		}
	}
	return labels;
}

/**
 * The chart in words, for `aria-label`: a canvas is invisible to a screen
 * reader, so the summary names each series and where it starts and ends.
 */
export function describeSeries(
	kind: string,
	series: readonly ChartSeries[],
	unit: "month" | "day" = "month",
): string {
	const drawn = series.filter((one) => one.points.length > 0);
	if (drawn.length === 0) return `${kind}: no data yet.`;

	const count = categoryLabels(drawn).length;
	const span = count === 1 ? `1 ${unit}` : `${count} ${unit}s`;
	const parts = drawn.map((one) => {
		const first = one.points[0] as ChartDatum;
		const last = one.points[one.points.length - 1] as ChartDatum;
		if (first.label === last.label) {
			return `${one.name}: ${moneyText(last.value, last.currency)} in ${last.label}`;
		}
		return `${one.name}: from ${moneyText(first.value, first.currency)} in ${first.label} to ${moneyText(last.value, last.currency)} in ${last.label}`;
	});

	return `${kind}, ${span}. ${parts.join(". ")}.`;
}
