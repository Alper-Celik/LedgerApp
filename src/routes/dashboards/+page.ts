// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { messageOf } from "$lib/errors.js";
import { commands, type Dashboard } from "../../bindings";
import type { PageLoad } from "./$types";

/**
 * The saved dashboards.
 *
 * A failing command is reported as data, not thrown: the list page has a
 * sentence to show for it, while SvelteKit would replace the page with its own
 * error screen. The first run answers with the backend's seeded dashboard.
 */
export const load: PageLoad = async () => {
	try {
		const result = await commands.dashboards();
		if (result.status === "error") return { dashboards: [] as Dashboard[], error: result.error };
		return { dashboards: result.data, error: "" };
	} catch (cause) {
		console.error("dashboards failed", cause);
		return { dashboards: [] as Dashboard[], error: messageOf(cause) };
	}
};
