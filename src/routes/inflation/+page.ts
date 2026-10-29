// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands, type IndexSeries, type LifestyleReport } from "../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/**
 * What the inflation screen starts from: every stored index series, and the
 * lifestyle report the screen opens on — the first series over the last twelve
 * months, which is the one comparison that means something before the reader
 * has chosen anything.
 *
 * Failures come back as this page's own `error` rather than a throw. Neither
 * half makes the other worthless: a report that cannot be run (no ledger open,
 * a series in another currency, a series that does not reach back twelve
 * months) still leaves the index list usable, and an unreadable index store
 * still lets the page say what is wrong instead of showing SvelteKit's error
 * page.
 */
export const load: PageLoad = async () => {
	let series: IndexSeries[] = [];
	let report: LifestyleReport | null = null;
	const failures: string[] = [];

	try {
		const result = await commands.indexSeries();
		if (result.status === "error") failures.push(result.error);
		else series = result.data ?? [];
	} catch (cause) {
		console.error("index series failed", cause);
		failures.push(messageOf(cause));
	}

	// A first series is the id-sorted first one the store holds; a series with
	// no points still gets asked for, and the backend's own sentence about the
	// missing month is worth showing rather than guessing at here.
	const first = series[0];
	if (first) {
		try {
			const result = await commands.lifestyleReport(12, first.id);
			if (result.status === "error") failures.push(result.error);
			else report = result.data;
		} catch (cause) {
			console.error("lifestyle report failed", cause);
			failures.push(messageOf(cause));
		}
	}

	return { series, report, error: failures.join("\n") };
};
