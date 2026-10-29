// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The assistant's wire, as `src-tauri/src/ai.rs` defines it.
 *
 * The `ai_*` commands are not in `src/bindings.ts` yet: the command layer that
 * registers them is being added separately. Every call therefore goes through
 * `invoke` by its Tauri name, and this module is the only place that knows
 * that — once the bindings are regenerated, only these five wrappers change.
 *
 * Multi-word arguments are camelCased exactly as the generated bindings spell
 * them (`csvText` for a Rust `csv_text`), because Tauri renames command
 * arguments that way. The key never leaves the backend: `AiSettings` carries
 * `has_key`, never the key itself.
 */
import { invoke } from "@tauri-apps/api/core";

/** The frontend's view of the endpoint configuration — no secret in it. */
export type AiSettings = {
	base_url: string;
	model: string;
	max_tokens: number | null;
	temperature: number | null;
	has_key: boolean;
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
