// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invalidateAll } from "$app/navigation";
import { commands, type LoadSummary } from "../bindings";
import { messageOf } from "./errors.js";
import { leaveTreeMode, noteSyncReadError, noteTreeReport } from "./sync.svelte.js";
import { resetLanguageServer } from "./lsp.js";
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
	/**
	 * Why the folder the app was pointed at last launch could not be reopened —
	 * set before any window existed, so it is read from the backend rather than
	 * remembered here. Empty when there is nothing to report.
	 */
	storageError: "",
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
 * Read the folder's last failure, if there was one.
 *
 * The backend records it from four places — the launch restore, a pick, the
 * assistant's conversation flush, a git operation's note — and clears it on the
 * next successful one. Every writer runs while the window is already up (bar the
 * restore), so this is read after each of them, not only on mount: a failure
 * nobody re-reads is as invisible as the stderr line it replaced.
 */
export async function refreshStorageError() {
	try {
		ledger.storageError = (await commands.storageError()) ?? "";
	} catch {
		// No backend, or a build from before the command existed: a missing
		// explanation is no worse than a wrong one.
	}
}

/**
 * Open a ledger with the desktop's own folder chooser: Tauri's dialog plugin
 * behind `pick_ledger_folder` — the xdg-desktop-portal on Linux, the system
 * dialog on Windows and macOS. This is the normal way in.
 *
 * A cancelled chooser does nothing. A runtime whose chooser answers with an
 * error — no dialog plugin, or a phone where the system picker failed — opens
 * the in-app picker instead, so the button never ends in a dead end.
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
	let kind: string;
	try {
		kind = await commands.storageKind();
	} catch (cause) {
		// Whether the ledger is in a folder is a read; a failure here says
		// nothing about a write having been lost.
		noteSyncReadError(messageOf(cause));
		return;
	}
	if (kind !== "tree") {
		// Not a mirror: a report from an earlier tree ledger describes a folder
		// that is not this one, and the bar must not carry it over — and a git
		// repository's ledger has no folder watch for it to belong to at all.
		leaveTreeMode();
		return;
	}
	try {
		const result = await commands.syncLedgerTree();
		if (result.status === "error") {
			noteTreeReport({ error: result.error });
			return;
		}
		const report = result.data;
		// A write that never reached the folder, or a file the folder changed
		// under it, is the reader's to know about: an edit that silently stayed in
		// the private mirror would be lost on the next restore.
		noteTreeReport({ external: report.external, conflicts: report.conflicts });
	} catch (cause) {
		// A pass that failed says nothing about which files an earlier one found:
		// only the failure is news, and it must not erase the conflicts whose
		// buttons settle them.
		noteTreeReport({ error: messageOf(cause) });
	} finally {
		// The backend records a folder failure from several places; this is the
		// one call every write already goes through, so it is where the sentence
		// becomes visible (and where a cleared one stops being shown).
		await refreshStorageError();
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
		const previous = ledger.summary?.path ?? "";
		ledger.summary = result.data;
		// The backend cleared any restore failure as soon as the folder was
		// picked; read it so an alert about a folder that is now open goes away.
		await refreshStorageError();
		// A *different* journal invalidates the ids, conversations and selections
		// that remember one; reading the same journal again (the sync bar's
		// reload, a git operation) renumbers entries too, and `ledgerRevision` is
		// what every id-bearing surface watches for that. Bumping the epoch on a
		// plain reload would discard an open draft or transcript that is still
		// about this very ledger.
		if (result.data.path !== previous) bumpLedgerEpoch();
		// The backend stops its language server on every load — even of the same
		// journal — so the client has to start one again either way.
		resetLanguageServer();
		bumpLedgerRevision();
		// The backend has swapped its ledger and every id-bearing surface has
		// its revision, so the open has succeeded whatever a route `load` does
		// next. A failing refetch is a failed read, not a refused folder: saying
		// otherwise leaves the picker up over a ledger that is already open.
		try {
			await invalidateAll();
		} catch (cause) {
			console.error("invalidateAll failed", cause);
		}
		return true;
	} catch (cause) {
		ledger.error = messageOf(cause);
		return false;
	} finally {
		ledger.opening = false;
	}
}
