// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * `ledger.py` is imported as text — it is compiled inside the Python sandbox,
 * not run by the bundler — so Vite's `?raw` query needs a type. This is the
 * only file the panel imports that way.
 */
declare module "*.py?raw" {
	const source: string;
	export default source;
}
