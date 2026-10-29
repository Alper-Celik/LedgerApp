// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/**
 * The budget's months, several at a time — the backend folds the envelope
 * plugin's own entries into them, one month per column. The window asked for is
 * the backend's own default (the six months ending at the newest the plugin
 * wrote) unless the URL names an end, so `?from=2025-08` opens further back and
 * `?to=2026-03` stops earlier; the screen then slides its columns inside
 * whatever came back.
 *
 * The backend's refusal is handed to the page rather than thrown: "this ledger
 * has no envelope budget data" is a state the screen has something to say
 * about, and a thrown error would replace it with SvelteKit's own error page.
 */
export const load: PageLoad = async ({ url }) => {
	const from = url.searchParams.get("from");
	const to = url.searchParams.get("to");
	try {
		const result = await commands.budgetRange(from || null, to || null);
		if (result.status === "error") return { range: null, error: result.error };
		return { range: result.data, error: "" };
	} catch (cause) {
		console.error("budgetRange failed", cause);
		return { range: null, error: messageOf(cause) };
	}
};
