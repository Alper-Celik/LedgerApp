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
	 * empty (the sidebar's virtual root). Filters are shared, so the search
	 * carries across accounts.
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
			class="h-8 w-full sm:max-w-xs"
			placeholder="Search transactions…"
			aria-label="Search transactions"
			bind:value={filters.query}
		/>
		{#if filters.query}
			<Button size="sm" variant="ghost" onclick={clearFilters}>Clear</Button>
		{/if}
	</div>

	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{:else if visible.length === 0}
		<p class="text-muted-foreground text-sm">
			{transactions.length === 0
				? `No transactions${account ? " touch this account" : ""}.`
				: "No transactions match the filter."}
		</p>
	{:else}
		<AccountRegister {account} transactions={visible} />
	{/if}
</div>
