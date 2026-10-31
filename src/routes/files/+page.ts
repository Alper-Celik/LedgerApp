// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands, type JournalFile } from "../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/** Everything the files screen says before the tree can be drawn. */
type FilesData = {
	/** The journal's files — its root and every `include` — in the backend's list. */
	files: JournalFile[];
	/** A failure of the listing itself, for a ledger that is open. */
	error: string;
	/** `error` is not an error: no journal is loaded, so there is nothing to list. */
	noLedger: boolean;
};

/**
 * The journal's files, once. `journalFiles` is the same read the per-file
 * screen and the sidebar make, and it fails when no ledger is loaded — which
 * is the app's ordinary first state, not something to show as a broken read.
 * `ledgerSummary` is asked only to tell the two apart: a summary means a ledger
 * is open and the listing failed; no summary means there is nothing to open yet.
 */
export const load: PageLoad = async (): Promise<FilesData> => {
	try {
		return { files: (await commands.journalFiles()) ?? [], error: "", noLedger: false };
	} catch (cause) {
		console.error("journalFiles failed", cause);
		const failure = messageOf(cause);
		try {
			const summary = await commands.ledgerSummary();
			if (summary === null) return { files: [], error: "", noLedger: true };
			return { files: [], error: failure, noLedger: false };
		} catch (summaryCause) {
			console.error("ledgerSummary failed", summaryCause);
			return { files: [], error: "", noLedger: true };
		}
	}
};
