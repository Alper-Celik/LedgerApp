<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { TransactionView } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import AccountRegister from "$lib/components/account-register.svelte";
	import { LEDGER_LABEL } from "$lib/accounts.js";
	import { clearFilters, filters, matchesFilters } from "$lib/filters.svelte.js";

	/**
	 * A register for one account, or for the whole ledger when `account` is
	 * empty (the sidebar's virtual root). Filters are global, so a chart can
	 * zoom a date range and this view follows it.
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
		<h1 class="truncate text-lg font-semibold tracking-tight">{account || LEDGER_LABEL}</h1>
		<span class="text-muted-foreground text-xs">
			{visible.length}{transactions.length === visible.length
				? ""
				: ` of ${transactions.length}`} transactions
		</span>
	</div>

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
</div>
