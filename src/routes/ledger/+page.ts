// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../bindings";
import type { PageLoad } from "./$types";

/** The whole ledger: the virtual root's register, read-only. */
export const load: PageLoad = async () => ({
	account: "",
	transactions: await commands.accountTransactions(""),
});
