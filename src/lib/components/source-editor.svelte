<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { onMount } from "svelte";
	import type { editor as MonacoEditor } from "monaco-editor";
	import { commands } from "../../bindings";
	import {
		BEANCOUNT_DARK,
		BEANCOUNT_LANGUAGE,
		BEANCOUNT_LIGHT,
		fieldSnippet,
		registerBeanLanguage,
	} from "$lib/beancount";
	import { Button } from "$lib/components/ui/button/index.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";

	/**
	 * A transaction's own source lines in a Monaco editor — the second way to
	 * edit an entry beside the register's in-place cells. Saving hands the whole
	 * text to `update_transaction_source`, which splices it over the
	 * transaction's own byte range in the `.bean` file and puts the result
	 * through the same reload-and-stage pipeline as a cell edit: the loader is
	 * still the authority on whether the entry is legal, and a refusal leaves
	 * the file byte-for-byte as it was.
	 *
	 * The entry's own characters are the only text this component ever shows:
	 * the syntax highlighting is a tokenizer over them, and Tab fields turns
	 * them into snippet stops whose defaults are those same characters, so
	 * inserting the snippet writes the entry back unchanged.
	 */
	let {
		id,
		onSaved,
		onCancel,
	}: {
		id: number;
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
	 * Puts the entry's fields under Tab: the snippet is the entry's current text
	 * with `${n:…}` stops around the date, the flag, the payee, the narration
	 * and each posting's amount, each defaulted to the file's own characters, so
	 * that inserting it writes those same characters back. Tab and Shift+Tab
	 * walk the stops, a stop whose text is emptied is skipped, and Escape leaves
	 * snippet mode. Re-entering is the toolbar's `Tab fields` button.
	 *
	 * Text whose first non-comment line is not a dated entry is not something
	 * this can read: the text is left exactly as it is and the cursor goes to
	 * the top, because a wrong guess here would edit an entry.
	 */
	function enterFields() {
		const model = editor?.getModel();
		const controller = editor?.getContribution<SnippetController>("snippetController2");
		if (!editor || !model || !controller) return;

		const template = fieldSnippet(model.getValue());
		if (!template) {
			editor.setPosition({ lineNumber: 1, column: 1 });
			editor.focus();
			return;
		}

		const before = model.getValue();
		// The snippet carries the whole text, so it replaces the whole text:
		// with everything selected, the insert changes nothing but the stops.
		editor.setSelection(model.getFullModelRange());
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

			const result = await commands.transactionSource(id);
			if (disposed) return;
			if (result.status === "error") {
				error = result.error;
				return;
			}

			// The app's theme is the `dark` class on `<html>`; Monaco takes an
			// explicit theme id, so read the class and follow it as it changes.
			const theme = () =>
				document.documentElement.classList.contains("dark") ? BEANCOUNT_DARK : BEANCOUNT_LIGHT;
			editor = monaco.editor.create(host, {
				value: result.data,
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
			// Opening on the entry's first field is the snippet the owner asked
			// for; the text is already the one from the file, so nothing moves.
			enterFields();
			themeObserver = new MutationObserver(() => monaco.editor.setTheme(theme()));
			themeObserver.observe(document.documentElement, { attributeFilter: ["class"] });
		})();

		return () => {
			disposed = true;
			themeObserver?.disconnect();
			editor?.getModel()?.dispose();
			editor?.dispose();
			editor = null;
		};
	});

	async function save() {
		if (!editor) return;
		saving = true;
		error = "";
		const result = await commands.updateTransactionSource(id, editor.getValue());
		saving = false;
		if (result.status === "error") {
			error = result.error;
			return;
		}
		bumpLedgerRevision();
		onSaved();
	}
</script>

<div class="border-border/40 flex flex-col gap-2 border-t px-3 py-2">
	<div bind:this={host} class="border-border/60 h-[200px] overflow-hidden rounded-md border font-mono"></div>
	<div class="flex items-center gap-2">
		<Button size="sm" onclick={save} disabled={saving || !ready}>Save</Button>
		<Button size="sm" variant="ghost" onclick={enterFields} disabled={!ready}>Tab fields</Button>
		<Button size="sm" variant="ghost" onclick={onCancel} disabled={saving}>Cancel</Button>
		<span class="text-muted-foreground text-xs">
			Tab and Shift+Tab step through the entry's fields, Esc leaves them.
		</span>
		{#if error}
			<span class="text-xs text-red-600 dark:text-red-400">{error}</span>
		{/if}
	</div>
</div>
