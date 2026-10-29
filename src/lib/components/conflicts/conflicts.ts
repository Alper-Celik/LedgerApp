// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The conflict resolver's model: what the two resolver commands answer with,
 * and the naming the list and the panel share.
 *
 * `sync_conflicts` is a generated binding already; `conflict_compare` and
 * `conflict_resolve` are newer than the checked-in `bindings.ts`, so this module
 * talks to them by command name and gives their shapes types here. Once the
 * bindings are regenerated the two wrappers below can call
 * `commands.conflictCompare` / `commands.conflictResolve` instead — the shapes
 * do not change, and nothing else on the screen has to.
 */

import { invoke } from "@tauri-apps/api/core";
import type { Candidate, SyncClient } from "../../../bindings";
import { messageOf } from "$lib/errors.js";

/** What to keep for one directive the merge could not settle by itself. */
export type ConflictPick = "ours" | "theirs" | "both" | "drop";

/** The pick control's four answers, in reading order. */
export const CHOICES: readonly { id: ConflictPick; label: string; hint: string }[] = [
	{ id: "ours", label: "Ours", hint: "Keep the original file's version" },
	{ id: "theirs", label: "Theirs", hint: "Keep the conflicted copy's version" },
	{ id: "both", label: "Both", hint: "Keep both versions, ours first" },
	{ id: "drop", label: "Drop", hint: "Keep neither version" },
];

/** One directive, as the merge sees it. */
export type ConflictDecision = {
	/** Opaque identity string, stable across the compare and the resolve. */
	key: string;
	/** `transaction`, `open`, `balance`, … */
	kind: string;
	/** One-line human description, e.g. `2026-01-10 * "Groceries"`. */
	label: string;
	/** What happened across the three versions. */
	class: string;
	/** Settled by the merge, or in need of a [`ConflictPick`]. */
	needs_user: boolean;
	/** This directive exactly as the original has it. */
	ours: string | null;
	/** This directive exactly as the conflicted copy has it. */
	theirs: string | null;
	/** This directive exactly as the common base has it, when there is one. */
	base: string | null;
};

/** Counts for the resolver's header. */
export type ConflictSummary = {
	unchanged: number;
	auto: number;
	needs_user: number;
	ambiguous: number;
	total: number;
	/** Directives per kind, in kind-name order. */
	kinds: [string, number][];
};

/** One original and its conflicted copy, compared. */
export type ConflictComparison = {
	original: string;
	copy: string;
	/** The client's display name (`Google Drive`, …), or empty when unknown. */
	client: string;
	/** False when the log holds no common base for the file yet. */
	base_found: boolean;
	/** The whole original text. */
	ours: string;
	/** The whole copy's text. */
	theirs: string;
	summary: ConflictSummary;
	decisions: ConflictDecision[];
};

export type CompareResult =
	| { ok: true; comparison: ConflictComparison }
	| { ok: false; error: string };

export type ResolveResult = { ok: true; message: string } | { ok: false; error: string };

/**
 * Compare an original with its conflict copy: the whole texts, the header's
 * counts, and one decision per directive. The backend remembers the comparison
 * for the copy, so `resolveConflict` only works after this call, in this
 * process.
 */
export async function compareConflict(
	original: string,
	copy: string
): Promise<CompareResult> {
	try {
		return {
			ok: true,
			comparison: await invoke<ConflictComparison>("conflict_compare", { original, copy }),
		};
	} catch (cause) {
		return { ok: false, error: messageOf(cause) };
	}
}

/**
 * Apply the picks and write the merged file back over `original`. The answer is
 * the backend's own sentence on success; a refusal is the exception text,
 * verbatim.
 */
export async function resolveConflict(
	original: string,
	picks: [string, ConflictPick][]
): Promise<ResolveResult> {
	try {
		return { ok: true, message: await invoke<string>("conflict_resolve", { original, picks }) };
	} catch (cause) {
		return { ok: false, error: messageOf(cause) };
	}
}

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
export function dateOf(decision: ConflictDecision): string | null {
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
export function isUnchanged(decision: ConflictDecision): boolean {
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
