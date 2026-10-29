// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "./gate";
import { invokes, stubIpc } from "./stub";

test("a proposed write is shown, and only the click writes it", async ({ page }) => {
	const entry = {
		file: "/ledger/main.bean",
		date: "2026-01-12",
		flag: "*",
		payee: null,
		narration: "Coffee",
		postings: [
			{ account: "Assets:Bank:Checking", units: "-42.50", currency: "TRY" },
			{ account: "Expenses:Food:Groceries", units: "42.50", currency: "TRY" },
		],
	};

	await stubIpc(page);
	await page.addInitScript(
		(fixture) => {
			let turn = 0;
			const internals = (window as any).__TAURI_INTERNALS__;
			const original = internals.invoke;
			internals.invoke = (cmd: string, args: unknown) => {
				if (cmd === "ai_settings") {
					return Promise.resolve({
						base_url: "https://example.invalid/v1",
						model: "test",
						max_tokens: null,
						temperature: null,
						has_key: true,
						system_prompt: "You are the assistant.",
					});
				}
				if (cmd === "ai_tools") return Promise.resolve([]);
				if (cmd === "conversations") return Promise.resolve({ sessions: [], errors: [] });
				if (cmd === "conversation_append") return Promise.resolve({});
				if (cmd === "ai_stream" || cmd === "ai_chat") {
					turn += 1;
					if (turn === 1) {
						return Promise.resolve({
							text: "",
							tool_calls: [
								{ id: "call-1", name: "add_transaction", arguments: JSON.stringify(fixture) },
							],
							usage: null,
							finish_reason: "tool_calls",
							cancelled: false,
						});
					}
					return Promise.resolve({
						text: "Proposed; nothing written yet.",
						tool_calls: [],
						usage: null,
						finish_reason: "stop",
						cancelled: false,
					});
				}
				return original(cmd, args);
			};
		},
		entry
	);

	await page.goto("/assistant");
	await page.getByRole("textbox").first().fill("add a coffee expense");
	await page.keyboard.press("Enter");

	// The proposal names what would be written, and says nothing has been.
	const apply = page.getByRole("button", { name: "Apply" });
	await expect(apply).toBeVisible();
	// The card names the entry it would write: header line, then the postings.
	await expect(page.getByText(/2026-01-12 \* "Coffee"/)).toBeVisible();
	await expect(page.getByRole("cell", { name: "Assets:Bank:Checking" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "-42.50" })).toBeVisible();
	await expect(page.getByText("Nothing is written until you apply this.")).toBeVisible();

	// Nothing was written before the click.
	const before = await invokes(page);
	expect(before.filter((call) => call.cmd === "add_transaction")).toHaveLength(0);

	await apply.click();
	await expect
		.poll(async () => (await invokes(page)).filter((call) => call.cmd === "add_transaction").length)
		.toBe(1);
});
