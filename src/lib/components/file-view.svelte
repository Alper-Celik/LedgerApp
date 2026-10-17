<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { ArrowLeft01Icon } from "@hugeicons/core-free-icons";
	import { commands, type JournalFile, type TransactionView } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Table from "$lib/components/ui/table/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import AccountRegister from "$lib/components/account-register.svelte";
	import SourceEditor from "$lib/components/source-editor.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { browser, openBrowserFile, type FileMode } from "$lib/file-view.svelte.js";
	import { ledgerRevision } from "$lib/ledger-revision.svelte.js";

	/**
	 * The journal's files, and the one that is open.
	 *
	 * The list says what each file holds — how many entries it writes, whether it
	 * is committed — so the explorer answers "where is what" before a file is
	 * opened. An open file is read two ways, the same two ways an entry is: as
	 * transactions, through the register itself (every leg shown, since a file has
	 * no account to be scoped to), and as the text it is on disk, through the
	 * source editor. Both write through the same reload-and-stage pipeline, and
	 * the loader still decides what a file may contain.
	 */
	let files = $state<JournalFile[]>([]);
	/** The open file's transactions, as the register takes them. */
	let transactions = $state<TransactionView[]>([]);
	let error = $state("");

	/** The open file, as the explorer listed it. */
	let open = $derived(files.find((file) => file.path === browser.path));

	/**
	 * The text editor is mounted when the text mode is first asked for and kept
	 * after that, hidden while transactions show: typing in the file and glancing
	 * at its entries would otherwise throw the text away.
	 */
	let textOpened = $state(false);
	$effect(() => {
		if (browser.mode === "text") textOpened = true;
	});

	async function load() {
		error = "";
		try {
			files = await commands.journalFiles();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : String(cause);
			return;
		}
		if (!browser.path) {
			transactions = [];
			return;
		}
		try {
			const result = await commands.fileTransactions(browser.path);
			if (result.status === "error") {
				error = result.error;
				transactions = [];
				return;
			}
			transactions = result.data;
		} catch (cause) {
			error = cause instanceof Error ? cause.message : String(cause);
			transactions = [];
		}
	}

	/** The listing and the open file both follow the ledger: every edit bumps the revision. */
	$effect(() => {
		void browser.path;
		void ledgerRevision.value;
		void load();
	});

	/** What an empty file says for itself. */
	const EMPTY = "This file writes no transactions.";
</script>

{#if browser.path}
	<div class="flex flex-col gap-3">
		<div class="flex flex-wrap items-center gap-2">
			<Button size="sm" variant="ghost" onclick={() => (browser.path = null)}>
				<HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} data-icon="inline-start" />
				Files
			</Button>
			<h2 class="truncate text-base font-semibold tracking-tight">
				{open?.label ?? browser.path}
			</h2>
			<span class="text-muted-foreground truncate text-xs" title={browser.path}>{browser.path}</span>
			<UncommittedMark modified={open?.modified ?? false} />
			<span class="text-muted-foreground text-xs">
				{transactions.length}{transactions.length === 1 ? " transaction" : " transactions"}
			</span>
			<ToggleGroup.Root
				class="ms-auto"
				type="single"
				value={browser.mode}
				onValueChange={(next) => next && (browser.mode = next as FileMode)}
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
				One editor per file: a different file is a different text, so the
				editor is rebuilt for it. A file rewritten elsewhere is caught by the
				editor's own save, which refuses text written against a stale copy.
			-->
			{#key browser.path}
				<div class={browser.mode === "text" ? "" : "hidden"}>
					<SourceEditor file={browser.path} />
				</div>
			{/key}
		{/if}

		{#if browser.mode === "transactions" && !error}
			{#if transactions.length === 0}
				<p class="text-muted-foreground text-sm">{EMPTY}</p>
			{:else}
				<!--
					The register, scoped to a file rather than to an account: no
					account to name the other side by, no account totals to put in
					the amount columns, so it reads as the full journal — every entry
					with all of its legs — and a new entry composed here goes into
					this file.
				-->
				<AccountRegister
					account=""
					{transactions}
					fixedView="full"
					newEntryFile={browser.path}
					expandAll
				/>
			{/if}
		{/if}
	</div>
{:else}
	<div class="flex flex-col gap-3">
		{#if error}
			<Alert variant="destructive">{error}</Alert>
		{/if}
		{#if files.length === 0}
			<p class="text-muted-foreground text-sm">No journal file is loaded.</p>
		{:else}
			<!--
				Fixed layout, like the register: the label and the path columns
				truncate, the count keeps its width.
			-->
			<Table.Root class="table-fixed">
				<colgroup>
					<col class="col-file-name" />
					<col class="col-file-path" />
					<col class="col-file-count" />
					<col class="col-file-mark" />
				</colgroup>
				<Table.Header>
					<Table.Row>
						<Table.Head class="h-7 pl-2">File</Table.Head>
						<Table.Head class="h-7 border-s border-border pl-2">Path</Table.Head>
						<Table.Head class="h-7 border-s border-border px-2 text-right">Transactions</Table.Head>
						<Table.Head class="h-7 border-s border-border"></Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each files as file (file.path)}
						<Table.Row class="transition-motion hover:bg-accent/50">
							<Table.Cell class="truncate py-1.5 pr-2 pl-2 font-medium">
								<button
									type="button"
									class="max-w-full truncate text-left underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring/30 focus-visible:outline-none"
									onclick={() => openBrowserFile(file.path)}
								>
									{file.label}
								</button>
							</Table.Cell>
							<Table.Cell class="text-muted-foreground border-s border-border truncate py-1.5 pr-2 pl-2">
								{file.path}
							</Table.Cell>
							<Table.Cell class="border-s border-border px-2 py-1.5 text-right tabular-nums">
								{file.transactions}
							</Table.Cell>
							<Table.Cell class="border-s border-border py-1.5 pr-2 pl-2">
								<UncommittedMark modified={file.modified} />
							</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		{/if}
	</div>
{/if}
