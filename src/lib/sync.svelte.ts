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
	/** Why the last check failed, when it did. */
	error: "",
	/** A reload is in flight. */
	reloading: false,
});

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
}
