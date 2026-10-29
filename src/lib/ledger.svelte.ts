// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invalidateAll } from "$app/navigation";
import { commands, type LoadSummary } from "../bindings";
import { messageOf } from "./errors.js";
import { noteTreeReport } from "./sync.svelte.js";
import { bumpLedgerEpoch, bumpLedgerRevision } from "./ledger-revision.svelte.js";

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
		const result = await commands.syncLedgerTree();
		if (result.status === "error") {
			noteTreeReport({ external: [], conflicts: [], error: result.error });
			return;
		}
		const report = result.data;
		// A write that never reached the folder, or a file the folder changed
		// under it, is the reader's to know about: an edit that silently stayed in
		// the private mirror would be lost on the next restore.
		noteTreeReport({ external: report.external, conflicts: report.conflicts });
	} catch (cause) {
		noteTreeReport({ external: [], conflicts: [], error: messageOf(cause) });
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
		// A ledger in a SAF tree is read from the app's private mirror, so
		// reopening the path would re-read the same stale bytes. The folder's
		// versions are pulled into the mirror first — that is what makes the sync
		// bar's "Reload ledger" mean anything there.
		if ((await commands.storageKind()) === "tree") {
			const pulled = await commands.reloadLedgerTree();
			if (pulled.status === "ok") {
				noteTreeReport({ external: pulled.data.external, conflicts: pulled.data.conflicts });
			} else {
				// The mirror was not refreshed, so loading now would present the
				// bytes the reader already had as the folder's: refuse, and say why.
				ledger.error = pulled.error;
				noteTreeReport({ error: pulled.error });
				return false;
			}
		}
		const result = await commands.loadLedger(path);
		if (result.status === "error") {
			ledger.error = result.error;
			return false;
		}
		ledger.summary = result.data;
		// A different journal: drafts, selections and half-typed entries that
		// remember an id from the previous one are now about nothing.
		bumpLedgerEpoch();
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
