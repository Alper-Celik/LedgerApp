// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { defineConfig } from "@playwright/test";

/**
 * End-to-end tests drive the real app in a real browser against a stubbed Tauri
 * IPC (see `e2e/stub.ts`), so they need neither the desktop shell nor a private
 * ledger: every fixture lives in this repository.
 *
 * Chromium: Playwright's own download is not used — this machine runs NixOS on
 * arm64, where a prebuilt browser does not link. Point `CHROMIUM_PATH` at any
 * Chromium-family binary (default: the system one) and export `LD_LIBRARY_PATH`
 * if that binary needs it, which is inherited by the launched browser.
 */
export default defineConfig({
	testDir: "e2e",
	timeout: 30_000,
	expect: { timeout: 5_000 },
	reporter: [["list"]],
	use: {
		baseURL: "http://127.0.0.1:5199",
		viewport: { width: 1200, height: 800 },
		launchOptions: {
			executablePath: process.env.CHROMIUM_PATH ?? "/run/current-system/sw/bin/chromium",
			args: ["--no-sandbox", "--disable-gpu"],
		},
	},
	webServer: {
		command: "npx vite dev --port 5199 --strictPort",
		port: 5199,
		reuseExistingServer: true,
		timeout: 60_000,
	},
});
