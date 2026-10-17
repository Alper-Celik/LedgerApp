<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { ChevronRightIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import type { BudgetGroup } from "../../../bindings";
	import BudgetRow from "$lib/components/budget/budget-row.svelte";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import * as Table from "$lib/components/ui/table/index.js";
	import { balancePillClass } from "$lib/budget.js";
	import { cn } from "$lib/utils.js";

	let { group, currency }: { group: BudgetGroup; currency: string } = $props();

	/**
	 * Folded away. The group's own row stays — it is where the folding is
	 * pressed, and its totals are the reason a group exists: the plugin funds an
	 * envelope's branch while the spending inside it lands on sub-accounts, so
	 * only the sum is the whole story.
	 */
	let folded = $state(false);
</script>

<Table.Body>
	<Table.Row class="bg-muted/40">
		<Table.Cell class="p-0">
			<button
				type="button"
				class="flex w-full items-center gap-1 px-2 py-1.5 text-left font-medium"
				aria-expanded={!folded}
				onclick={() => (folded = !folded)}
			>
				<HugeiconsIcon
					icon={ChevronRightIcon}
					strokeWidth={2}
					class={cn("size-3.5 shrink-0 transition-transform", !folded && "rotate-90")}
				/>
				<span class="truncate">{group.name}</span>
			</button>
		</Table.Cell>
		<Table.Cell class="w-28 px-2 py-1 text-right">
			<MoneyAmount units={group.budgeted} {currency} />
		</Table.Cell>
		<Table.Cell class="w-28 px-2 py-1 text-right">
			<MoneyAmount units={group.spent} {currency} />
		</Table.Cell>
		<Table.Cell class="w-32 px-2 py-1 text-right">
			<span
				class={cn(
					"inline-block rounded-full px-2 py-0.5 font-medium tabular-nums",
					balancePillClass(group.balance)
				)}
			>
				<MoneyAmount units={group.balance} {currency} />
			</span>
		</Table.Cell>
	</Table.Row>
	{#if !folded}
		{#each group.envelopes as envelope (envelope.full_name)}
			<BudgetRow {envelope} {currency} />
		{/each}
	{/if}
</Table.Body>
