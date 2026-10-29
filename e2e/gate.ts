// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { test as base } from "@playwright/test";
import { unknownCommands } from "./stub";

export { expect } from "@playwright/test";

/**
 * The suite's `test`, with one automatic fixture bolted on: when the test body
 * ends, the stub must not have answered any command by falling through to its
 * permissive default. That default is deliberate — an unlisted *read* should
 * not fail a test — but silently serving a command nobody stubbed is exactly
 * how a command the app calls stops existing on the Rust side without a single
 * test noticing. So the gap is recorded by the stub (see `stubIpc`) and this
 * fixture turns it into a failed test, naming the commits to make.
 *
 * Specs import `test` from here, not from `@playwright/test`.
 */
export const test = base.extend<{ stubAnsweredEveryCommand: void }>({
	stubAnsweredEveryCommand: [
		async ({ page }, use) => {
			await use();
			// A closed page cannot report; a test that closes its own page has
			// already decided what it is asserting.
			if (page.isClosed()) return;
			const unknown = await unknownCommands(page);
			if (unknown.length === 0) return;
			throw new Error(
				`the app called ${unknown.length} command(s) that e2e/stub.ts does not stub: ` +
					`${unknown.join(", ")}. They were answered with the permissive ` +
					`{status:"ok",data:null} default, so whatever they feed is untested — ` +
					`add a case for each to e2e/stub.ts (or stop calling them).`
			);
		},
		{ auto: true },
	],
});
