// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { BudgetRangeMonth } from "../bindings";

/**
 * How many month columns the grid shows at a given content width: one under
 * 500px, then one more per 250px up to six — the widths the multi-month screen
 * was designed at. Past six months a window slides rather than squeezing the
 * figures. (A phone forces one column before this is consulted; these are the
 * steps a desktop window's content area takes.)
 */
export function budgetColumns(width: number): number {
	if (width < 500) return 1;
	if (width < 750) return 2;
	if (width < 1000) return 3;
	if (width < 1250) return 4;
	if (width < 1500) return 5;
	return 6;
}

/**
 * A window start clamped so `count` columns sit inside `total` months: the
 * window never runs off either end of the data, and is never wider than it.
 * `0` while there is nothing to show.
 */
export function clampWindowStart(start: number, count: number, total: number): number {
	if (total <= 0 || count <= 0) return 0;
	const size = Math.min(count, total);
	return Math.max(0, Math.min(Math.trunc(start), total - size));
}

/**
 * The window that ends on the newest month — where the screen opens, and where
 * `0` returns to after navigating: the newest month stays visible while the
 * window is narrower than the data.
 */
export function newestWindowStart(count: number, total: number): number {
	if (total <= 0) return 0;
	return total - Math.min(count, total);
}

/**
 * The `grid-template-columns` every band and every row is laid out on, so one
 * month's three figures line up from the summary strip down to the last
 * envelope: a category column, then one track per month wide enough for
 * Budgeted, Spent and Balance side by side, and no wider than four times that —
 * a handful of months on a wide screen should not stretch each figure a third
 * of the window apart. The month track is exactly three times the money track's
 * minimum, so an inner three-column grid divides it evenly and needs no widths
 * of its own.
 */
export function budgetGridTemplate(count: number): string {
	return `minmax(7rem, 1.5fr) repeat(${Math.max(count, 1)}, minmax(12rem, 24rem))`;
}

/** One envelope's or group's three figures in one month. */
export type BudgetCell = {
	/** What the month put into it, net of rollover corrections. */
	budgeted: string;
	/** What the month spent out of it, as a positive figure. */
	spent: string;
	/** What it held at the month's end. */
	balance: string;
};

/**
 * A month's figures for a row that month did not report. An envelope or group
 * absent from a month's payload has no envelopes below it, so nothing was
 * assigned, spent or held: the grid shows zeros rather than a blank that would
 * break the column's arithmetic. The decimal string stays exact, as everywhere
 * else.
 */
const EMPTY_CELL: BudgetCell = { budgeted: "0", spent: "0", balance: "0" };

/** One line of the grid: a group's own totals, or one of its envelopes. */
export type BudgetRow = {
	/** Stable key for `{#each}` and for nothing else: the group name, or its account. */
	key: string;
	/** The top-level branch the row belongs to — a group row's own name. */
	group: string;
	/** The envelope's label inside its group; empty on a group's own row. */
	envelope: string;
	/** The account the envelope writes to, the register link's target; empty on a group row. */
	fullName: string;
	/** One cell per visible month, in the same order as the months. */
	cells: BudgetCell[];
};

type GroupDraft = {
	/** The group's own figures per month, `null` where the month did not report it. */
	cells: (BudgetCell | null)[];
	/** Its envelopes in first-seen order — the ledger's tree order, oldest month first. */
	envelopes: Map<string, { name: string; cells: (BudgetCell | null)[] }>;
};

/**
 * The visible months as grid rows, each row carrying one cell per month so a
 * year of columns is drawn without asking the backend per group: a group's own
 * row (its totals, as the plugin summed them) followed by its envelopes.
 *
 * Groups are the union of every visible month, alphabetically, because an
 * envelope funded in one month and not the next must still have a row to
 * compare across; the months that lacked it read as zeros. Envelopes keep the
 * order the first month reporting them wrote — the ledger's tree order — so a
 * column's rows do not shuffle as the window slides.
 */
export function budgetRows(months: readonly BudgetRangeMonth[]): BudgetRow[] {
	const groups = new Map<string, GroupDraft>();

	const draftOf = (name: string): GroupDraft => {
		let draft = groups.get(name);
		if (!draft) {
			draft = { cells: months.map(() => null), envelopes: new Map() };
			groups.set(name, draft);
		}
		return draft;
	};

	months.forEach((month, index) => {
		for (const group of month.groups) {
			const draft = draftOf(group.name);
			draft.cells[index] = {
				budgeted: group.budgeted,
				spent: group.spent,
				balance: group.balance,
			};
			for (const envelope of group.envelopes) {
				let row = draft.envelopes.get(envelope.full_name);
				if (!row) {
					row = { name: envelope.name, cells: months.map(() => null) };
					draft.envelopes.set(envelope.full_name, row);
				}
				row.cells[index] = {
					budgeted: envelope.budgeted,
					spent: envelope.spent,
					balance: envelope.balance,
				};
			}
		}
	});

	const rows: BudgetRow[] = [];
	for (const name of [...groups.keys()].sort((left, right) => left.localeCompare(right))) {
		const draft = groups.get(name);
		if (!draft) continue;
		rows.push({
			key: `group:${name}`,
			group: name,
			envelope: "",
			fullName: "",
			cells: draft.cells.map((value) => value ?? EMPTY_CELL),
		});
		for (const [fullName, envelope] of draft.envelopes) {
			rows.push({
				key: `envelope:${fullName}`,
				group: name,
				envelope: envelope.name,
				fullName,
				cells: envelope.cells.map((value) => value ?? EMPTY_CELL),
			});
		}
	}
	return rows;
}
