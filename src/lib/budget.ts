// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { BudgetMonth } from "../bindings";
import { signOf, sumDecimals } from "./money.js";

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

/** What a month's envelope rows add up to, as exact decimal strings. */
export type BudgetTotals = {
	budgeted: string;
	spent: string;
	balance: string;
};

/**
 * The month's column totals over every group. The backend works out each
 * envelope's own figures; the page only ever adds those up, and does it with
 * `sumDecimals`, so no amount passes through a float.
 */
export function budgetTotals(budget: BudgetMonth): BudgetTotals {
	return {
		budgeted: sumDecimals(budget.groups.map((group) => group.budgeted)),
		spent: sumDecimals(budget.groups.map((group) => group.spent)),
		balance: sumDecimals(budget.groups.map((group) => group.balance)),
	};
}

/**
 * The balance pill's classes: green with money left in the envelope, grey at
 * exactly zero, red once it is overspent — the same colours the register paints
 * amounts in, as a fill rather than a text colour so the column reads as a
 * column of balances. An overspent pill also takes a ring, so it stays marked
 * where colour alone would not carry.
 */
export function balancePillClass(units: string): string {
	const sign = signOf(units);
	if (sign > 0) {
		return "bg-emerald-600/10 text-emerald-700 dark:bg-emerald-400/15 dark:text-emerald-400";
	}
	if (sign < 0) {
		return "bg-red-600/10 text-red-700 ring-1 ring-red-600/30 ring-inset dark:bg-red-400/15 dark:text-red-400 dark:ring-red-400/30";
	}
	return "bg-muted text-muted-foreground";
}

/**
 * The months either side of `month` among `months`, each `null` at an end of the
 * range. The backend hands over exactly the months the plugin budgeted, so the
 * screen steps through those and never invents a month in between.
 */
export function monthNeighbours(
	months: string[],
	month: string
): { previous: string | null; next: string | null } {
	const index = months.indexOf(month);
	if (index === -1) return { previous: null, next: null };
	return { previous: months[index - 1] ?? null, next: months[index + 1] ?? null };
}
