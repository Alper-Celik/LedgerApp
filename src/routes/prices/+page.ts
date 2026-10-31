// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import {
	commands,
	type IndexSeries,
	type JournalFile,
	type PriceSourceInfo,
	type PriceTarget,
} from "../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/**
 * Everything the prices screen starts from, asked for at once: the pairs the
 * ledger's balances can be priced through, the sources this build offers, the
 * journal's files (a write has to land in one), the root journal (the file a
 * write defaults to), and the stored index series (an index point is a price,
 * so a stored series is a row that can be written into the journal).
 *
 * A backend failure comes back as this page's own error rather than a throw: an
 * unreadable ledger is a state the screen can explain, while SvelteKit's error
 * page would read as the app being broken. The index store is read apart from
 * the rest: a series store that cannot be read must not blank the price table.
 */
export const load: PageLoad = async () => {
	const empty = {
		targets: [] as PriceTarget[],
		sources: [] as PriceSourceInfo[],
		files: [] as JournalFile[],
		root: "",
		series: [] as IndexSeries[],
		seriesError: "",
	};
	let base = {
		targets: [] as PriceTarget[],
		sources: [] as PriceSourceInfo[],
		files: [] as JournalFile[],
		root: "",
	};
	try {
		const [targets, sources, files, summary] = await Promise.all([
			commands.priceTargets(),
			commands.priceSources(),
			commands.journalFiles(),
			commands.ledgerSummary(),
		]);
		base = { targets, sources, files, root: summary?.path ?? "" };
	} catch (cause) {
		console.error("prices load failed", cause);
		return { ...empty, error: messageOf(cause) };
	}

	let series: IndexSeries[] = [];
	let indexError = "";
	try {
		const result = await commands.indexSeries();
		if (result.status === "error") indexError = result.error;
		else series = result.data ?? [];
	} catch (cause) {
		console.error("index series failed", cause);
		indexError = messageOf(cause);
	}

	return { ...base, series, error: "", seriesError: indexError };
};
