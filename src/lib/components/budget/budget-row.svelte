<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { BudgetEnvelope } from "../../../bindings";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import * as Table from "$lib/components/ui/table/index.js";
	import { balancePillClass } from "$lib/budget.js";
	import { signOf } from "$lib/money.js";
	import { cn } from "$lib/utils.js";

	let { envelope, currency }: { envelope: BudgetEnvelope; currency: string } = $props();

	/** In the red: the row is marked as overspent, not only painted red. */
	const overspent = $derived(signOf(envelope.balance) < 0);
</script>

<!--
	One envelope of a group. The figures are the backend's own — exact decimal
	strings, added up and carried forward there — and the row shows them the way
	the budget columns read: what the month assigned, what it spent, and the pill
	the balance ends the month on.
-->
<Table.Row class={cn(overspent && "bg-red-600/5 dark:bg-red-400/5")}>
	<Table.Cell class="py-1 pr-3 pl-8">
		<span class="truncate" title={envelope.full_name}>{envelope.name}</span>
	</Table.Cell>
	<Table.Cell class="w-28 px-2 py-1 text-right">
		<MoneyAmount units={envelope.budgeted} {currency} />
	</Table.Cell>
	<Table.Cell class="w-28 px-2 py-1 text-right">
		<MoneyAmount units={envelope.spent} {currency} />
	</Table.Cell>
	<Table.Cell class="w-32 px-2 py-1 text-right">
		<span
			class={cn(
				"inline-block rounded-full px-2 py-0.5 tabular-nums",
				balancePillClass(envelope.balance)
			)}
			title={overspent ? "Overspent" : undefined}
		>
			<MoneyAmount units={envelope.balance} {currency} />
		</span>
	</Table.Cell>
</Table.Row>
