// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The conflict resolver's model: the naming the list and the panel share, and
 * the one pick vocabulary the backend takes.
 *
 * The commands and their shapes are generated bindings (`commands.conflictCompare`,
 * `commands.conflictResolve`, `ConflictView`, `DecisionDto`, `MergeSummaryDto`),
 * so this module holds only what the wire does not say: the four picks the
 * resolver offers, and how a client, a path and a classification read on
 * screen.
 */

import type { Candidate, DecisionDto, SyncClient } from "../../../bindings";

/**
 * What to keep for one directive the merge could not settle by itself.
 *
 * The backend's `conflict_resolve` takes these as plain strings; the union is
 * this module's, so the pick control cannot invent a fifth answer.
 */
export type ConflictPick = "ours" | "theirs" | "both" | "drop";

/** The pick control's four answers, in reading order. */
export const CHOICES: readonly { id: ConflictPick; label: string; hint: string }[] = [
	{ id: "ours", label: "Ours", hint: "Keep the original file's version" },
	{ id: "theirs", label: "Theirs", hint: "Keep the conflicted copy's version" },
	{ id: "both", label: "Both", hint: "Keep both versions, ours first" },
	{ id: "drop", label: "Drop", hint: "Keep neither version" },
];

/** How a client names itself in a heading. */
const CLIENT_LABELS: Record<SyncClient, string> = {
	drive: "Google Drive",
	dropbox: "Dropbox",
	one_drive: "OneDrive",
	nextcloud: "Nextcloud",
	syncthing: "Syncthing",
	seafile: "Seafile",
	icloud: "iCloud Drive",
};

export function clientLabel(client: SyncClient): string {
	return CLIENT_LABELS[client] ?? client;
}

/** The last segment of a path, for a row that shows the full path as its title. */
export function nameOf(path: string): string {
	const parts = path.split(/[\\/]/);
	return parts[parts.length - 1] || path;
}

const ISO_DATE = /\d{4}-\d{2}-\d{2}/;

/**
 * The decision's date: the first ISO date its label spells out (`2026-01-10 *
 * "Groceries"`), or, when the label omits one, the first date either side
 * carries. Directives without a date (`option`, `include`, `plugin`) answer
 * null.
 */
export function dateOf(decision: DecisionDto): string | null {
	return (
		decision.label.match(ISO_DATE)?.[0] ??
		decision.ours?.match(ISO_DATE)?.[0] ??
		decision.theirs?.match(ISO_DATE)?.[0] ??
		null
	);
}

/**
 * The classification as words: the backend's snake_case names (`both_changed`)
 * and a serialised enum name (`BothChanged`) both become `both changed`; a
 * phrase stays as it is.
 */
export function classLabel(classification: string): string {
	if (classification.includes("_")) return classification.replace(/_/g, " ");
	if (!/^[A-Za-z]+$/.test(classification)) return classification;
	return classification.replace(/([a-z0-9])([A-Z])/g, "$1 $2").toLowerCase();
}

/**
 * True when the merge left the directive exactly as both sides had it. The
 * panel shows these with the settled directives, not as work: `summary` already
 * counts them apart from `auto`.
 */
export function isUnchanged(decision: DecisionDto): boolean {
	return classLabel(decision.class) === "unchanged";
}

/** Candidates by the client whose naming produced them, in a fixed order. */
export type ClientGroup = { client: SyncClient; candidates: Candidate[] };

const CLIENT_ORDER: SyncClient[] = [
	"drive",
	"dropbox",
	"one_drive",
	"nextcloud",
	"syncthing",
	"seafile",
	"icloud",
];

export function byClient(candidates: Candidate[]): ClientGroup[] {
	const groups = new Map<SyncClient, Candidate[]>();
	for (const candidate of candidates) {
		const group = groups.get(candidate.client);
		if (group) group.push(candidate);
		else groups.set(candidate.client, [candidate]);
	}
	const ordered: ClientGroup[] = [];
	for (const client of CLIENT_ORDER) {
		const group = groups.get(client);
		if (!group) continue;
		ordered.push({ client, candidates: group });
		groups.delete(client);
	}
	// Nothing is dropped if a future client has no place in the order yet.
	for (const [client, rest] of groups) ordered.push({ client, candidates: rest });
	return ordered;
}
