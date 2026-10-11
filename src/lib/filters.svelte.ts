// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { TransactionView } from "../bindings";

/**
 * Transaction filters, shared by every view that lists transactions. They are
 * module state on purpose: a chart can zoom a date range straight into
 * `filters.from`/`filters.to` and the account page shows exactly those rows, so
 * "click a point to see the transactions behind it" needs no plumbing.
 *
 * Dates are `YYYY-MM-DD` (ISO compares lexicographically); `""` means open.
 */
export const filters = $state({
	/** Free text over date, flag, payee, narration and every posting. */
	query: "",
	from: "",
	to: "",
});

export function clearFilters() {
	filters.query = "";
	filters.from = "";
	filters.to = "";
}

/** True when `tx` passes the shared date range and free-text query. */
export function matchesFilters(tx: TransactionView): boolean {
	if (filters.from && tx.date < filters.from) return false;
	if (filters.to && tx.date > filters.to) return false;
	const query = filters.query.trim().toLowerCase();
	if (!query) return true;
	const haystack = [
		tx.date,
		tx.flag,
		tx.payee ?? "",
		tx.narration,
		...tx.postings.flatMap((posting) => [posting.account, posting.units, posting.currency]),
	];
	return haystack.some((field) => field.toLowerCase().includes(query));
}
