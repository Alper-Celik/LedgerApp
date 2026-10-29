// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import {
	commands,
	type JournalFile,
	type PriceSourceInfo,
	type PriceTarget,
} from "../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/**
 * Everything the prices screen starts from, asked for at once: the pairs the
 * ledger's balances can be priced through, the sources this build offers, the
 * journal's files (a write has to land in one) and the root journal (the file
 * a write defaults to). A backend failure comes back as this page's own error
 * rather than a throw: an unreadable ledger is a state the screen can explain,
 * while SvelteKit's error page would read as the app being broken.
 */
export const load: PageLoad = async () => {
	const empty = {
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
		return { targets, sources, files, root: summary?.path ?? "", error: "" };
	} catch (cause) {
		console.error("prices load failed", cause);
		return { ...empty, error: messageOf(cause) };
	}
};
