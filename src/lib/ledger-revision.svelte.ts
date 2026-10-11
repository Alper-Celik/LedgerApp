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
