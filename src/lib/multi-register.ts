// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { TransactionView } from "../bindings";

/**
 * Merges the registers of several accounts into one, newest first. A
 * transaction that touches more than one selected account appears once: the
 * backend answers per account, so identity — not content — deduplicates here.
 */
export function mergeRegisters(lists: TransactionView[][]): TransactionView[] {
	const byId = new Map<number, TransactionView>();
	for (const list of lists) {
		for (const tx of list) {
			if (!byId.has(tx.id)) byId.set(tx.id, tx);
		}
	}
	return [...byId.values()].sort((a, b) => b.date.localeCompare(a.date) || b.id - a.id);
}

/**
 * Reads the `accounts` URL parameter into a deduped, ordered selection. The
 * value arrives already URL-decoded by `URL.searchParams`; commas separate
 * accounts, and account names themselves never contain them.
 */
export function parseSelection(param: string | null): string[] {
	if (!param) return [];
	const seen = new Set<string>();
	const out: string[] = [];
	for (const part of param.split(",")) {
		const name = part.trim();
		if (name && !seen.has(name)) {
			seen.add(name);
			out.push(name);
		}
	}
	return out;
}

/** Joins a selection back into the `accounts` URL parameter, per account encoded. */
export function selectionParam(accounts: string[]): string {
	return accounts.map(encodeURIComponent).join(",");
}
