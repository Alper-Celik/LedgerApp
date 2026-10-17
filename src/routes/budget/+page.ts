// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands } from "../../bindings";
import type { PageLoad } from "./$types";

/**
 * The budget's newest month, one month's figures at a time — the backend folds
 * the plugin's own envelope entries into them. Its refusal is handed to the page
 * rather than thrown: "this ledger has no envelope budget data" is a state the
 * screen has something to say about, and a thrown error would replace it with
 * SvelteKit's own error page.
 */
export const load: PageLoad = async () => {
	try {
		const result = await commands.budget(null);
		if (result.status === "error") return { budget: null, error: result.error };
		return { budget: result.data, error: "" };
	} catch (cause) {
		console.error("budget failed", cause);
		return {
			budget: null,
			error: cause instanceof Error ? cause.message : String(cause),
		};
	}
};
