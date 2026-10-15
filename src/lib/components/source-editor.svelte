<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { onMount } from "svelte";
	import type { editor as MonacoEditor, IRange } from "monaco-editor";
	import { commands } from "../../bindings";
	import {
		BEANCOUNT_DARK,
		BEANCOUNT_LANGUAGE,
		BEANCOUNT_LIGHT,
		fieldSnippet,
		registerBeanLanguage,
	} from "$lib/beancount";
	import { Alert, AlertDescription } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import type { EditorScope } from "$lib/editor-scope.svelte.js";
	import { cn } from "$lib/utils.js";

	/**
	 * The ledger's text in a Monaco editor — the second way to edit beside the
	 * register's in-place cells. `scope` picks how much of it:
	 *
	 * - `entry` shows the transaction's own lines; saving hands them to
	 *   `update_transaction_source`, which splices them over the entry's byte
	 *   range in the `.bean` file.
	 * - `file` shows the whole `.bean` the entry lives in and opens on the
	 *   entry (revealed and selected, so Tab fields acts on it); saving hands
	 *   the whole text to `update_file_source`.
	 *
	 * Either way the result goes through the same reload-and-stage pipeline as a
	 * cell edit: the loader is still the authority on whether the text is a
	 * legal ledger, and a refusal leaves the file byte-for-byte as it was.
	 *
	 * The file's own characters are the only text this component ever shows: the
	 * syntax highlighting is a tokenizer over them, and Tab fields turns them
	 * into snippet stops whose defaults are those same characters, so inserting
	 * the snippet writes them back unchanged.
	 */
	let {
		id,
		scope,
		onSaved,
		onCancel,
	}: {
		id: number;
		scope: EditorScope;
		onSaved: () => void;
		onCancel: () => void;
	} = $props();

	/** Where Monaco mounts. */
	let host: HTMLDivElement;
	let saving = $state(false);
	/** True once the editor is up; Save stays disabled until then. */
	let ready = $state(false);
	/** The loader's complaint, shown in line rather than as a toast. */
	let error = $state("");

	/** Monaco loads lazily, so the instance is spoken to through this handle. */
	let editor: MonacoEditor.IStandaloneCodeEditor | null = null;

	/**
	 * The entry's own range in `file` scope, as a Monaco decoration so it
	 * follows the text through edits — the cursor can then be anywhere in the
	 * file and `Tab fields` (or Tab) still knows which entry it means.
	 */
	let entryRange: MonacoEditor.IEditorDecorationsCollection | null = null;

	/**
	 * The snippet contribution, as much of it as this component uses. Monaco
	 * 0.57 no longer registers the `insertSnippet` command the older examples
	 * trigger, so the controller is taken off the editor; `getContribution`
	 * types what it returns as a bare contribution, hence the local interface.
	 */
	interface SnippetController extends MonacoEditor.IEditorContribution {
		insert(
			template: string,
			options?: {
				undoStopBefore?: boolean;
				undoStopAfter?: boolean;
				adjustWhitespace?: boolean;
			},
		): void;
		cancel(resetSelection?: boolean): void;
		isInSnippet(): boolean;
	}

	/**
	 * Puts an entry's fields under Tab: the snippet is the text's current
	 * characters with `${n:…}` stops around the date, the flag, the payee, the
	 * narration and each posting's amount, each defaulted to the file's own
	 * characters, so that inserting it writes those same characters back. Tab
	 * and Shift+Tab walk the stops, a stop whose text is emptied is skipped, and
	 * Escape leaves snippet mode. Re-entering is the toolbar's `Tab fields`
	 * button.
	 *
	 * The text it acts on is the whole model in `entry` scope and the entry's
	 * own tracked range in `file` scope — never the selection, so it works
	 * wherever the cursor happens to be, and still works after the entry has
	 * been edited.
	 *
	 * Text whose first non-comment line is not a dated entry is not something
	 * this can read: the text is left exactly as it is (and, in `entry` scope,
	 * the cursor goes to the top), because a wrong guess here would edit an
	 * entry.
	 */
	function enterFields() {
		const model = editor?.getModel();
		const controller = editor?.getContribution<SnippetController>("snippetController2");
		if (!editor || !model || !controller) return;

		const selection =
			scope === "file"
				? (entryRange?.getRange(0) ?? editor.getSelection())
				: model.getFullModelRange();
		if (!selection) return;

		const template = fieldSnippet(model.getValueInRange(selection));
		if (!template) {
			if (scope !== "file") editor.setPosition({ lineNumber: 1, column: 1 });
			editor.focus();
			return;
		}

		const before = model.getValue();
		// The snippet carries the text, so it replaces that text: with the range
		// selected, the insert changes nothing but the stops.
		editor.setSelection(selection);
		controller.insert(template, { adjustWhitespace: false });
		if (model.getValue() !== before) {
			// Should not happen — `adjustWhitespace: false` keeps Monaco from
			// re-indenting the template — but the entry is the authority here, so
			// a Monaco that reformats it gets the original text handed back.
			controller.cancel();
			model.setValue(before);
		}
		editor.focus();
	}

	/**
	 * Keys the toolbar's buttons also answer to, bound on the way down so they
	 * win over both Monaco's own bindings and the browser's:
	 *
	 * - Ctrl/Cmd+S saves, the way every editor does.
	 * - Tab, in `file` scope with the cursor inside the entry and no snippet
	 *   running, starts the field stops — the same thing `Tab fields` does, so
	 *   the whole file is still edited with the keyboard. Anywhere else in the
	 *   file Tab keeps Monaco's own meaning (indent, or step through a snippet
	 *   that is already running).
	 */
	function onKeydown(event: KeyboardEvent) {
		if (!editor) return;
		if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
			event.preventDefault();
			save();
			return;
		}
		if (event.key !== "Tab" || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) {
			return;
		}
		if (scope !== "file") return;
		const controller = editor.getContribution<SnippetController>("snippetController2");
		if (controller?.isInSnippet()) return;
		const entry = entryRange?.getRange(0);
		const position = editor.getPosition();
		if (!entry || !position || !entry.containsPosition(position)) return;
		event.preventDefault();
		enterFields();
	}

	onMount(() => {
		let disposed = false;
		let themeObserver: MutationObserver | null = null;

		(async () => {
			// Monaco is a browser-only dependency the size of an app of its own:
			// importing it here keeps it in a lazy chunk, out of SSR and out of
			// the initial bundle. Only the base editor worker is needed: the
			// beancount language is a Monarch tokenizer, which runs here on the
			// main thread, and the language-service workers belong to features
			// this editor does not use.
			const [monaco, { default: EditorWorker }] = await Promise.all([
				import("monaco-editor"),
				import("monaco-editor/editor/editor.worker.js?worker"),
			]);
			if (disposed) return;
			self.MonacoEnvironment = { getWorker: () => new EditorWorker() };
			// The ledger language and both themes have to exist before a model
			// is created: a model built under an unknown language id stays plain
			// text, and a theme id cannot be switched to before it is defined.
			registerBeanLanguage(monaco);

			// `entry` reads the transaction's own lines, `file` the whole `.bean`
			// it lives in; both are the file's characters, a different span of
			// them, and `file` also carries where the entry sits in that span.
			let text: string;
			let entry: IRange | null = null;
			if (scope === "file") {
				const result = await commands.transactionFile(id);
				if (disposed) return;
				if (result.status === "error") {
					error = result.error;
					return;
				}
				text = result.data.text;
				entry = {
					startLineNumber: result.data.start_line,
					startColumn: result.data.start_column,
					endLineNumber: result.data.end_line,
					endColumn: result.data.end_column,
				};
			} else {
				const result = await commands.transactionSource(id);
				if (disposed) return;
				if (result.status === "error") {
					error = result.error;
					return;
				}
				text = result.data;
			}

			// The app's theme is the `dark` class on `<html>`; Monaco takes an
			// explicit theme id, so read the class and follow it as it changes.
			const theme = () =>
				document.documentElement.classList.contains("dark") ? BEANCOUNT_DARK : BEANCOUNT_LIGHT;
			editor = monaco.editor.create(host, {
				value: text,
				language: BEANCOUNT_LANGUAGE,
				theme: theme(),
				minimap: { enabled: false },
				lineNumbers: "on",
				scrollBeyondLastLine: false,
				automaticLayout: true,
				fontSize: 12,
				fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
			});
			ready = true;
			if (entry) {
				// Open on the entry the editor was asked for: brought into view
				// and selected, so it is both what you see and what Tab fields
				// acts on. The decoration is what keeps that range true as the
				// entry is edited; a whole file is not a single entry, so the
				// tab stops are never laid over all of it.
				entryRange = editor.createDecorationsCollection([
					{
						range: entry,
						options: {
							stickiness: monaco.editor.TrackedRangeStickiness.GrowsOnlyWhenTypingAfter,
						},
					},
				]);
				editor.revealRangeInCenter(entry);
				editor.setSelection(entry);
				editor.focus();
			} else {
				// Opening on the entry's first field is the snippet the owner
				// asked for; the text is already the one from the file, so
				// nothing moves.
				enterFields();
			}
			themeObserver = new MutationObserver(() => monaco.editor.setTheme(theme()));
			themeObserver.observe(document.documentElement, { attributeFilter: ["class"] });
		})();

		return () => {
			disposed = true;
			themeObserver?.disconnect();
			entryRange?.clear();
			entryRange = null;
			editor?.getModel()?.dispose();
			editor?.dispose();
			editor = null;
		};
	});

	async function save() {
		if (!editor || saving) return;
		saving = true;
		error = "";
		try {
			const result =
				scope === "file"
					? await commands.updateFileSource(id, editor.getValue())
					: await commands.updateTransactionSource(id, editor.getValue());
			if (result.status === "error") {
				error = result.error;
				return;
			}
		} catch (cause) {
			// A rejection that is not the command's own error string — a missing
			// command, a payload the webview's IPC refused — arrives as a thrown
			// Error. It has to be shown and `saving` has to come back down, or the
			// button stays disabled and says nothing about why.
			error = cause instanceof Error ? cause.message : String(cause);
			return;
		} finally {
			saving = false;
		}
		bumpLedgerRevision();
		onSaved();
	}
</script>

<div
	class="border-border flex flex-col gap-2 border-t px-3 py-2"
	onkeydowncapture={onKeydown}
>
	<!--
		The whole file wants the room; the entry's own lines fit in a fold.
	-->
	<div
		bind:this={host}
		class={cn(
			"border-border overflow-hidden rounded-md border font-mono",
			scope === "file" ? "editor-pane-tall" : "editor-pane"
		)}
	></div>
	<div class="flex flex-wrap items-center gap-2">
		<Button size="sm" onclick={save} disabled={saving || !ready}>Save</Button>
		<Button size="sm" variant="ghost" onclick={enterFields} disabled={!ready}>Tab fields</Button>
		<Button size="sm" variant="ghost" onclick={onCancel} disabled={saving}>Cancel</Button>
		<span class="text-muted-foreground text-xs">
			{#if scope === "file"}
				The whole journal file; the entry opens selected. Tab on an entry starts its fields,
				Tab and Shift+Tab step through them, Esc leaves them. ⌘/Ctrl+S saves.
			{:else}
				Tab and Shift+Tab step through the entry's fields, Esc leaves them. ⌘/Ctrl+S saves.
			{/if}
		</span>
	</div>
	{#if error}
		<Alert variant="destructive">
			<AlertDescription>{error}</AlertDescription>
		</Alert>
	{/if}
</div>
