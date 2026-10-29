<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import * as Table from "$lib/components/ui/table/index.js";
	import type { BalanceRow } from "./data.js";

	/**
	 * A ledger's balances at the configured depth, as a table.
	 *
	 * Amounts are the backend's exact decimal strings shown through
	 * `MoneyAmount`; this component never adds, rounds or converts them. The
	 * body scrolls so a long ledger cannot push the rest of the dashboard off
	 * the page.
	 */
	let { rows }: { rows: readonly BalanceRow[] } = $props();
</script>

<div class="max-h-64 overflow-auto rounded-md">
	<Table.Root>
		<Table.Header class="bg-card sticky top-0 z-10">
			<Table.Row class="hover:bg-transparent">
				<Table.Head class="text-muted-foreground h-8 text-xs font-medium">Account</Table.Head>
				<Table.Head class="text-muted-foreground h-8 text-right text-xs font-medium">
					Balance
				</Table.Head>
			</Table.Row>
		</Table.Header>
		<Table.Body>
			{#each rows as row (row.account)}
				<Table.Row class="h-8">
					<Table.Cell class="w-full max-w-0 truncate p-2 text-xs" title={row.account}>
						{row.account}
					</Table.Cell>
					<Table.Cell class="p-2 text-right">
						<MoneyAmount units={row.balance} currency={row.currency} />
					</Table.Cell>
				</Table.Row>
			{/each}
		</Table.Body>
	</Table.Root>
</div>
