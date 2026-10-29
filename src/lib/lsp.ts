/*
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
*/
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type * as Monaco from "monaco-editor";
import { commands } from "../bindings";
import { BEANCOUNT_LANGUAGE } from "$lib/beancount.js";

/**
 * The language server embedded in the app, as the editor sees it.
 *
 * Rust owns the server (`src-tauri/src/lsp.rs`): it answers `initialize`,
 * loads the journal, and runs `rustledger-lsp`'s main loop over channels. This
 * module is only the client half — JSON-RPC messages go out through
 * `lsp_send`, come back as `lsp-message` events, and are turned into Monaco
 * features here:
 *
 * - completion (accounts, currencies, payees, tags, links, directives, dates),
 * - hover (an account's `open` directive, a currency's occurrences, a
 *   directive keyword's meaning),
 * - diagnostics (parse and validation errors, as markers).
 *
 * Positions cross this boundary as UTF-8 byte offsets, which is what the
 * server's handlers emit and what `initialize` negotiates; Monaco's columns
 * are UTF-16 code units, so every message is converted line by line.
 */

/** The event Rust emits each language-server message on. */
const MESSAGE_EVENT = "lsp-message";

/** Marker owner, so a detach clears only this editor's markers. */
const MARKER_OWNER = "rustledger";

type JsonRpcMessage = {
	id?: number | string;
	method?: string;
	result?: unknown;
	error?: { code: number; message: string };
	params?: unknown;
};

type LspPosition = { line: number; character: number };
type LspRange = { start: LspPosition; end: LspPosition };

type CompletionItem = {
	label: string;
	kind?: number;
	detail?: string;
	documentation?: unknown;
	insertText?: string;
	filterText?: string;
	sortText?: string;
};

type Diagnostic = {
	range: LspRange;
	severity?: number;
	message: string;
	code?: string | number;
	source?: string;
};

type Pending = {
	resolve: (value: unknown) => void;
	reject: (reason: Error) => void;
	timer: ReturnType<typeof setTimeout>;
};

type Document = {
	model: Monaco.editor.ITextModel;
	uri: string;
	version: number;
	change: Monaco.IDisposable;
	/** Whether the server's diagnostics are shown for this document. A
	 * transaction fragment is not a ledger file, so its standalone validation
	 * would be noise; completion and hover still apply. */
	diagnostics: boolean;
	/** Set on each edit; the change goes out once typing pauses. */
	pending: boolean;
	timer?: ReturnType<typeof setTimeout>;
};

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** An LSP position (UTF-8) as a Monaco position (UTF-16, line/column 1-based). */
function positionFromLsp(
	model: Monaco.editor.ITextModel,
	position: LspPosition,
): Monaco.IPosition {
	const lineNumber = Math.min(Math.max(position.line + 1, 1), model.getLineCount());
	const line = model.getLineContent(lineNumber);
	return {
		lineNumber,
		column: decoder.decode(encoder.encode(line).slice(0, position.character)).length + 1,
	};
}

/** A Monaco position as the LSP position the server expects. */
function positionToLsp(
	model: Monaco.editor.ITextModel,
	position: Monaco.IPosition,
): LspPosition {
	const line = model.getLineContent(position.lineNumber);
	return {
		line: position.lineNumber - 1,
		character: encoder.encode(line.slice(0, position.column - 1)).length,
	};
}

function rangeFromLsp(model: Monaco.editor.ITextModel, range: LspRange): Monaco.IRange {
	const start = positionFromLsp(model, range.start);
	const end = positionFromLsp(model, range.end);
	return {
		startLineNumber: start.lineNumber,
		startColumn: start.column,
		endLineNumber: end.lineNumber,
		endColumn: end.column,
	};
}

/**
 * A `file:` URI for a journal path. The path arrives absolute from
 * `lsp_file_path`; each segment is percent-encoded, so spaces and non-ASCII
 * names survive the URL parse on the Rust side.
 */
function fileUri(path: string): string {
	const absolute = path.replaceAll("\\", "/");
	const rooted = absolute.startsWith("/") ? absolute : `/${absolute}`;
	return `file://${rooted
		.split("/")
		.map((segment) => encodeURIComponent(segment))
		.join("/")}`;
}

/** LSP `CompletionItemKind` → Monaco's enum (the two numberings differ). */
function completionKind(kind: number | undefined): Monaco.languages.CompletionItemKind {
	const kinds = {
		1: 18, // Text
		2: 0, // Method
		3: 1, // Function
		4: 2, // Constructor
		5: 3, // Field
		6: 4, // Variable
		7: 5, // Class
		8: 7, // Interface
		9: 8, // Module
		10: 9, // Property
		11: 12, // Unit
		12: 13, // Value
		13: 15, // Enum
		14: 17, // Keyword
		15: 28, // Snippet
		16: 19, // Color
		17: 20, // File
		18: 21, // Reference
		19: 23, // Folder
		20: 16, // EnumMember
		21: 14, // Constant
		22: 6, // Struct
		23: 10, // Event
		24: 11, // Operator
		25: 24, // TypeParameter
	} as const;
	return kinds[kind as keyof typeof kinds] ?? 18;
}

/** LSP severity → Monaco marker severity. */
function markerSeverity(severity: number | undefined): Monaco.MarkerSeverity {
	switch (severity) {
		case 1:
			return 8; // Error
		case 2:
			return 4; // Warning
		case 3:
			return 2; // Info
		default:
			return 1; // Hint
	}
}

/** LSP documentation is a string or a `MarkupContent`. */
function documentationOf(value: unknown): string | Monaco.IMarkdownString | undefined {
	if (typeof value === "string") return value;
	if (value && typeof value === "object" && "value" in value) {
		const markup = value as { value?: unknown };
		if (typeof markup.value === "string") return { value: markup.value };
	}
	return undefined;
}

/** LSP hover contents — markup, a marked string, or an array of either. */
function hoverContents(contents: unknown): Monaco.IMarkdownString[] {
	if (typeof contents === "string") return [{ value: contents }];
	if (Array.isArray(contents)) return contents.flatMap(hoverContents);
	if (contents && typeof contents === "object") {
		const marked = contents as { value?: unknown; language?: unknown };
		if (typeof marked.value === "string") {
			const language = typeof marked.language === "string" ? marked.language : "";
			return [
				{
					value: language ? `\`\`\`${language}\n${marked.value}\n\`\`\`` : marked.value,
				},
			];
		}
	}
	return [];
}

class LanguageServer {
	private nextId = 1;
	private pending = new Map<number, Pending>();
	/** Attached documents, keyed by the Monaco model's URI. */
	private documents = new Map<string, Document>();
	/** The same documents, keyed by their `file:` URI. */
	private byUri = new Map<string, Set<Document>>();
	/** Sends are chained so a notification never overtakes the one before it. */
	private sending: Promise<unknown> = Promise.resolve();
	private unlisten: UnlistenFn | null = null;
	private starting: Promise<void> | null = null;
	/**
	 * Bumped by every shutdown. A start that is still in flight when it moves
	 * belongs to a journal that is no longer open, so it stops at its next await
	 * instead of installing a listener nobody can remove or starting the server
	 * for whatever is loaded now.
	 */
	private generation = 0;
	private monaco: typeof Monaco | null = null;
	private providersRegistered = false;

	/**
	 * Start the server once, register the Monaco providers, and open `model`
	 * as the document at `path`. For a whole file `path` is the journal file;
	 * for a single transaction it is a document key of its own, so the server
	 * never mistakes the fragment for the file's contents. The returned
	 * function closes the document and takes its markers away; the caller owns
	 * the model.
	 */
	async attach(
		monaco: typeof Monaco,
		model: Monaco.editor.ITextModel,
		path: string,
		diagnostics = true,
	): Promise<() => void> {
		this.monaco = monaco;
		this.registerProviders(monaco);
		await this.ensureStarted();

		const resolved = await commands.lspFilePath(path);
		if (resolved.status === "error") throw new Error(resolved.error);
		const uri = fileUri(resolved.data);
		const document: Document = {
			model,
			uri,
			version: 1,
			diagnostics,
			pending: false,
			change: model.onDidChangeContent(() => this.changed(document)),
		};
		this.documents.set(model.uri.toString(), document);
		const attached = this.byUri.get(uri) ?? new Set<Document>();
		const alreadyOpen = attached.size > 0;
		attached.add(document);
		this.byUri.set(uri, attached);

		if (alreadyOpen) {
			// Another model on the same file already opened it. The server holds one
			// buffer per URI, so a second `didOpen` would reset it (and both copies
			// would then send versions the server cannot order); the text this model
			// shows is what the server should have.
			this.notify("textDocument/didChange", {
				textDocument: { uri, version: document.version },
				contentChanges: [{ text: model.getValue() }],
			});
		} else {
			this.notify("textDocument/didOpen", {
				textDocument: {
					uri,
					languageId: BEANCOUNT_LANGUAGE,
					version: document.version,
					text: model.getValue(),
				},
			});
		}

		return () => this.detach(document);
	}

	private detach(document: Document): void {
		clearTimeout(document.timer);
		document.pending = false;
		const key = document.model.uri.toString();
		if (this.documents.get(key) === document) this.documents.delete(key);
		const attached = this.byUri.get(document.uri);
		attached?.delete(document);
		if (attached?.size === 0) this.byUri.delete(document.uri);
		document.change.dispose();
		this.monaco?.editor.setModelMarkers(document.model, MARKER_OWNER, []);
		// Closing the file for the *other* model still showing it would drop the
		// server's buffer and blank its markers; the last one out closes it.
		if (attached?.size) return;
		this.notify("textDocument/didClose", {
			textDocument: { uri: document.uri },
		});
	}

	private changed(document: Document): void {
		document.version += 1;
		document.pending = true;
		clearTimeout(document.timer);
		document.timer = setTimeout(() => this.flush(document), 150);
	}

	/**
	 * Send the document's latest text if an edit is waiting. A full-text sync
	 * per keystroke is a whole file over the IPC every key, so edits coalesce;
	 * a completion or hover request flushes first, because the server answers
	 * from the buffer it has.
	 */
	private flush(document: Document): void {
		clearTimeout(document.timer);
		document.timer = undefined;
		if (!document.pending) return;
		document.pending = false;
		this.notify("textDocument/didChange", {
			textDocument: { uri: document.uri, version: document.version },
			contentChanges: [{ text: document.model.getValue() }],
		});
	}

	/**
	 * Reopen every surviving document on a fresh server.
	 *
	 * `load_ledger` stops the backend session on every load — including the
	 * reload of the journal already open — and the editors stay mounted, so
	 * discarding the client would leave them attached to nothing: no completion,
	 * no hover, and markers frozen where they were. The documents are kept and
	 * reopened instead, and the markers are cleared because the server's opinion
	 * of them is gone.
	 */
	async restart(): Promise<void> {
		this.generation += 1;
		const unlisten = this.unlisten;
		this.unlisten = null;
		this.starting = null;
		await unlisten?.();
		for (const document of this.documents.values()) {
			clearTimeout(document.timer);
			document.pending = false;
			document.version += 1;
			this.monaco?.editor.setModelMarkers(document.model, MARKER_OWNER, []);
		}
		if (this.documents.size === 0) return;
		try {
			await this.ensureStarted();
		} catch (cause) {
			// A server that will not start is the editor's problem to show, and
			// `ensureStarted` has already forgotten the attempt so the next attach
			// can try again.
			console.error("language server restart failed", cause);
			return;
		}
		// One `didOpen` per URI: the server holds one buffer per file.
		const opened = new Set<string>();
		for (const document of this.documents.values()) {
			if (opened.has(document.uri)) continue;
			opened.add(document.uri);
			this.notify("textDocument/didOpen", {
				textDocument: {
					uri: document.uri,
					languageId: BEANCOUNT_LANGUAGE,
					version: document.version,
					text: document.model.getValue(),
				},
			});
		}
	}

	private ensureStarted(): Promise<void> {
		if (!this.starting) {
			this.starting = this.start().catch((cause) => {
				// A failed start must not poison the next editor: forgetting the
				// promise lets a later attach try again.
				this.starting = null;
				void this.unlisten?.();
				this.unlisten = null;
				throw cause;
			});
		}
		return this.starting;
	}

	private async start(): Promise<void> {
		const generation = this.generation;
		const unlisten = await listen<string>(MESSAGE_EVENT, (event) => {
			this.receive(event.payload);
		});
		if (generation !== this.generation) {
			// Overtaken by a journal switch: this listener belongs to nothing.
			void unlisten();
			return;
		}
		this.unlisten = unlisten;
		const started = await commands.lspStart();
		if (generation !== this.generation) return;
		if (started.status === "error") throw new Error(started.error);
		if (started.data) {
			await this.request("initialize", {
				processId: null,
				capabilities: {
					general: { positionEncodings: ["utf-8"] },
					textDocument: {
						completion: {
							completionItem: { snippetSupport: false },
						},
					},
				},
			});
		}
		this.notify("initialized", {});
	}

	private receive(raw: string): void {
		let message: JsonRpcMessage;
		try {
			message = JSON.parse(raw) as JsonRpcMessage;
		} catch (cause) {
			console.error("language server sent invalid JSON", cause);
			return;
		}
		if (
			typeof message.id === "number" &&
			(message.result !== undefined || message.error !== undefined)
		) {
			const pending = this.pending.get(message.id);
			if (!pending) return;
			this.pending.delete(message.id);
			clearTimeout(pending.timer);
			if (message.error) {
				pending.reject(new Error(message.error.message));
			} else {
				pending.resolve(message.result);
			}
			return;
		}
		if (typeof message.method === "string") {
			// The server asks the client things too — today only
			// `client/registerCapability` for file watching. The editor does not
			// watch files, but the request is acknowledged so the server never
			// waits on it.
			if (message.id !== undefined) {
				void this.enqueue({ id: message.id, result: null }).catch((cause: unknown) => {
					console.error("language server request reply failed", cause);
				});
			}
			this.notification(message.method, message.params);
		}
	}

	private notification(method: string, params: unknown): void {
		if (method !== "textDocument/publishDiagnostics") return;
		const published = params as {
			uri?: string;
			diagnostics?: Diagnostic[];
		};
		if (!published.uri || !this.monaco) return;
		const attached = this.byUri.get(published.uri);
		if (!attached) return;
		for (const document of attached) {
			if (!document.diagnostics) continue;
			const markers: Monaco.editor.IMarkerData[] = (published.diagnostics ?? []).map(
				(diagnostic) => ({
					...rangeFromLsp(document.model, diagnostic.range),
					severity: markerSeverity(diagnostic.severity),
					message: diagnostic.message,
					source: diagnostic.source ?? "rustledger",
					code:
						diagnostic.code === undefined || diagnostic.code === null
							? undefined
							: String(diagnostic.code),
				}),
			);
			this.monaco.editor.setModelMarkers(document.model, MARKER_OWNER, markers);
		}
	}

	private request(method: string, params: unknown, timeout = 2000): Promise<unknown> {
		const id = this.nextId++;
		const { promise, resolve, reject } = Promise.withResolvers<unknown>();
		const timer = setTimeout(() => {
			this.pending.delete(id);
			reject(new Error(`${method} timed out`));
		}, timeout);
		this.pending.set(id, { resolve, reject, timer });
		void this.enqueue({ id, method, params }).catch((cause: unknown) => {
			const pending = this.pending.get(id);
			if (!pending) return;
			this.pending.delete(id);
			clearTimeout(pending.timer);
			pending.reject(cause instanceof Error ? cause : new Error(String(cause)));
		});
		return promise;
	}

	private notify(method: string, params: unknown): void {
		void this.enqueue({ method, params }).catch((cause: unknown) => {
			console.error(`language server ${method} failed`, cause);
		});
	}

	private enqueue(message: JsonRpcMessage): Promise<void> {
		const sent = this.sending.then(async () => {
			const result = await commands.lspSend(JSON.stringify(message));
			if (result.status === "error") throw new Error(result.error);
		});
		// Keep the chain alive after a failed send; the request's own rejection
		// path (or the notification's log) already reported that one.
		this.sending = sent.catch(() => undefined);
		return sent;
	}

	private registerProviders(monaco: typeof Monaco): void {
		if (this.providersRegistered) return;
		this.providersRegistered = true;

		monaco.languages.registerCompletionItemProvider(BEANCOUNT_LANGUAGE, {
			triggerCharacters: [":", " ", '"', "#", "^"],
			provideCompletionItems: async (model, position, context) => {
				const document = this.documents.get(model.uri.toString());
				if (!document) return null;
				let result: unknown;
				try {
					this.flush(document);
					result = await this.request("textDocument/completion", {
						textDocument: { uri: document.uri },
						position: positionToLsp(model, position),
						context: {
							triggerKind:
								context.triggerKind === 1
									? 2
									: context.triggerKind === 2
										? 3
										: 1,
							triggerCharacter: context.triggerCharacter,
						},
					});
				} catch {
					return null;
				}
				const items = Array.isArray(result)
					? (result as CompletionItem[])
					: ((result as { items?: CompletionItem[] } | null)?.items ?? []);
				const word = model.getWordUntilPosition(position);
				const range: Monaco.IRange = {
					startLineNumber: position.lineNumber,
					startColumn: word.startColumn,
					endLineNumber: position.lineNumber,
					endColumn: word.endColumn,
				};
				return {
					suggestions: items.map(
						(item): Monaco.languages.CompletionItem => ({
							label: item.label,
							kind: completionKind(item.kind),
							detail: item.detail,
							documentation: documentationOf(item.documentation),
							insertText: item.insertText ?? item.label,
							filterText: item.filterText,
							sortText: item.sortText,
							range,
						}),
					),
				};
			},
		});

		monaco.languages.registerHoverProvider(BEANCOUNT_LANGUAGE, {
			provideHover: async (model, position) => {
				const document = this.documents.get(model.uri.toString());
				if (!document) return null;
				let result: {
					contents?: unknown;
					range?: LspRange;
				} | null;
				try {
					this.flush(document);
					result = (await this.request("textDocument/hover", {
						textDocument: { uri: document.uri },
						position: positionToLsp(model, position),
					})) as { contents?: unknown; range?: LspRange } | null;
				} catch {
					return null;
				}
				const contents = hoverContents(result?.contents);
				if (contents.length === 0) return null;
				return {
					contents,
					range: result?.range ? rangeFromLsp(model, result.range) : undefined,
				};
			},
		});
	}
}

let shared: LanguageServer | null = null;

/**
 * The backend stopped its language server — it does on every load, not only on a
 * journal switch — so the client has to reach the replacement with the documents
 * it still holds. Dropping the client would orphan every mounted editor.
 */
export function resetLanguageServer(): void {
	shared ??= new LanguageServer();
	void shared.restart();
}

/**
 * Open a journal file — or a transaction fragment at a document key of its
 * own — in the language server and wire its answers into this model. The
 * returned function detaches it; call it before disposing the model.
 */
export async function attachLanguageServer(
	monaco: typeof Monaco,
	model: Monaco.editor.ITextModel,
	path: string,
	diagnostics = true,
): Promise<() => void> {
	shared ??= new LanguageServer();
	return shared.attach(monaco, model, path, diagnostics);
}
