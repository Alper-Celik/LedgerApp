// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { redirect } from "@sveltejs/kit";
import type { PageLoad } from "./$types";

/**
 * The window opens at `/` (Tauri's URL has no path), so the root is nothing but
 * the way in: the whole ledger is what the app has to show.
 */
export const load: PageLoad = () => redirect(307, "/ledger");
