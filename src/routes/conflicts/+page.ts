// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/**
 * The sync client's conflict copies under the ledger folder — the resolver's
 * list. A scan failure is handed to the page rather than thrown: "the folder
 * cannot be read" is a state the screen has an alert for, and a thrown error
 * would replace it with SvelteKit's own error page.
 */
export const load: PageLoad = async () => {
	try {
		const result = await commands.syncConflicts();
		return result.status === "ok"
			? { report: result.data, error: "" }
			: { report: null, error: result.error };
	} catch (cause) {
		console.error("syncConflicts failed", cause);
		return { report: null, error: messageOf(cause) };
	}
};
