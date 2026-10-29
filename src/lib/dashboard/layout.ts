// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { WidgetSpan } from "./types.js";

/** The dashboard grid: one column on a phone, two on a tablet, four on a desktop. */
export const GRID_COLUMNS = { sm: 1, md: 2, lg: 4 } as const;

/** The width a dashboard is laid out at. */
export type Breakpoint = keyof typeof GRID_COLUMNS;

/** A column count the grid has at some breakpoint. */
export type Columns = (typeof GRID_COLUMNS)[Breakpoint];

/** Every span a widget can ask for, narrowest first. */
export const WIDGET_SPANS: readonly WidgetSpan[] = [1, 2, 4];

/** The grid container's class list; every breakpoint's column count lives here. */
export const GRID_CLASS = "grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-4";

/**
 * How many columns a widget actually occupies in a grid of `columns` columns.
 *
 * A span wider than the grid takes the whole row: a half-width widget on a
 * one-column phone, or a full-width one on a two-column tablet, gets the width
 * there is rather than overflowing it.
 */
export function spanFor(span: WidgetSpan, columns: Columns): Columns {
	return span <= columns ? span : columns;
}

/** Which spans a breakpoint can honour — the widths a widget may ask for there. */
export function spansAt(breakpoint: Breakpoint): WidgetSpan[] {
	const columns = GRID_COLUMNS[breakpoint];
	return WIDGET_SPANS.filter((span) => span <= columns);
}

/**
 * The Tailwind classes for a span, one per breakpoint.
 *
 * Written out rather than interpolated: Tailwind only ships classes it can see
 * in the source, so `col-span-${span}` would produce nothing. These are the
 * literal form of `spanFor(span, GRID_COLUMNS[breakpoint])` — keep the two in
 * step if a breakpoint ever changes.
 */
const SPAN_CLASSES: Record<WidgetSpan, string> = {
	1: "col-span-1 md:col-span-1 lg:col-span-1",
	2: "col-span-1 md:col-span-2 lg:col-span-2",
	4: "col-span-1 md:col-span-2 lg:col-span-4",
};

/** The classes that give a widget its span at every breakpoint. */
export function spanClasses(span: WidgetSpan): string {
	return SPAN_CLASSES[span];
}

/** The least a placement needs to know about a widget. */
export type Placeable = { id: string; span: WidgetSpan; order: number };

/** Where a widget sits: a 0-based row, a 1-based column, and the width it got. */
export type Placement = { id: string; row: number; column: number; span: Columns };

/**
 * Auto-placement, the way CSS grid does it without `dense`: walk the widgets in
 * `order`, put each in the next free columns of the current row, and start a new
 * row when it no longer fits.
 *
 * A row is therefore a run of widgets whose spans add up to at most `columns`;
 * a wide widget that does not fit leaves the rest of its row empty, exactly as
 * the grid would. The placements are pure positions — no DOM, no measurement.
 */
export function place(widgets: readonly Placeable[], columns: Columns): Placement[] {
	const ordered = [...widgets].sort((a, b) => a.order - b.order);
	const placements: Placement[] = [];
	let row = 0;
	let column = 1;

	for (const widget of ordered) {
		const span = spanFor(widget.span, columns);
		if (column - 1 + span > columns) {
			row += 1;
			column = 1;
		}
		placements.push({ id: widget.id, row, column, span });
		column += span;
		if (column > columns) {
			row += 1;
			column = 1;
		}
	}

	return placements;
}
