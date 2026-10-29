// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type {
	JournalFile,
	PriceQuote,
	PriceSourceInfo,
	PriceTarget,
} from "../../../bindings";

/**
 * One row's own fetch, kept beside the ledger's newest price rather than
 * replacing it: what the ledger holds and what a source has just answered are
 * two facts, and this screen is where the second is chosen to become the first.
 * A failure belongs to its row, so the rest of the table goes on either way.
 */
export type RowState =
	| { status: "idle" }
	| { status: "busy" }
	| { status: "quote"; quote: PriceQuote }
	| { status: "failed"; error: string };

/**
 * A commodity and the currency it is priced in name one price, so the pair is
 * both a row's identity and the key its fetch state is kept under. `null` and
 * `""` name the same thing here — "no currency chosen" — so the two callers
 * (a target, a quote) agree on the key.
 */
export function pairKey(commodity: string, currency: string | null): string {
	return `${commodity}|${currency ?? ""}`;
}

/**
 * The source a kind is priced from until the row is pointed elsewhere. The
 * registry says what it carries but not which kind each source quotes, so the
 * order below is this screen's own reading of the names: central-bank reference
 * rates for currencies, a market listing for traded commodities and funds. The
 * first name the build actually offers wins; failing that an official source,
 * failing that the first one — a row always has a source to send.
 */
/**
 * The screen's preference, mirroring `prices::default_source` in the backend: a
 * currency is a reference rate (ECB), everything else a market quote. The list
 * after the first is this screen's fallback chain, which the registry order does
 * not express; the first entry is the rule the CLI uses too.
 */
const PREFERRED: Record<PriceTarget["kind"], readonly string[]> = {
	currency: ["ecb", "ratesapi"],
	commodity: ["yahoo", "alphavantage"],
	fund: ["yahoo", "alphavantage"],
};

/** The source to preselect for `kind`, or `""` when the build offers none. */
export function defaultSource(kind: PriceTarget["kind"], sources: PriceSourceInfo[]): string {
	// Matched case-insensitively: the registry's own spelling is what gets sent back.
	const offered = new Map(sources.map((source) => [source.name.toLowerCase(), source.name]));
	for (const candidate of PREFERRED[kind]) {
		const name = offered.get(candidate);
		if (name !== undefined) return name;
	}
	return sources.find((source) => source.official)?.name ?? sources[0]?.name ?? "";
}

/**
 * The file a write lands in when none has been picked: the journal's root,
 * which is where a ledger writes its own `price` directives, or the first file
 * the journal lists when the root is not among them. No backend call says which
 * file the existing prices live in, so the root is as close as the screen can
 * stand to "the file the ledger's prices are in".
 */
export function defaultFile(files: JournalFile[], root: string): string {
	if (root !== "" && files.some((file) => file.path === root)) return root;
	return files[0]?.path ?? "";
}
