// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The two halves of the register's page: the account's own register, and the journal's files. */
export type BrowserTab = "register" | "files";

/** How the file the explorer has open is shown: its transactions, or its text. */
export type FileMode = "transactions" | "text";

/** localStorage key holding the chosen tab. */
export const BROWSER_TAB_STORAGE_KEY = "browser_tab";

// SPA mode (`ssr = false`), so localStorage exists when this module loads.
const stored =
	typeof localStorage === "undefined" ? null : localStorage.getItem(BROWSER_TAB_STORAGE_KEY);

/**
 * What the browser is showing: which tab, and — for the file tab — which journal
 * file is open and whether it is read as transactions or as text. Module state,
 * the way the filters are, so a tab and an open file survive moving between an
 * account and the whole ledger; the tab is remembered across reloads too.
 *
 * `path` and `mode` are plain fields a view assigns to (the filters are bound
 * the same way); the two transitions worth naming are below.
 */
export const browser = $state({
	tab: (stored === "files" ? "files" : "register") as BrowserTab,
	/** The path of the open journal file; `null` is the explorer's list. */
	path: null as string | null,
	mode: "transactions" as FileMode,
});

/** Switch tabs, remembering which one the app opens on next time. */
export function setBrowserTab(tab: BrowserTab) {
	browser.tab = tab;
	localStorage.setItem(BROWSER_TAB_STORAGE_KEY, tab);
}

/** Open a journal file, on its transactions — the shape the register shows. */
export function openBrowserFile(path: string) {
	browser.path = path;
	browser.mode = "transactions";
}
