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
 * Open a ledger with the desktop's own folder chooser: Tauri's dialog plugin
 * behind `pick_ledger_folder` — the xdg-desktop-portal on Linux, the system
 * dialog on Windows and macOS. This is the normal way in.
 *
 * A cancelled chooser does nothing. A platform with no chooser (phones, today)
 * answers with an error, and the in-app picker opens instead, so the button
 * never ends in a dead end.
 */
export async function chooseLedger() {
	ledger.error = "";
	try {
		const result = await commands.pickLedgerFolder();
		if (result.status === "error") {
			openFolderPicker();
			return;
		}
		if (result.data) await openLedgerPath(result.data);
	} catch (cause) {
		ledger.error = messageOf(cause);
	}
}

/**
 * Write the local mirror's changed files back into the SAF tree, when the ledger
 * lives in one. Every successful edit bumps the revision, so this is the one
 * place a phone's changes reach the folder the sync client watches; failures are
 * logged, never surfaced as a failed edit (the edit itself did happen).
 */
export async function syncTree(): Promise<void> {
	try {
		const kind = await commands.storageKind();
		if (kind !== "tree") return;
		await commands.syncLedgerTree();
	} catch (cause) {
		console.error("tree sync failed", cause);
	}
}

/**
 * Open the in-app folder picker. This is the fallback for runtimes whose window
 * stack cannot show a native chooser; it lists directories through
 * `list_folders` (see `folders.rs`).
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
