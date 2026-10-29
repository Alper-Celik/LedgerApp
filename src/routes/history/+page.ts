// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands, type Operation } from "../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/** One page of the log; Load more asks for a bigger `limit`, not a cursor. */
const PAGE = 50;

/**
 * The operation log's newest page — one line per write the app recorded, with
 * who made it and what it touched. A read failure is handed to the page rather
 * than thrown: "no log yet" and "the log is unreadable" are states the screen
 * has its own words for, and a thrown error would replace them with SvelteKit's
 * error page.
 */
export const load: PageLoad = async () => {
	try {
		const result = await commands.history(null, null, PAGE);
		return result.status === "ok"
			? { operations: result.data, page: PAGE, error: "" }
			: { operations: [] as Operation[], page: PAGE, error: result.error };
	} catch (cause) {
		console.error("history failed", cause);
		return { operations: [] as Operation[], page: PAGE, error: messageOf(cause) };
	}
};
