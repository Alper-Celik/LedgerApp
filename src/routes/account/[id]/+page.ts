// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../../bindings";
import type { PageLoad } from "./$types";

/** Loads the register for the account in the URL (read-only). */
export const load: PageLoad = async ({ params }) => {
	const account = decodeURIComponent(params.id ?? "");
	return { account, transactions: await commands.accountTransactions(account) };
};
