// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../bindings";
import type { PageLoad } from "./$types";

/** The whole ledger, or an explanatory error instead of a thrown one. */
export const load: PageLoad = async () => {
	try {
		return { account: "", transactions: await commands.accountTransactions(""), error: "" };
	} catch (cause) {
		console.error("accountTransactions failed", cause);
		return { account: "", transactions: [], error: cause instanceof Error ? cause.message : String(cause) };
	}
};
