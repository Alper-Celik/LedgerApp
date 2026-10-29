// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Bumped whenever the ledger's source file changes on disk — an edit saved, a
 * commit made. Surfaces that keep their own copy of ledger data (the sidebar's
 * balances, which are fetched once rather than loaded per route) refetch when
 * it changes; routed views get the same via `invalidateAll()`.
 */
export const ledgerRevision = $state({ value: 0 });

/** Call after an edit or a commit so dependent surfaces refetch. */
export function bumpLedgerRevision() {
	ledgerRevision.value += 1;
}

/**
 * Bumped when a *different journal* becomes the session's ledger.
 *
 * A write (the revision above) must leave an open draft alone; a different
 * ledger must not. Transaction ids are positions in the booked stream, so entry
 * 7 of one journal is an unrelated entry in another — a draft kept across a
 * switch would save onto the wrong transaction. Every surface holding an id
 * from the previous ledger watches this and drops its draft.
 */
export const ledgerEpoch = $state({ value: 0 });

/** Call after a load *replaced* the ledger, before the routes refetch. */
export function bumpLedgerEpoch() {
	ledgerEpoch.value += 1;
}
