// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../../bindings";
import type { PageLoad } from "./$types";

/**
 * The account's register. A backend failure is reported to the page rather than
 * thrown: SvelteKit would turn an exception here into a full error page, which
 * reads as "the app is broken" when only the ledger is unreadable.
 */
export const load: PageLoad = async ({ params }) => {
	const account = decodeURIComponent(params.id ?? "");
	try {
		return { account, transactions: await commands.accountTransactions(account), error: "" };
	} catch (cause) {
		console.error("accountTransactions failed", cause);
		return { account, transactions: [], error: messageOf(cause) };
	}
};

function messageOf(cause: unknown): string {
	return cause instanceof Error ? cause.message : String(cause);
}
