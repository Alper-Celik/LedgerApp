// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { messageOf } from "$lib/errors.js";
import { commands, type Dashboard } from "../../../../bindings";
import type { PageLoad } from "./$types";

/**
 * The dashboard the editor opens, or `null` for an id that is not saved yet.
 *
 * An unknown id is not a failure: `New` on the list page opens the editor under
 * a fresh id, and Save is what creates it. A store that cannot be read is
 * reported as a message beside the editor, which still lets a dashboard be
 * saved — `saveDashboard` is an upsert by id.
 */
export const load: PageLoad = async ({ params }) => {
	const id = params.id ?? "";
	try {
		const result = await commands.dashboards();
		if (result.status === "error") return { id, dashboard: null, error: result.error };
		return {
			id,
			dashboard: result.data.find((one: Dashboard) => one.id === id) ?? null,
			error: "",
		};
	} catch (cause) {
		console.error("dashboard editor failed", cause);
		return { id, dashboard: null, error: messageOf(cause) };
	}
};
