// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Which list the sidebar shows below its navigation: the account tree or the
 * journal's files. They used to stack, one under the other, which is what the
 * tabs replace.
 */
export type SidebarTab = "accounts" | "files";

/** The two tabs, in the order the tab row shows them. */
export const SIDEBAR_TABS = [
	{ id: "accounts", label: "Accounts" },
	{ id: "files", label: "Files" },
] as const satisfies readonly { id: SidebarTab; label: string }[];

/** localStorage key holding the last chosen tab. */
const SIDEBAR_TAB_STORAGE_KEY = "sidebar_tab";

// SPA mode (`ssr = false`), so localStorage exists when this module loads.
const stored =
	typeof localStorage === "undefined" ? null : localStorage.getItem(SIDEBAR_TAB_STORAGE_KEY);

/**
 * The selected tab: module state so it survives every navigation (the sidebar
 * itself never unmounts, but the mobile sheet does) and persisted so a fresh
 * launch opens on the tab the reader last used.
 */
export const sidebarTab = $state<{ value: SidebarTab }>({
	value: stored === "files" ? "files" : "accounts",
});

/** Selects a tab and remembers it across reloads. */
export function selectSidebarTab(tab: SidebarTab) {
	sidebarTab.value = tab;
	localStorage.setItem(SIDEBAR_TAB_STORAGE_KEY, tab);
}
