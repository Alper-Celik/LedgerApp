// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Everything a widget's config may hold: it is written to a file, so JSON only. */
export type JsonValue =
	| string
	| number
	| boolean
	| null
	| JsonValue[]
	| { [key: string]: JsonValue };

/** A widget's saved settings. Values are JSON, and `normalise` puts the defaults back. */
export type WidgetConfig = { [key: string]: JsonValue };

/** Columns a widget spans: a quarter, a half, or the full dashboard grid. */
export type WidgetSpan = 1 | 2 | 4;

/**
 * The widgets a dashboard can hold.
 *
 * The kind is the only thing that decides what a widget shows; its `config`
 * only tunes it. Adding a kind is one entry in `registry.ts` and one value here.
 */
export type WidgetKind =
	| "net-worth"
	| "cash-flow"
	| "spending-by-month"
	| "budget-vs-actual"
	| "category-share"
	| "lifestyle-inflation"
	| "balances-table";

/**
 * One card on a dashboard: what it is, how it is tuned, and where it sits.
 *
 * Deliberately not derived from the generated `DashboardWidget`: that type's
 * `kind` is a string, because a saved file may hold one this build cannot draw.
 * This union is the set the registry can render, so adding a kind to the backend
 * has to be a decision here rather than a silent drop at runtime.
 */
export type Widget = {
	/** Stable identity, so a layout can be saved and reordered by it. */
	id: string;
	kind: WidgetKind;
	/** Plain JSON, normalised against the kind's defaults when the dashboard loads. */
	config: WidgetConfig;
	/** Columns it asks for; the grid narrows it when the row is too narrow. */
	span: WidgetSpan;
	/** Position in the dashboard; ties keep their saved order. */
	order: number;
};

/** A named dashboard: an ordered set of widgets saved alongside the ledger. */
export type Dashboard = {
	id: string;
	name: string;
	widgets: Widget[];
};

/** Whether a value read from a saved dashboard is a span this grid understands. */
export function isWidgetSpan(value: unknown): value is WidgetSpan {
	return value === 1 || value === 2 || value === 4;
}
