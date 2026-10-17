<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { TransactionView } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import AccountRegister from "$lib/components/account-register.svelte";
	import FileView from "$lib/components/file-view.svelte";
	import { LEDGER_LABEL } from "$lib/accounts.js";
	import { browser, setBrowserTab, type BrowserTab } from "$lib/file-view.svelte.js";
	import { clearFilters, filters, matchesFilters } from "$lib/filters.svelte.js";

	/**
	 * A register for one account, or for the whole ledger when `account` is
	 * empty (the sidebar's virtual root). Filters are global, so a chart can
	 * zoom a date range and this view follows it.
	 *
	 * The page has two tabs: the register itself, and the journal's files (the
	 * explorer, which opens one file as its transactions or as its text). The
	 * choice is module state, so it follows the reader from an account to the
	 * ledger and back.
	 */
	let { account, transactions, error = "" }: {
		account: string;
		transactions: TransactionView[];
		error?: string;
	} = $props();
	let visible = $derived(transactions.filter(matchesFilters));
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="truncate text-lg font-semibold tracking-tight">
			{browser.tab === "register" ? account || LEDGER_LABEL : "Files"}
		</h1>
		{#if browser.tab === "register"}
			<span class="text-muted-foreground text-xs">
				{visible.length}{transactions.length === visible.length
					? ""
					: ` of ${transactions.length}`} transactions
			</span>
		{:else}
			<span class="text-muted-foreground text-xs">the journal, file by file</span>
		{/if}
		<ToggleGroup.Root
			class="ms-auto"
			type="single"
			value={browser.tab}
			onValueChange={(next) => next && setBrowserTab(next as BrowserTab)}
			aria-label="View"
		>
			<ToggleGroup.Item value="register">Register</ToggleGroup.Item>
			<ToggleGroup.Item value="files">Files</ToggleGroup.Item>
		</ToggleGroup.Root>
	</div>

	{#if browser.tab === "files"}
		<FileView />
	{:else}
		<div class="flex flex-wrap items-center gap-2">
			<Input
				class="h-8 max-w-xs"
				placeholder="Search transactions…"
				aria-label="Search transactions"
				bind:value={filters.query}
			/>
			{#if filters.from || filters.to}
				<!-- A chart sets this range on the shared filters and the register follows it. -->
				<span class="text-muted-foreground text-xs">
					{filters.from || "…"} → {filters.to || "…"}
				</span>
			{/if}
			{#if filters.query || filters.from || filters.to}
				<Button size="sm" variant="ghost" onclick={clearFilters}>Clear</Button>
			{/if}
		</div>

		{#if error}
			<Alert variant="destructive">{error}</Alert>
		{:else}
			{#if visible.length === 0}
				<!--
					Only the message changes: the register itself stays, since that is
					where an entry is added, and an account with nothing in it yet is
					exactly where one is most likely to be.
				-->
				<p class="text-muted-foreground text-sm">
					{transactions.length === 0
						? `No transactions${account ? " touch this account" : ""}.`
						: "No transactions match the filter."}
				</p>
			{/if}
			<AccountRegister {account} transactions={visible} />
		{/if}
	{/if}
</div>
