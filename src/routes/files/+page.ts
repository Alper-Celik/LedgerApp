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
 * screen and the sidebar make; it answers an empty list when no ledger is
 * loaded rather than failing, so an empty answer is not by itself the app's
 * "nothing is open" state — `ledgerSummary` is what tells the two apart, and it
 * answers `null` exactly when no journal is loaded. A failure of the listing
 * with a ledger open is the only real error to show.
 */
export const load: PageLoad = async (): Promise<FilesData> => {
	try {
		const files = (await commands.journalFiles()) ?? [];
		if (files.length > 0) return { files, error: "", noLedger: false };
		const summary = await commands.ledgerSummary().catch(() => null);
		return { files, error: "", noLedger: summary === null };
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
