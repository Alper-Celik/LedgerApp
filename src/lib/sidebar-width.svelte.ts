// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Width range the sidebar can be resized to, in px. */
const SIDEBAR_MIN_WIDTH = 192;
const SIDEBAR_MAX_WIDTH = 448;
export const SIDEBAR_DEFAULT_WIDTH = 256;

/** localStorage key holding the last chosen width. */
const SIDEBAR_WIDTH_STORAGE_KEY = "sidebar_width";

// SPA mode (`ssr = false`), so localStorage exists when this module loads.
const stored =
	typeof localStorage === "undefined" ? NaN : Number(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY));

function clamp(px: number): number {
	return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(px)));
}

/**
 * Sidebar width in px; `+layout.svelte` feeds it into the sidebar's
 * `--sidebar-width` custom property, so every part of the sidebar (the panel,
 * the desktop gap it reserves, the inset offset) follows the drag.
 */
export const sidebarWidth = $state({
	px: Number.isFinite(stored) && stored > 0 ? clamp(stored) : SIDEBAR_DEFAULT_WIDTH,
});

/**
 * Sets the width, clamped to `SIDEBAR_MIN_WIDTH`..`SIDEBAR_MAX_WIDTH`, and
 * remembers it across reloads — the drag and Settings' reset both land here.
 */
export function setSidebarWidth(px: number) {
	sidebarWidth.px = clamp(px);
	localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(sidebarWidth.px));
}
