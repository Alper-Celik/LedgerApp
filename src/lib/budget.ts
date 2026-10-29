// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { signOf } from "./money.js";

/**
 * A `YYYY-MM` month as a heading ("October 2026"). The budget's month is a label
 * rather than an instant — the plugin never consults a clock — so it is anchored
 * at its first day and read in UTC, the one reading of it that cannot slip into
 * the month before or after.
 */
export function monthLabel(month: string): string {
	const [year, number] = month.split("-").map(Number);
	if (!year || !number) return month;
	return new Intl.DateTimeFormat(undefined, {
		month: "long",
		year: "numeric",
		timeZone: "UTC",
	}).format(new Date(Date.UTC(year, number - 1, 1)));
}

/**
 * The same month as a tab label ("Oct"): a phone shows every month of the range
 * as one tab, so the year is carried by the summary card and the tab's own
 * `title` rather than repeated in each one.
 */
export function shortMonthLabel(month: string): string {
	const [year, number] = month.split("-").map(Number);
	if (!year || !number) return month;
	return new Intl.DateTimeFormat(undefined, { month: "short", timeZone: "UTC" }).format(
		new Date(Date.UTC(year, number - 1, 1))
	);
}

/**
 * Which kind of figure is coloured. Almost every positive is ordinary text: an
 * envelope holding money is not news, and colouring it would paint the whole
 * grid. The exception is a month's To budget, where money still to assign is the
 * one thing on the screen that is good news.
 */
export type FigureTone = "plain" | "to-budget";

/**
 * A budget figure's colour: overspent or over-assigned is red, an untouched
 * envelope faint, everything else the ordinary text colour. Read from the
 * decimal's own sign, so an amount never passes through a float.
 */
export function figureClass(units: string, tone: FigureTone = "plain"): string {
	const sign = signOf(units);
	if (sign < 0) return "text-removed";
	if (sign > 0) return tone === "to-budget" ? "text-emerald-600 dark:text-emerald-400" : "";
	return "text-muted-foreground";
}

/**
 * localStorage key for the groups folded away on the budget screen. Versioned:
 * the JSON shape may change, and an older entry should be ignored rather than
 * misread — bump the suffix when the shape does.
 */
const COLLAPSED_GROUPS_STORAGE_KEY = "budget_collapsed_groups_v1";

/**
 * The groups the reader has folded away, as stored. Unreadable or foreign
 * entries give an empty list: the collapse state is a convenience, and a
 * corrupt one should cost nothing but a re-fold.
 */
export function readCollapsedGroups(): string[] {
	if (typeof localStorage === "undefined") return [];
	try {
		const stored: unknown = JSON.parse(
			localStorage.getItem(COLLAPSED_GROUPS_STORAGE_KEY) ?? "[]"
		);
		return Array.isArray(stored)
			? stored.filter((name): name is string => typeof name === "string")
			: [];
	} catch {
		return [];
	}
}

/** Remembers the folded groups across reloads, under the versioned key. */
export function writeCollapsedGroups(groups: readonly string[]): void {
	if (typeof localStorage === "undefined") return;
	localStorage.setItem(COLLAPSED_GROUPS_STORAGE_KEY, JSON.stringify(groups));
}
