// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { WidgetSpan } from "./types.js";

/** Every span a widget can ask for, narrowest first. */
export const WIDGET_SPANS: readonly WidgetSpan[] = [1, 2, 4];

/** The grid container's class list; every breakpoint's column count lives here. */
export const GRID_CLASS = "grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-4";

/**
 * The Tailwind classes for a span, one per breakpoint.
 *
 * Written out rather than interpolated: Tailwind only ships classes it can see
 * in the source, so `col-span-${span}` would produce nothing. A widget wider than
 * the grid takes the whole row, which is what `col-span-1` at `sm` is: the grid
 * does the placement, this file only names the classes it does it with.
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
