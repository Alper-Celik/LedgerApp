<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { onMount } from "svelte";
	import { commands } from "../../bindings";
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
	let editor: import("monaco-editor").editor.IStandaloneCodeEditor | null = null;

	onMount(() => {
		let disposed = false;
		let themeObserver: MutationObserver | null = null;

		(async () => {
			// Monaco is a browser-only dependency the size of an app of its own:
			// importing it here keeps it in a lazy chunk, out of SSR and out of
			// the initial bundle. Only the base editor worker is needed — the
			// language-service workers belong to language features this
			// plain-text editor does not use.
			const [monaco, { default: EditorWorker }] = await Promise.all([
				import("monaco-editor"),
				import("monaco-editor/editor/editor.worker.js?worker"),
			]);
			if (disposed) return;
			self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

			const result = await commands.transactionSource(id);
			if (disposed) return;
			if (result.status === "error") {
				error = result.error;
				return;
			}

			// The app's theme is the `dark` class on `<html>`; Monaco takes an
			// explicit theme id, so read the class and follow it as it changes.
			const theme = () => (document.documentElement.classList.contains("dark") ? "vs-dark" : "vs");
			editor = monaco.editor.create(host, {
				value: result.data,
				language: "plaintext",
				theme: theme(),
				minimap: { enabled: false },
				lineNumbers: "on",
				scrollBeyondLastLine: false,
				automaticLayout: true,
				fontSize: 12,
				fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
			});
			ready = true;
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
		<Button size="sm" variant="ghost" onclick={onCancel} disabled={saving}>Cancel</Button>
		{#if error}
			<span class="text-xs text-red-600 dark:text-red-400">{error}</span>
		{/if}
	</div>
</div>
