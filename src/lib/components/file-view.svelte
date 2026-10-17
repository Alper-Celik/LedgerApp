<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands, type JournalFile, type TransactionView } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import AccountRegister from "$lib/components/account-register.svelte";
	import SourceEditor from "$lib/components/source-editor.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { ledgerRevision } from "$lib/ledger-revision.svelte.js";

	/** How the open file is shown: its transactions, or its text. */
	type FileMode = "transactions" | "text";

	/**
	 * One journal file, read two ways — the two ways an entry is read.
	 *
	 * As transactions, through the register itself: every entry with all of its
	 * legs, since a file has no account to name the other side by or to total,
	 * and a file is read rather than asked about one entry at a time. An entry
	 * composed here goes into this file.
	 *
	 * As text, through the source editor: the whole file, exactly as it is on
	 * disk. Both write through the same reload-and-stage pipeline as every other
	 * edit, and the loader still decides what a file may contain.
	 */
	let {
		path,
		file,
		transactions,
		error,
	}: {
		/** The path the browser's link carried; the page is about this file. */
		path: string;
		/** The file as the journal lists it; `null` when the journal does not hold it. */
		file: JournalFile | null;
		transactions: TransactionView[];
		error: string;
	} = $props();

	/** The way this visit reads the file; a way of looking, so it is not remembered. */
	let mode = $state<FileMode>("transactions");

	/**
	 * The text editor is mounted when the text mode is first asked for and kept
	 * after that, hidden while transactions show: typing in the file and glancing
	 * at its entries would otherwise throw the text away.
	 */
	let textOpened = $state(false);
	$effect(() => {
		if (mode === "text") textOpened = true;
	});

	/** What the refetch last answered; `null` until an edit has moved the ledger. */
	let answered = $state<TransactionView[] | null>(null);
	/** What the register shows: the page's own rows, or the refetched ones once there are any. */
	let entries = $derived(answered ?? transactions);
	$effect(() => {
		if (ledgerRevision.value === 0) return;
		void refreshed(path);
	});

	/** The entries as the backend has them now; an edit anywhere may have added one. */
	async function refreshed(wanted: string) {
		try {
			const result = await commands.fileTransactions(wanted);
			// A refetch that arrives after the page has moved to another file is
			// about a file this view no longer shows.
			if (result.status === "ok" && wanted === path) answered = result.data;
		} catch (cause) {
			console.error("fileTransactions failed", cause);
		}
	}
</script>

<div class="flex flex-col gap-3">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="truncate text-lg font-semibold tracking-tight">{file?.label ?? path}</h1>
		<span class="text-muted-foreground truncate text-xs" title={path}>{path}</span>
		<UncommittedMark modified={file?.modified ?? false} />
		<span class="text-muted-foreground text-xs">
			{entries.length}{entries.length === 1 ? " transaction" : " transactions"}
		</span>
		<ToggleGroup.Root
			class="ms-auto"
			type="single"
			value={mode}
			onValueChange={(next) => next && (mode = next as FileMode)}
			aria-label="File view"
		>
			<ToggleGroup.Item value="transactions">Transactions</ToggleGroup.Item>
			<ToggleGroup.Item value="text">Text</ToggleGroup.Item>
		</ToggleGroup.Root>
	</div>

	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{/if}

	{#if textOpened}
		<!--
			One editor per file: a different file is a different text, so the editor
			is rebuilt for it. A file rewritten elsewhere is caught by the editor's
			own save, which refuses text written against a stale copy.
		-->
		{#key path}
			<div class={mode === "text" ? "" : "hidden"}>
				<SourceEditor file={path} />
			</div>
		{/key}
	{/if}

	{#if mode === "transactions" && !error}
		{#if entries.length === 0}
			<p class="text-muted-foreground text-sm">This file writes no transactions.</p>
		{:else}
			<AccountRegister account="" transactions={entries} fixedView="full" newEntryFile={path} expandAll />
		{/if}
	{/if}
</div>
