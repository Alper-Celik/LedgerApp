// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import {
	commands,
	type BudgetPluginStatus,
	type BudgetRange,
	type BudgetSettings,
} from "../../bindings";
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
 * The same load reads the budget's *configuration*: whether the plugin is
 * declared, the `custom "envelope"` block, and each envelope's newest `fund`
 * and `rollover`. That is what the screen edits — never the emitted figures —
 * so it has to be read alongside the months and refreshed by the same
 * `invalidateAll` after a write.
 *
 * The backend's refusal is handed to the page rather than thrown: "this ledger
 * has no envelope budget data" is a state the screen has something to say
 * about, and a thrown error would replace it with SvelteKit's own error page.
 * A settings failure is kept separate: the grid can still show what the plugin
 * wrote even when the editor cannot read the configuration.
 */
export const load: PageLoad = async ({ url }) => {
	const from = url.searchParams.get("from");
	const to = url.searchParams.get("to");
	const [range, settings, plugin] = await Promise.allSettled([
		commands.budgetRange(from || null, to || null),
		commands.budgetSettings(),
		commands.budgetPluginStatus(),
	]);

	let months: BudgetRange | null = null;
	let error = "";
	if (range.status === "rejected") {
		console.error("budgetRange failed", range.reason);
		error = messageOf(range.reason);
	} else if (range.value.status === "error") {
		error = range.value.error;
	} else {
		months = range.value.data;
	}

	let configuration: BudgetSettings | null = null;
	let settingsError = "";
	if (settings.status === "rejected") {
		console.error("budgetSettings failed", settings.reason);
		settingsError = messageOf(settings.reason);
	} else if (settings.value.status === "error") {
		settingsError = settings.value.error;
	} else {
		configuration = settings.value.data;
	}

	// The plugin's own status is what the screen's notice and its repair
	// controls read: whether the journal declares the module, which file
	// declares it, and the paths the `.wasm` can sit at. No ledger loaded is
	// an empty status, not a failure.
	let pluginStatus: BudgetPluginStatus | null = null;
	let pluginError = "";
	if (plugin.status === "rejected") {
		console.error("budgetPluginStatus failed", plugin.reason);
		pluginError = messageOf(plugin.reason);
	} else {
		pluginStatus = plugin.value;
	}

	return {
		range: months,
		error,
		settings: configuration,
		settingsError,
		plugin: pluginStatus,
		pluginError,
	};
};
