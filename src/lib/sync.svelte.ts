// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { commands, type ExternalChange } from "../bindings";
import { messageOf } from "./errors.js";
import { ledger } from "./ledger.svelte.js";

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
	/** Why the last read of the change list failed, when it did. */
	error: "",
	/**
	 * Why the last push into a SAF folder failed, when it did. Kept apart from
	 * `error`: one is "the app cannot tell whether the folder moved", the other is
	 * "a write did not reach the folder", and only the second must survive a
	 * successful read.
	 */
	treeError: "",
	/** A reload is in flight. */
	reloading: false,
});

/**
 * Take what a tree reconciliation reported.
 *
 * An error and a report are different things: a failed pass says nothing about
 * which files a successful one found, so it must not erase the conflicts whose
 * buttons the reader needs — and a pass that succeeded is what clears the error,
 * since nothing else can.
 */
export function noteTreeReport(report: {
	external?: string[];
	conflicts?: { path: string }[];
	error?: string;
}) {
	if (report.external) syncWatch.external = report.external;
	if (report.conflicts) syncWatch.conflicts = report.conflicts.map((conflict) => conflict.path);
	if (report.error !== undefined) {
		syncWatch.treeError = report.error;
	} else if (report.external && report.conflicts) {
		// A pass that completed is what clears its own failure.
		syncWatch.treeError = "";
	}
}

/** Re-read the watch list. Cheap: one command call, no ledger walk. */
export async function refreshSyncWatch() {
	try {
		const result = await commands.syncChanges();
		if (result.status === "error") {
			// No ledger open is the app's normal first state, not a failure to
			// report: there is no folder to have changed.
			syncWatch.error = ledger.summary === null ? "" : result.error;
			return;
		}
		syncWatch.changes = result.data;
		syncWatch.error = "";
	} catch (cause) {
		// A ledger in a plain local folder never reports anything; a failure here
		// must not disturb the screen.
		syncWatch.error = ledger.summary === null ? "" : messageOf(cause);
	}
}

/** Dismiss the bar until the next change. */
export function dismissSyncWatch() {
	syncWatch.changes = [];
	syncWatch.external = [];
	syncWatch.conflicts = [];
	// A dismissed failure is one the reader has seen; the next pass reports a new
	// one. Neither field is cleared blindly: the tree push is a different problem
	// from the change list, and its own pass is what retires it.
	syncWatch.error = "";
}
