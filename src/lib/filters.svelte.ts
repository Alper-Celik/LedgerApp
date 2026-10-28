// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { TransactionView } from "../bindings";

/**
 * Transaction filters, shared by every view that lists transactions. They are
 * module state on purpose: every register reads the same query, so it survives
 * navigating between accounts without being threaded through the routes.
 */
export const filters = $state({
	/** Free text over date, flag, payee, narration and every posting. */
	query: "",
});

export function clearFilters() {
	filters.query = "";
}

/** True when `tx` passes the shared free-text query. */
export function matchesFilters(tx: TransactionView): boolean {
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
