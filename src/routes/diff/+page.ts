// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../bindings";
import type { PageLoad } from "./$types";

/** The ledger's uncommitted changes (working tree + index) against HEAD. */
export const load: PageLoad = async () => {
	const result = await commands.ledgerDiff();
	return result.status === "ok" ? { diff: result.data, error: "" } : { diff: "", error: result.error };
};
