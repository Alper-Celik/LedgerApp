// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands, type AccountBalance, type TransactionView } from "../../bindings";
import { mergeRegisters, parseSelection } from "$lib/multi-register.js";
import type { PageLoad } from "./$types";

/**
 * The combined register of the selected accounts. Each account is fetched on
 * its own command and merged client-side, so no backend change is needed; a
 * failing account is reported by name while the rest still render — a broken
 * backend must not take the page down (same stance as the other registers).
 */
export const load: PageLoad = async ({ url }) => {
	const accounts = parseSelection(url.searchParams.get("accounts"));
	const allAccounts: AccountBalance[] = await commands.accounts().catch((cause) => {
		console.error("accounts failed", cause);
		return [];
	});
	if (accounts.length === 0) {
		return { accounts, allAccounts, transactions: [], error: "" };
	}

	const settled = await Promise.allSettled(
		accounts.map((account) => commands.accountTransactions(account))
	);
	const lists: TransactionView[][] = [];
	const failed: string[] = [];
	settled.forEach((result, index) => {
		if (result.status === "fulfilled") {
			lists.push(result.value);
		} else {
			console.error(`accountTransactions(${accounts[index]}) failed`, result.reason);
			failed.push(accounts[index]);
		}
	});

	return {
		accounts,
		allAccounts,
		transactions: mergeRegisters(lists),
		error: failed.length > 0 ? `Failed to load: ${failed.join(", ")}` : "",
	};
};
