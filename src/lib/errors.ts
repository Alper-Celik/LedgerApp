// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * A thrown value as the text to show: an `Error`'s message (a Tauri command's
 * own error, or a `catch` around the IPC), otherwise its string form. Every
 * catch in the app reports through this, so a failure reads the same in a
 * route's alert, the folder picker and the source editor.
 */
export function messageOf(cause: unknown): string {
	return cause instanceof Error ? cause.message : String(cause);
}
