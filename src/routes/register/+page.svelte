<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { goto } from "$app/navigation";
	import { resolve } from "$app/paths";
	import TransactionBrowser from "$lib/components/transaction-browser.svelte";
	import { Button } from "$lib/components/ui/button/index.js";
	import { selectionParam } from "$lib/multi-register.js";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();

	let picked = $state("");
	const selected = $derived(data.accounts);

	/** The selection lives in the URL, so every view of it is deep-linkable. */
	function hrefFor(accounts: string[]): string {
		const param = selectionParam(accounts);
		return param ? `${resolve("/register")}?accounts=${param}` : resolve("/register");
	}

	function addPicked() {
		if (!picked || selected.includes(picked)) return;
		goto(hrefFor([...selected, picked]));
	}
</script>

<div class="flex flex-col gap-4 p-4">
	{#if selected.length === 0}
		<p class="text-muted-foreground text-sm">
			No accounts selected. Use the + button next to an account in the sidebar, or pick one here.
		</p>
	{:else}
		<div class="flex flex-wrap items-center gap-2">
			{#each selected as account (account)}
				<a
					class="flex items-center gap-1 rounded-md border bg-secondary px-2 py-0.5 text-xs font-medium hover:bg-secondary/80"
					href={hrefFor(selected.filter((name) => name !== account))}
					aria-label={`Remove ${account} from selection`}
				>
					{account}
					<span aria-hidden="true">×</span>
				</a>
			{/each}
			<Button size="sm" variant="ghost" href={hrefFor([])}>Clear all</Button>
		</div>
	{/if}

	<div class="flex items-center gap-2">
		<select
			bind:value={picked}
			class="h-8 max-w-xs rounded-md border bg-transparent px-2 text-sm"
			aria-label="Account to add"
		>
			<option value="" disabled>Pick an account…</option>
			{#each data.allAccounts as account (account.name)}
				<option value={account.name}>{account.name}</option>
			{/each}
		</select>
		<Button size="sm" onclick={addPicked} disabled={!picked}>Add</Button>
	</div>

	{#if selected.length > 0}
		<TransactionBrowser
			account=""
			transactions={data.transactions}
			error={data.error}
			title={selected.length === 1 ? selected[0] : `${selected.length} selected accounts`}
			emptyText="No transactions touch the selected accounts."
		/>
	{/if}
</div>
