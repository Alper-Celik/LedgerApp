// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { dashboards } from "$lib/dashboards.js";
import type { PageLoad } from "./$types";

/**
 * The registry, cut down to what a card can hold: an id, a title, a blurb and
 * an icon cross a load boundary — a Svelte component does not. The page behind
 * a card looks its entry up in the registry again when it renders it.
 */
export const load: PageLoad = () => ({
	dashboards: dashboards.map(({ id, title, blurb, icon }) => ({ id, title, blurb, icon })),
});
