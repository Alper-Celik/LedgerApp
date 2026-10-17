// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands, type JournalFile, type TransactionView } from "../../../bindings";
import type { PageLoad } from "./$types";

/**
 * One journal file, as the file browser's row for it opens: the file it names —
 * the journal's own listing again, so the page shows the file's own label and
 * notices when the journal no longer holds it — and the transactions the file
 * writes, which is the page's first mode. The other mode is the file's text,
 * which the editor fetches for itself.
 *
 * A backend failure is reported to the page rather than thrown: SvelteKit would
 * turn an exception here into a full error page, which reads as "the app is
 * broken" when only the ledger is unreadable.
 */
export const load: PageLoad = async ({ params }) => {
	const path = params.path ?? "";
	const failed = (error: string, files: JournalFile[] = [], file: JournalFile | null = null) => ({
		path,
		files,
		file,
		transactions: [] as TransactionView[],
		error,
	});
	const messageOf = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

	let files: JournalFile[];
	try {
		files = await commands.journalFiles();
	} catch (cause) {
		console.error("journalFiles failed", cause);
		return failed(messageOf(cause));
	}
	const file = files.find((candidate) => candidate.path === path) ?? null;
	if (!file) return failed(`the journal holds no file at ${path}`, files);

	try {
		const result = await commands.fileTransactions(path);
		if (result.status === "error") return failed(result.error, files, file);
		return { path, files, file, transactions: result.data, error: "" };
	} catch (cause) {
		console.error("fileTransactions failed", cause);
		return failed(messageOf(cause), files, file);
	}
};
