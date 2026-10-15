// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/** What the source editor opens: a transaction's own lines, or its whole file. */
export type EditorScope = "entry" | "file";

/** The two scopes, in the order the register's toggle shows them. */
export const EDITOR_SCOPES = [
	{ id: "entry", label: "Entry", hint: "Edit the transaction's own lines" },
	{ id: "file", label: "File", hint: "Edit the whole .bean file, opened on the entry" },
] as const satisfies readonly { id: EditorScope; label: string; hint: string }[];

/** localStorage key holding the chosen scope. */
export const EDITOR_SCOPE_STORAGE_KEY = "editor_scope";

// SPA mode (`ssr = false`), so localStorage exists when this module loads.
const stored =
	typeof localStorage === "undefined" ? null : localStorage.getItem(EDITOR_SCOPE_STORAGE_KEY);

/**
 * The source editor's scope, as a setting rather than a per-entry choice:
 * `entry` hands the editor just the transaction's own lines, `file` hands it
 * the whole `.bean` the entry lives in and opens on the entry. Module state so
 * every register agrees and the next entry opened follows it; persisted so it
 * survives a reload.
 */
export const editorScope = $state<{ scope: EditorScope }>({
	scope: stored === "file" ? "file" : "entry",
});

/** Sets the scope and remembers it across reloads. */
export function setEditorScope(scope: EditorScope) {
	editorScope.scope = scope;
	localStorage.setItem(EDITOR_SCOPE_STORAGE_KEY, scope);
}
