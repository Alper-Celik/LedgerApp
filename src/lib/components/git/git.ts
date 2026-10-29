// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The naming the branches screen, the merge panel and the conflict resolver
 * share: git's answers as sentences, the commit-time and count formatting, and
 * the path helpers the route's load and the panels both need to line a
 * repository-relative conflict path up with the journal's absolute files.
 *
 * Nothing here touches a store or a command: the route's `load` imports these,
 * and a universal load should not drag the ledger's reactive store in with it.
 */

/**
 * The paths of the ledger's changed files, taken from `ledger_diff`'s
 * `=== path ===` blocks. That command limits its diff to the journal's own
 * files, so this is the ledger's working tree, not the repository's.
 */
export function changedPaths(diff: string): string[] {
	const paths: string[] = [];
	const seen = new Set<string>();
	for (const line of diff.split("\n")) {
		const match = /^=== (.+) ===$/.exec(line);
		const path = match?.[1]?.trim();
		if (!path || seen.has(path)) continue;
		seen.add(path);
		paths.push(path);
	}
	return paths;
}

/**
 * `path` relative to the repository root, for matching a journal file's
 * absolute path against the repository-relative paths git reports. A path that
 * is not under the root is returned as it stands.
 */
export function repoRelative(root: string | null, path: string): string {
	if (!root) return path;
	const base = root.endsWith("/") ? root : `${root}/`;
	return path.startsWith(base) ? path.slice(base.length) : path;
}

/** One conflicted path's kind, said the way the merge left it. */
export function kindLabel(kind: string): string {
	switch (kind) {
		case "both_modified":
			return "both sides changed it";
		case "both_added":
			return "both sides added it";
		case "deleted_by_us":
			return "deleted on this branch";
		case "deleted_by_them":
			return "deleted on the merged branch";
		case "added_by_us":
			return "added on this branch";
		case "added_by_them":
			return "added on the merged branch";
		case "deleted_by_both":
			return "deleted on both sides";
		case "unclassified":
			return "unclassified conflict";
		default:
			return kind.replaceAll("_", " ");
	}
}

/**
 * A commit's author time as local text. The backend sends seconds since the
 * epoch (a double, because a JS number cannot carry a 64-bit integer).
 */
export function commitTime(seconds: number | null): string {
	if (seconds === null) return "unknown time";
	const at = new Date(seconds * 1000);
	if (Number.isNaN(at.getTime())) return "unknown time";
	return at.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** A full object id shortened the way the log's own rows shorten it. */
export function shortId(id: string): string {
	return id.slice(0, 7);
}

/** `3 files`, `1 file`. */
export function fileCount(count: number): string {
	return `${count} ${count === 1 ? "file" : "files"}`;
}

