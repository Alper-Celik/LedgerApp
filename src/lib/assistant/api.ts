// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The assistant's wire, as `src-tauri/src/ai.rs` defines it.
 *
 * The `ai_*` commands are not in `src/bindings.ts` yet: the command layer that
 * registers them is being added separately. Every call therefore goes through
 * `invoke` by its Tauri name, and this module is the only place that knows
 * that — once the bindings are regenerated, only these wrappers change.
 *
 * Multi-word arguments are camelCased exactly as the generated bindings spell
 * them (`csvText` for a Rust `csv_text`), because Tauri renames command
 * arguments that way. The key never leaves the backend: `AiSettings` carries
 * `has_key`, never the key itself.
 *
 * [`aiStream`] is one turn delivered while it is written: the command forwards
 * `ai::StreamEvent`s as `ai://delta` events and still resolves with the whole
 * answer, which is what the tool loop consumes. A runtime with no event bridge
 * answers through [`aiChat`] instead, so the panel keeps working unchanged.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** The frontend's view of the endpoint configuration — no secret in it. */
export type AiSettings = {
	base_url: string;
	model: string;
	max_tokens: number | null;
	temperature: number | null;
	has_key: boolean;
	/** The instructions the backend defines; the panel sends them with each turn. */
	system_prompt: string;
};

/** One model the endpoint lists. `vision` is the endpoint's own answer, when it gives one. */
export type ModelInfo = {
	id: string;
	context_window: number | null;
	vision: boolean | null;
};

/** One tool the backend offers the model. */
export type ToolSchema = {
	name: string;
	description: string;
	parameters: unknown;
};

/** One call the model asked for. `arguments` stays the model's JSON text. */
export type ToolCall = {
	id: string;
	name: string;
	arguments: string;
};

export type ChatRole = "system" | "user" | "assistant" | "tool";

/** One turn of the request, in the shape the endpoint accepts. */
export type ChatMessage = {
	role: ChatRole;
	text: string;
	tool_calls: ToolCall[];
	tool_call_id: string | null;
};

/** One image (or other data part) attached to the newest user message. */
export type MediaPart = {
	mime: string;
	data_base64: string;
	detail: "auto" | "low" | "high" | null;
};

export type ChatRequest = {
	messages: ChatMessage[];
	tools: ToolSchema[];
	media: MediaPart[];
};

export type Usage = {
	prompt_tokens: number;
	completion_tokens: number;
	total_tokens: number;
};

export type ChatResponse = {
	text: string;
	tool_calls: ToolCall[];
	usage: Usage | null;
	finish_reason: string | null;
	cancelled?: boolean;
};

/** The configuration, or the defaults when none is saved yet. */
export function aiSettings(): Promise<AiSettings> {
	return invoke<AiSettings>("ai_settings");
}

/**
 * Save the configuration and answer the view of it.
 *
 * `apiKey` is `null` to keep the stored key untouched; `""` would clear it.
 * `maxTokens` and `temperature` are `null` for the endpoint's own defaults.
 */
export function aiConfigure(
	baseUrl: string,
	apiKey: string | null,
	model: string,
	maxTokens: number | null,
	temperature: number | null
): Promise<AiSettings> {
	return invoke<AiSettings>("ai_configure", { baseUrl, apiKey, model, maxTokens, temperature });
}

/** The endpoint's model list. Needs a reachable endpoint and, usually, a key. */
export function aiModels(): Promise<ModelInfo[]> {
	return invoke<ModelInfo[]>("ai_models");
}

/** The tool schemas the backend offers the model for this ledger. */
export function aiTools(): Promise<ToolSchema[]> {
	return invoke<ToolSchema[]>("ai_tools");
}

/** One turn, answered in full. Non-streaming: the turn arrives as one payload. */
export function aiChat(request: ChatRequest): Promise<ChatResponse> {
	return invoke<ChatResponse>("ai_chat", { request });
}

/** The event name the streaming command forwards its deltas on. */
export const STREAM_EVENT = "ai://delta";

/**
 * One delta of a streamed turn, as `ai::StreamEvent` serialises it: a tagged
 * object whose `kind` says what arrived. Prose comes in `text_delta`
 * fragments; the tool calls and the finish reason are complete in the
 * promise's answer, so the panel only reads `text` here.
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
 * over — the same shape [`aiChat`] answers, tool calls included.
 *
 * The subscription is made before the command is invoked: the stream may emit
 * while the invoke is still in flight, and an event fired before the listener
 * exists would be lost. The listener is removed however the turn ends.
 *
 * A runtime with no event bridge (a browser without Tauri's internals) fails
 * the subscription rather than the turn; that is the one failure the
 * non-streaming call exists for, so it is answered by [`aiChat`] and the panel
 * behaves as it did before streaming. A listener that fails *after* the turn
 * started is the turn's own error and is passed through.
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
		return aiChat(request);
	}
	try {
		return await invoke<ChatResponse>("ai_stream", { request });
	} finally {
		unlisten?.();
	}
}
