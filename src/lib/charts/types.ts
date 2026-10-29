// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * One money amount as the ledger writes it.
 *
 * `value` stays the exact decimal string `rustledger` produced: charts plot a
 * number derived from it (pixels need one), but everything a reader sees — the
 * tooltip, the axis-free summary, the center of a donut — is built from this
 * string by `$lib/money.js`. Nothing anywhere rounds it through a float.
 */
export type ChartDatum = {
	/** Where the point sits on the category axis: a month (`2026-01`) or a category name. */
	label: string;
	/** Exact decimal string, e.g. `-1234.56`; the same form the backend answers with. */
	value: string;
	/** ISO 4217 code or commodity the amount is written in, e.g. `EUR`. */
	currency: string;
};

/**
 * A named series of points over the same category axis — normally months, one
 * point per month, ascending.
 *
 * Series are aligned by `label`, not by position: a series may start later or
 * skip a month, and the wrapped chart leaves a gap there instead of shifting
 * the other series' points onto the wrong month.
 */
export type ChartSeries = {
	/** Shown in the legend and the tooltip; the chart's accessible summary uses it too. */
	name: string;
	points: readonly ChartDatum[];
	/** Optional CSS color. Left out, the series takes `--chart-1`…`--chart-5` in turn. */
	color?: string;
};
