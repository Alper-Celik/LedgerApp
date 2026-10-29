// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { commands, type ExternalChange } from "../bindings";
import { messageOf } from "./errors.js";

/**
 * Files the ledger's folder changed behind the app — another device, a cloud
 * client. The backend compares the disk against the operation log's last-seen
 * map (see `src-tauri/src/sync.rs`), so this lists exactly the files whose bytes
 * are not the ones this device last wrote.
 *
 * The screen offers a reload rather than doing one: a reload replaces the ledger
 * under any open editor draft, which is the reader's call.
 */
export const syncWatch = $state({
	changes: [] as ExternalChange[],
	/**
	 * Files a SAF tree changed while the app had them: the app never overwrites
	 * one, so they need the same reload an external change does.
	 */
	external: [] as string[],
	/** Files changed on both sides, which a reload cannot settle. */
	conflicts: [] as string[],
	/** Why the last check failed, when it did. */
	error: "",
	/** A reload is in flight. */
	reloading: false,
});

/** Take what a tree reconciliation reported, replacing the previous report. */
export function noteTreeReport(report: {
	external: string[];
	conflicts: { path: string }[];
	error?: string;
}) {
	syncWatch.external = report.external;
	syncWatch.conflicts = report.conflicts.map((conflict) => conflict.path);
	if (report.error !== undefined) syncWatch.error = report.error;
}

/** Re-read the watch list. Cheap: one command call, no ledger walk. */
export async function refreshSyncWatch() {
	try {
		const result = await commands.syncChanges();
		if (result.status === "error") {
			syncWatch.error = result.error;
			return;
		}
		syncWatch.changes = result.data;
		syncWatch.error = "";
	} catch (cause) {
		// A ledger in a plain local folder never reports anything; a failure here
		// must not disturb the screen.
		syncWatch.error = messageOf(cause);
	}
}

/** Dismiss the bar until the next change. */
export function dismissSyncWatch() {
	syncWatch.changes = [];
	syncWatch.external = [];
	syncWatch.conflicts = [];
}
