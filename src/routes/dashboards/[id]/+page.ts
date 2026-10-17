// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { dashboards, ledgerData } from "$lib/dashboards.js";
import type { PageLoad } from "./$types";

/**
 * The chosen dashboard's data.
 *
 * An id the registry does not know is answered with `known: false` rather than
 * thrown — a stale link should read as "no such dashboard", not as a broken
 * app. A failing command is reported the way the register reports one: as a
 * message for an Alert, since SvelteKit would turn a throw here into a full
 * error page.
 *
 * The component itself never crosses this boundary; a component is not data,
 * so the page reads the entry out of the registry again by id.
 */
export const load: PageLoad = async ({ params }) => {
	const id = decodeURIComponent(params.id ?? "");
	const entry = dashboards.find((dashboard) => dashboard.id === id);
	if (!entry) return { id, known: false, data: null, error: "" };
	try {
		return { id, known: true, data: await (entry.load ?? ledgerData)(), error: "" };
	} catch (cause) {
		console.error(`dashboard ${id} failed`, cause);
		return {
			id,
			known: true,
			data: null,
			error: cause instanceof Error ? cause.message : String(cause),
		};
	}
};
