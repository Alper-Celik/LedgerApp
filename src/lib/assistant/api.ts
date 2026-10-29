// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The assistant's streamed turn.
 *
 * Every `ai_*` command, and every shape they speak, is a generated binding in
 * `src/bindings.ts` (`commands.aiSettings`, `commands.aiChat`, `ChatRequest`, …),
 * so this module keeps only the one thing the bindings cannot express: the
 * `ai://delta` subscription that turns `ai_stream`'s deltas into the `onText`
 * callback while the command's own answer — the assembled turn — comes back
 * from the same call.
 *
 * A runtime with no event bridge (a browser without Tauri's internals) fails
 * the subscription rather than the turn; that is the one failure the
 * non-streaming call exists for, so it is answered by `commands.aiChat` and the
 * panel behaves as it did before streaming.
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands, type ChatRequest, type ChatResponse } from "../../bindings";

/** The event name the streaming command forwards its deltas on. */
export const STREAM_EVENT = "ai://delta";

/**
 * One delta of a streamed turn, as `ai::StreamEvent` serialises it: a tagged
 * object whose `kind` says what arrived. Prose comes in `text_delta`
 * fragments; the tool calls and the finish reason are complete in the
 * promise's answer, so the panel only reads `text` here.
 *
 * Not a generated type: `ai_stream` reports it through an event rather than
 * through its return value, so `specta` never sees it.
 */
export type StreamEvent =
	| { kind: "text_delta"; text: string }
	| {
			kind: "tool_call_delta";
			index: number;
			id: string | null;
			name: string | null;
			arguments: string;
	  }
	| {
			kind: "usage";
			prompt_tokens: number;
			completion_tokens: number;
			total_tokens: number;
	  }
	| { kind: "done"; finish_reason: string | null };

/**
 * One turn, streamed: `onText` is called with each prose fragment as it is
 * written, and the promise resolves with the whole answer once the stream is
 * over — tool calls included.
 *
 * The subscription is made before the command is invoked: the stream may emit
 * while the invoke is still in flight, and an event fired before the listener
 * exists would be lost. The listener is removed however the turn ends.
 *
 * A listener that fails *after* the turn started is the turn's own error and is
 * passed through. A command refusal rejects with the backend's sentence, so the
 * panel reports it the same way it reports a non-streamed failure.
 */
export async function aiStream(
	request: ChatRequest,
	onText: (text: string) => void
): Promise<ChatResponse> {
	let unlisten: UnlistenFn | null = null;
	try {
		unlisten = await listen<StreamEvent>(STREAM_EVENT, (event) => {
			const delta = event.payload;
			if (delta && delta.kind === "text_delta" && typeof delta.text === "string") {
				onText(delta.text);
			}
		});
	} catch (cause) {
		console.warn("ai://delta is not available here; answering in one turn", cause);
		const answer = await commands.aiChat(request);
		if (answer.status === "error") throw new Error(answer.error);
		return answer.data;
	}
	try {
		const answer = await commands.aiStream(request);
		if (answer.status === "error") throw new Error(answer.error);
		return answer.data;
	} finally {
		unlisten?.();
	}
}
