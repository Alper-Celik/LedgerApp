// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The assistant's state: the endpoint configuration, the model and tool
 * catalogues, the conversation on screen, and the tool loop.
 *
 * One conversation is live at a time. Every message is appended to the ledger's
 * own store as it is produced (`.ledger/sessions/`, through the existing
 * `conversation_*` commands), so switching away or closing the app loses
 * nothing. A turn streams: each round's prose is appended to a live bubble as
 * its deltas arrive, the tool calls run when the round ends, and at most
 * [`MAX_ROUNDS`] tool rounds are run before the panel stops and says so. The
 * final answer, not the deltas, is what the bubble and the store keep — an
 * endpoint may spell a fragment differently than the turn it ends.
 *
 * The wire history is not persisted as such. The conversation store has no
 * field for an assistant's `tool_calls` (its `data` is only read back for tool
 * messages), so each tool message carries its own call id, name and arguments;
 * [`adopt`] rebuilds a replayable assistant-call/tool-answer pair from that.
 * Answers are never invented here: `text` comes from the endpoint, and every
 * ledger number in it comes from [`dispatchTool`].
 */
import {
	commands,
	type Conversation,
	type MalformedFile,
	type Message as StoredMessage,
	type Summary,
} from "../../bindings";
import { messageOf } from "../errors.js";
import {
	aiConfigure,
	aiModels,
	aiSettings,
	aiStream,
	aiTools,
	type AiSettings,
	type ChatMessage,
	type ChatResponse,
	type MediaPart,
	type ModelInfo,
	type ToolCall,
	type ToolSchema,
} from "./api.js";
import { mediaPartsOf, visionOf, type Attachment } from "./media.js";
import { dispatchTool, offeredTools } from "./tools.js";

/** How many tool rounds one turn may run before the panel stops. */
export const MAX_ROUNDS = 6;

/** One message as the transcript draws it. */
export type Entry =
	| { kind: "user"; text: string }
	| { kind: "assistant"; text: string }
	| { kind: "tool"; name: string; result: unknown; text: string; ok: boolean };

export class Assistant {
	settings = $state<AiSettings | null>(null);
	settingsError = $state("");
	/** True while the first-run form and its model picker hold the panel. */
	setupOpen = $state(false);

	models = $state<ModelInfo[]>([]);
	modelsError = $state("");
	modelsLoading = $state(false);

	tools = $state<ToolSchema[]>([]);
	toolsError = $state("");

	sessions = $state<Summary[]>([]);
	malformed = $state<MalformedFile[]>([]);
	sessionsError = $state("");

	activeId = $state("");
	activeTitle = $state("");
	entries = $state<Entry[]>([]);

	busy = $state(false);
	busyNote = $state("");
	/** Bumped for every streamed fragment, so the transcript follows the text as it lands. */
	streamTick = $state(0);
	opening = $state(false);
	loading = $state(true);

	chatError = $state("");
	saveError = $state("");
	notice = $state("");

	/** The wire history for the live turn; rebuilt whenever a conversation is opened. */
	private wire: ChatMessage[] = [];
	private startPromise: Promise<void> | null = null;

	/** Load once per app session, then just re-read the conversation list. */
	init(): Promise<void> {
		if (this.startPromise) return this.refreshSessions();
		this.startPromise = this.bootstrap();
		return this.startPromise;
	}

	private async bootstrap(): Promise<void> {
		this.loading = true;
		try {
			await this.loadSettings();
			await this.refreshSessions();
			// The tool schemas decide whether an answer can read the ledger at
			// all, so the transcript waits for them. The model list only feeds
			// the settings picker and arrives in the background.
			if (this.settings?.has_key) await this.loadTools();
		} finally {
			this.loading = false;
		}
		if (this.settings?.has_key) void this.loadModels();
	}

	/** The settings error page's retry: read the configuration and its catalogues. */
	async retrySettings(): Promise<void> {
		await this.loadSettings();
		if (!this.settings?.has_key) return;
		await this.loadTools();
		void this.loadModels();
	}

	async loadSettings(): Promise<void> {
		try {
			const settings = await aiSettings();
			this.settings = settings;
			this.settingsError = "";
			if (!settings.has_key) this.setupOpen = true;
		} catch (cause) {
			this.settingsError = messageOf(cause);
		}
	}

	/**
	 * Save the endpoint configuration. The key is only sent when the user typed
	 * one; `null` leaves the backend's stored secret alone, so the browser never
	 * has to hold it.
	 */
	async configure(
		baseUrl: string,
		apiKey: string | null,
		model: string,
		maxTokens: number | null,
		temperature: number | null
	): Promise<boolean> {
		const wasSetup = this.setupOpen || this.settings?.has_key === false;
		this.settingsError = "";
		try {
			const settings = await aiConfigure(baseUrl, apiKey, model, maxTokens, temperature);
			this.settings = settings;
			if (settings.has_key && wasSetup) this.setupOpen = true;
			if (settings.has_key) {
				await this.loadTools();
				void this.loadModels();
			}
			return true;
		} catch (cause) {
			this.settingsError = messageOf(cause);
			return false;
		}
	}

	/** Keep the stored settings and switch the model; the list is already loaded. */
	async selectModel(model: string): Promise<boolean> {
		const settings = this.settings;
		if (!settings) return false;
		this.settingsError = "";
		try {
			this.settings = await aiConfigure(
				settings.base_url,
				null,
				model,
				settings.max_tokens,
				settings.temperature
			);
			return true;
		} catch (cause) {
			this.settingsError = messageOf(cause);
			return false;
		}
	}

	/** Leave first-run setup; the transcript takes over. */
	finishSetup(): void {
		this.setupOpen = false;
	}

	async loadModels(): Promise<void> {
		this.modelsLoading = true;
		try {
			this.models = await aiModels();
			this.modelsError = "";
		} catch (cause) {
			this.modelsError = messageOf(cause);
		} finally {
			this.modelsLoading = false;
		}
	}

	async loadTools(): Promise<void> {
		try {
			this.tools = offeredTools(await aiTools());
			this.toolsError = "";
		} catch (cause) {
			// The backend's catalogue failing must not hide the sandbox: the panel
			// offers `run_python` itself, and its reads fail on their own terms.
			this.tools = offeredTools([]);
			this.toolsError = messageOf(cause);
		}
	}

	/** The ledger's conversations, newest first; a list failure keeps the open one. */
	async refreshSessions(): Promise<void> {
		try {
			const result = await commands.conversations();
			if (result.status === "error") {
				this.sessionsError = result.error;
				return;
			}
			this.sessions = result.data.sessions;
			this.malformed = result.data.errors;
			this.sessionsError = "";
			if (this.activeId && !this.sessions.some((session) => session.id === this.activeId)) {
				this.newConversation();
			}
		} catch (cause) {
			this.sessionsError = messageOf(cause);
		}
	}

	/** A blank conversation; the file is created by the first message. */
	newConversation(): void {
		this.activeId = "";
		this.activeTitle = "";
		this.entries = [];
		this.wire = [];
		this.chatError = "";
		this.saveError = "";
		this.notice = "";
	}

	async openConversation(id: string): Promise<void> {
		if (this.busy) return;
		this.opening = true;
		this.chatError = "";
		try {
			const result = await commands.conversation(id);
			if (result.status === "error") {
				this.sessionsError = result.error;
				return;
			}
			this.adopt(result.data);
		} catch (cause) {
			this.sessionsError = messageOf(cause);
		} finally {
			this.opening = false;
		}
	}

	async renameConversation(id: string, title: string): Promise<boolean> {
		const next = title.trim();
		if (!next) return false;
		this.sessionsError = "";
		try {
			const result = await commands.conversationRename(id, next);
			if (result.status === "error") {
				this.sessionsError = result.error;
				return false;
			}
			this.mergeSummary(result.data);
			if (this.activeId === id) this.activeTitle = result.data.title;
			return true;
		} catch (cause) {
			this.sessionsError = messageOf(cause);
			return false;
		}
	}

	async deleteConversation(id: string): Promise<boolean> {
		this.sessionsError = "";
		try {
			const result = await commands.conversationDelete(id);
			if (result.status === "error") {
				this.sessionsError = result.error;
				return false;
			}
			this.sessions = this.sessions.filter((session) => session.id !== id);
			if (this.activeId === id) this.newConversation();
			return true;
		} catch (cause) {
			this.sessionsError = messageOf(cause);
			return false;
		}
	}

	/**
	 * Ask one turn: persist the user message, stream the model's answer, run
	 * every tool it asks for, and call again until it answers in prose or
	 * [`MAX_ROUNDS`] is spent.
	 *
	 * Returns whether the turn started; false means nothing was sent (busy, no
	 * key, no text and nothing sendable attached).
	 */
	async send(text: string, attachments: Attachment[]): Promise<boolean> {
		const settings = this.settings;
		if (!settings?.has_key || this.busy || this.opening) return false;

		const trimmed = text.trim();
		if (!trimmed && attachments.length === 0) return false;

		this.chatError = "";
		this.notice = "";

		this.busy = true;
		try {
			const prepared = await this.mediaFor(attachments, settings);
			this.notice = prepared.notice;
			if (!trimmed && prepared.media.length === 0) return false;

			const body = trimmed || "Please look at the attached file.";
			this.entries.push({ kind: "user", text: body });
			this.wire.push(userWire(body));
			await this.saveMessage({ role: "user", text: body, data: null }, titleFrom(body));
			await this.rounds(prepared.media);
		} finally {
			this.busy = false;
			this.busyNote = "";
		}
		return true;
	}

	/**
	 * The wire parts one turn's attachments contribute, and what could not be
	 * sent.
	 *
	 * Only a vision-capable model gets any of it — the endpoint's model list is
	 * the only voice on that. Images and CSVs go as the browser read them, while
	 * a PDF the user placed in the ledger is rasterised by the backend page by
	 * page (see `mediaPartsOf`), which is what the model can actually read. A
	 * refusal is handed back for the notice line, verbatim, never swallowed.
	 */
	private async mediaFor(
		attachments: Attachment[],
		settings: AiSettings
	): Promise<{ media: MediaPart[]; notice: string }> {
		if (attachments.length === 0) return { media: [], notice: "" };
		const vision = visionOf(this.models, settings.model);
		if (vision !== "yes") {
			const files = attachments.length === 1 ? "the attachment was" : "the attachments were";
			return {
				media: [],
				notice:
					vision === "no"
						? `The endpoint reports that ${settings.model || "this model"} does not accept images, so ${files} not sent.`
						: `The endpoint did not say whether ${settings.model || "this model"} accepts images, so ${files} not sent.`,
			};
		}

		const media: MediaPart[] = [];
		const errors: string[] = [];
		for (const attachment of attachments) {
			this.busyNote =
				attachment.mime === "application/pdf" && attachment.path?.trim()
					? `Rendering “${attachment.name}”…`
					: `Reading “${attachment.name}”…`;
			const answer = await mediaPartsOf(attachment);
			media.push(...answer.parts);
			if (answer.error) errors.push(answer.error);
		}
		return { media, notice: errors.join(" ") };
	}

	/** One model round, streamed, plus the tool rounds it asks for. */
	private async rounds(media: MediaPart[]): Promise<void> {
		let pendingMedia = media;

		for (let round = 0; round < MAX_ROUNDS; round++) {
			this.busyNote = "Thinking…";
			// The bubble the prose lands in, pushed before the call so the first
			// fragment has somewhere to go; a round that ends without prose takes
			// it away again.
			this.entries.push({ kind: "assistant", text: "" });
			const entry = this.entries[this.entries.length - 1];

			let response: ChatResponse;
			try {
				response = await aiStream(
					{
						messages: [
							systemWire(this.tools.length > 0, this.settings?.system_prompt ?? ""),
							...replayable(this.wire),
						],
						tools: $state.snapshot(this.tools),
						media: pendingMedia,
					},
					(chunk) => {
						if (entry.kind !== "assistant") return;
						if (entry.text === "") this.busyNote = "Answering…";
						entry.text += chunk;
						this.streamTick += 1;
					}
				);
			} catch (cause) {
				// What streamed stays on screen and the refusal is shown in the
				// backend's own words; the wire history never learned of the turn.
				if (!entry.text.trim()) this.dropEntry(entry);
				this.chatError = messageOf(cause);
				return;
			}
			pendingMedia = [];

			const answer = response.text ?? "";
			const calls = Array.isArray(response.tool_calls) ? response.tool_calls : [];

			if (!answer.trim() && !calls.length) {
				this.dropEntry(entry);
				this.notice = "The model returned no text and no tool calls.";
				return;
			}

			// The answer, not the deltas, is what the transcript keeps: an
			// endpoint may stream fragments in a different spelling than it ends.
			if (entry.kind === "assistant") entry.text = answer;
			this.wire.push({
				role: "assistant",
				text: answer,
				tool_calls: calls,
				tool_call_id: null,
			});
			// An assistant turn that only calls tools has no prose; the call
			// itself lives on each tool message, so there is nothing worth a
			// section of its own in the ledger's file — or in a bubble.
			if (answer.trim()) {
				await this.saveMessage({ role: "assistant", text: answer, data: null });
			} else {
				this.dropEntry(entry);
			}

			if (!calls.length) {
				if (response.finish_reason === "length") {
					this.notice = "The answer was cut off: the endpoint reported finish_reason `length`.";
				}
				return;
			}

			for (const call of calls) {
				this.busyNote = `Running \`${call.name}\`…`;
				const outcome = await dispatchTool(call);
				this.entries.push({
					kind: "tool",
					name: call.name,
					result: outcome.result,
					text: outcome.text,
					ok: outcome.ok,
				});
				this.wire.push({
					role: "tool",
					text: outcome.text,
					tool_calls: [],
					tool_call_id: call.id,
				});
				await this.saveMessage({
					role: "tool",
					text: outcome.text,
					data: {
						tool_call_id: call.id,
						name: call.name,
						arguments: call.arguments,
						result: outcome.result,
					},
				});
			}
		}

		this.notice = `Stopped after ${MAX_ROUNDS} tool rounds; send another message to continue.`;
	}

	/** Remove a placeholder bubble a turn never filled. */
	private dropEntry(entry: Entry): void {
		const at = this.entries.indexOf(entry);
		if (at >= 0) this.entries.splice(at, 1);
	}

	/** Append one message to the open conversation, creating it when this is the first. */
	private async saveMessage(message: StoredMessage, title?: string): Promise<void> {
		const model = this.settings?.model ?? "";
		const id = this.activeId;
		const conversationTitle =
			title ||
			this.activeTitle ||
			(message.role === "user" ? titleFrom(message.text) : "Conversation");
		try {
			const result = await commands.conversationAppend(
				id,
				conversationTitle,
				model,
				message
			);
			if (result.status === "error") {
				this.saveError = result.error;
				return;
			}
			if (!id) {
				this.activeId = result.data.id;
				this.activeTitle = result.data.title;
			}
			this.mergeSummary(result.data);
			this.saveError = "";
			this.sessionsError = "";
		} catch (cause) {
			this.saveError = messageOf(cause);
		}
	}

	private mergeSummary(summary: Summary): void {
		const rest = this.sessions.filter((session) => session.id !== summary.id);
		this.sessions = [summary, ...rest].sort((a, b) => b.updated.localeCompare(a.updated));
	}

	/**
	 * Show a stored conversation and rebuild the wire history.
	 *
	 * Assistant tool calls are not stored on the assistant message (the store
	 * only lifts a `data` block from tool messages), so each tool message's own
	 * payload supplies its call: an assistant call/tool answer pair is inserted
	 * before it. A tool message with no call id still draws, but stays out of the
	 * wire, where a tool answer without its call would be invalid.
	 */
	private adopt(conversation: Conversation): void {
		this.activeId = conversation.id;
		this.activeTitle = conversation.title;
		this.entries = [];
		this.wire = [];
		this.chatError = "";
		this.saveError = "";
		this.notice = "";

		for (const message of conversation.messages) {
			if (message.role === "user") {
				this.entries.push({ kind: "user", text: message.text });
				this.wire.push(userWire(message.text));
			} else if (message.role === "assistant") {
				this.entries.push({ kind: "assistant", text: message.text });
				if (message.text.trim()) {
					this.wire.push({
						role: "assistant",
						text: message.text,
						tool_calls: [],
						tool_call_id: null,
					});
				}
			} else {
				const stored = readToolMessage(message);
				this.entries.push({
					kind: "tool",
					name: stored.call.name,
					result: stored.result,
					text: message.text,
					ok: stored.ok,
				});
				if (stored.call.id) {
					this.wire.push({
						role: "assistant",
						text: "",
						tool_calls: [stored.call],
						tool_call_id: null,
					});
					this.wire.push({
						role: "tool",
						text: message.text,
						tool_calls: [],
						tool_call_id: stored.call.id,
					});
				}
			}
		}
	}
}

/**
 * The panel's single instance.
 *
 * It lives at module scope so walking away from `/assistant` and back keeps the
 * loaded conversation and settings warm; `init` still re-reads the conversation
 * list on every mount, because the ledger may have changed underneath.
 */
export const assistant = new Assistant();

/** The title the conversation gets from its first user message. */
export function titleFrom(text: string): string {
	const line = text.replace(/\s+/g, " ").trim();
	if (!line) return "New conversation";
	return line.length > 60 ? `${line.slice(0, 59)}…` : line;
}

function userWire(text: string): ChatMessage {
	return { role: "user", text, tool_calls: [], tool_call_id: null };
}

/**
 * The instructions in front of every request: use the tools, never invent a
 * ledger figure. The tool sentence changes when the backend offered none, so
 * the model is told to decline instead of guessing.
 */
function systemWire(hasTools: boolean, base = ""): ChatMessage {
	const ledger = hasTools
		? "Use the provided read-only tools for every ledger fact. Every amount, date, account and transaction must come from a tool result."
		: "No ledger tools are available in this session, so say plainly that you cannot read the ledger instead of guessing any figure.";
	return {
		role: "system",
		text: base.trim() === "" ? `You are the assistant inside a Beancount ledger app. ${ledger}` : `${base.trim()} ${ledger}`,
		tool_calls: [],
		tool_call_id: null,
	};
}

/**
 * Drop the parts of the history an endpoint would refuse: a tool answer whose
 * call was never announced, and a call whose answer never arrived (a turn cut
 * off between the two). Everything else is sent in order.
 */
function replayable(messages: ChatMessage[]): ChatMessage[] {
	const answered = new Set(
		messages
			.filter((message) => message.role === "tool" && message.tool_call_id)
			.map((message) => message.tool_call_id as string)
	);
	const announced = new Set<string>();
	for (const message of messages) {
		if (message.role !== "assistant") continue;
		for (const call of message.tool_calls) announced.add(call.id);
	}

	const out: ChatMessage[] = [];
	for (const message of messages) {
		if (message.role === "tool") {
			if (message.tool_call_id && announced.has(message.tool_call_id)) out.push(message);
			continue;
		}
		if (message.role === "assistant" && message.tool_calls.length) {
			out.push({
				...message,
				tool_calls: message.tool_calls.filter((call) => answered.has(call.id)),
			});
			continue;
		}
		out.push(message);
	}
	return out;
}

/** One stored tool message's call and parsed answer. */
function readToolMessage(message: StoredMessage): {
	call: ToolCall;
	result: unknown;
	ok: boolean;
} {
	const data =
		message.data && typeof message.data === "object" && !Array.isArray(message.data)
			? (message.data as Record<string, unknown>)
			: null;
	const payload =
		data && "result" in data
			? data.result
			: (message.data ?? message.text);
	return {
		call: {
			id: typeof data?.tool_call_id === "string" ? data.tool_call_id : "",
			name: typeof data?.name === "string" ? data.name : "tool",
			arguments: typeof data?.arguments === "string" ? data.arguments : "",
		},
		result: payload,
		ok: !(payload && typeof payload === "object" && "error" in payload),
	};
}
