<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { ChevronLeftIcon, ChevronRightIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import type { BudgetMonth } from "../../../bindings";
	import BudgetGroup from "$lib/components/budget/budget-group.svelte";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as Table from "$lib/components/ui/table/index.js";
	import { budgetTotals, monthLabel, monthNeighbours } from "$lib/budget.js";

	let {
		budget,
		busy = false,
		onmonth,
	}: {
		budget: BudgetMonth;
		/** A month change is in flight; the controls wait for it. */
		busy?: boolean;
		onmonth: (month: string) => void;
	} = $props();

	const totals = $derived(budgetTotals(budget));
	const neighbours = $derived(monthNeighbours(budget.months, budget.month));
</script>

<!--
	One month of the budget, the way Actual Budget's own screen is laid out: the
	month with its arrows at the top, the four figures the month comes down to
	under it, then one collapsible section per group — its own totals on the
	section row, one row per envelope below — over three columns of amounts.
-->
<div class="flex flex-col gap-3" aria-busy={busy}>
	<div class="flex flex-wrap items-center gap-2">
		<div class="flex items-center gap-1">
			<Button
				size="icon-sm"
				variant="ghost"
				aria-label="Previous month"
				disabled={busy || !neighbours.previous}
				onclick={() => neighbours.previous && onmonth(neighbours.previous)}
			>
				<HugeiconsIcon icon={ChevronLeftIcon} strokeWidth={2} />
			</Button>
			<span class="min-w-40 text-center text-sm font-medium">{monthLabel(budget.month)}</span>
			<Button
				size="icon-sm"
				variant="ghost"
				aria-label="Next month"
				disabled={busy || !neighbours.next}
				onclick={() => neighbours.next && onmonth(neighbours.next)}
			>
				<HugeiconsIcon icon={ChevronRightIcon} strokeWidth={2} />
			</Button>
		</div>
		<!--
			Every month the plugin budgeted, which is what the arrows step through
			too — the backend reports exactly those, so neither control can land on
			a month the budget has nothing to say about.
		-->
		<span class="text-muted-foreground ml-auto text-xs">Jump to</span>
		<Select.Root
			type="single"
			value={budget.month}
			onValueChange={(next) => next && onmonth(next)}
		>
			<Select.Trigger size="sm" aria-label="Jump to month">
				<span data-slot="select-value">{monthLabel(budget.month)}</span>
			</Select.Trigger>
			<Select.Content>
				{#each budget.months as month (month)}
					<Select.Item value={month} label={monthLabel(month)} />
				{/each}
			</Select.Content>
		</Select.Root>
	</div>

	<div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
		<div
			class="rounded-lg border bg-card px-3 py-2"
			title="Money the configuration has not assigned to an envelope yet"
		>
			<p class="text-muted-foreground text-xs">To budget</p>
			<MoneyAmount
				units={budget.to_budget}
				currency={budget.currency}
				class="text-base font-semibold"
			/>
		</div>
		<div class="rounded-lg border bg-card px-3 py-2" title="What this month assigned to envelopes">
			<p class="text-muted-foreground text-xs">Budgeted</p>
			<MoneyAmount
				units={totals.budgeted}
				currency={budget.currency}
				class="text-base font-semibold"
			/>
		</div>
		<div class="rounded-lg border bg-card px-3 py-2" title="What this month spent out of them">
			<p class="text-muted-foreground text-xs">Spent</p>
			<MoneyAmount units={totals.spent} currency={budget.currency} class="text-base font-semibold" />
		</div>
		<div
			class="rounded-lg border bg-card px-3 py-2"
			title="What they hold at this month's end, every earlier month carried in — with what is left to budget, it is the ledger's on-budget money"
		>
			<p class="text-muted-foreground text-xs">In envelopes</p>
			<MoneyAmount
				units={totals.balance}
				currency={budget.currency}
				class="text-base font-semibold"
			/>
		</div>
	</div>

	{#if budget.groups.length === 0}
		<p class="text-muted-foreground text-xs">No envelope has been budgeted as of this month.</p>
	{:else}
		<Table.Root>
			<Table.Header>
				<Table.Row>
					<Table.Head>Category</Table.Head>
					<Table.Head
						class="w-28 text-right"
						title="What the month put into the envelope: inherited funding, one-off adjustments and the rollover's corrections"
					>
						Budgeted
					</Table.Head>
					<Table.Head
						class="w-28 text-right"
						title="What was spent out of the envelope, mirrored — positive for spending, negative for a refund"
					>
						Spent
					</Table.Head>
					<Table.Head
						class="w-32 text-right"
						title="What the envelope holds at this month's end, every earlier month carried into it"
					>
						Balance
					</Table.Head>
				</Table.Row>
			</Table.Header>
			{#each budget.groups as group (group.name)}
				<BudgetGroup {group} currency={budget.currency} />
			{/each}
		</Table.Root>
	{/if}
</div>
