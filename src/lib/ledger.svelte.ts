// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invalidateAll } from "$app/navigation";
import { commands, type LoadSummary } from "../bindings";
import { messageOf } from "./errors.js";
import { bumpLedgerRevision } from "./ledger-revision.svelte.js";

/**
 * The ledger the app has open, as the backend resolved it. `summary.path` is
 * the root journal — when a folder was picked it is the journal inside that
 * folder, not the folder itself — so the UI can name what it is showing.
 */
export const ledger = $state({
	summary: null as LoadSummary | null,
	/** Why the last open failed, when it did. */
	error: "",
	/** A load is in flight. */
	opening: false,
	/** The folder picker is showing. */
	pickerOpen: false,
});

/**
 * Read what is open right now, including the journal the app loaded at startup
 * (from the CLI argument, `LEDGER_PATH` or `main.bean` in the working
 * directory).
 */
export async function refreshLedger() {
	try {
		ledger.summary = await commands.ledgerSummary();
	} catch {
		// No backend — a plain browser during development. Nothing to report.
	}
}

/**
 * Open the folder picker. There is no native dialog behind it: under the Qt
 * runtime neither a plugin dialog nor an in-process GTK one can open (see
 * `folders.rs`), so the picker is part of the app and lists directories
 * through `list_folders`.
 */
export function openFolderPicker() {
	ledger.error = "";
	ledger.pickerOpen = true;
}

/** Hide the folder picker without choosing anything. */
export function closeFolderPicker() {
	ledger.pickerOpen = false;
}

/**
 * Load `path` — a folder holding a journal, or a journal file — as the
 * session's ledger and bring every view in line: the backend swaps its loaded
 * ledger, `invalidateAll` re-runs the route `load`s, and the revision gets the
 * sidebar's balances refetched.
 *
 * Returns whether the ledger was loaded; a refusal leaves the previously loaded
 * ledger in place and its complaint lands in `ledger.error`, so the picker can
 * stay open and say why.
 */
export async function openLedgerPath(path: string): Promise<boolean> {
	if (ledger.opening) return false;
	ledger.opening = true;
	ledger.error = "";
	try {
		const result = await commands.loadLedger(path);
		if (result.status === "error") {
			ledger.error = result.error;
			return false;
		}
		ledger.summary = result.data;
		bumpLedgerRevision();
		await invalidateAll();
		return true;
	} catch (cause) {
		ledger.error = messageOf(cause);
		return false;
	} finally {
		ledger.opening = false;
	}
}
