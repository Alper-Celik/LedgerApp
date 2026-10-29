<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import BudgetAmount from "$lib/components/budget/budget-amount.svelte";
	import { monthLabel } from "$lib/budget.js";
	import { signOf } from "$lib/money.js";
	import { cn } from "$lib/utils.js";

	let {
		month,
		toBudget,
		currency,
		totals = null,
		selected = false,
		onpick,
	}: {
		/** `YYYY-MM`. */
		month: string;
		/** `Equity:Envelope:To-Be-Budgeted` at the month's end, as the backend wrote it. */
		toBudget: string;
		currency: string;
		/**
		 * The month's own Budgeted/Spent/Balance. The desktop header band carries
		 * them for every visible month, so only the phone card — which has no
		 * header row — hands them over.
		 */
		totals?: { budgeted: string; spent: string; balance: string } | null;
		/** This month is the window's first, the one the arrow keys move away from. */
		selected?: boolean;
		/** Pick this month as the window's first; without it the card is a plain summary. */
		onpick?: () => void;
	} = $props();

	/**
	 * To-Be-Budgeted is the plugin's rule `on-budget money − Σ categories`: below
	 * zero the month assigned more than it had, which the card says in words
	 * rather than leaving a bare minus to explain.
	 */
	const over = $derived(signOf(toBudget) < 0);
</script>

<!--
	One month of the strip, the way Actual Budget's own grid opens: the month and
	what is still to assign, over a card that picks the window when pressed. The
	desktop strip is the month picker; the phone shows the same card alone.
-->
<svelte:element
	this={onpick ? "button" : "div"}
	type={onpick ? "button" : undefined}
	class={cn(
		"flex h-full w-full min-w-0 flex-col justify-center gap-0.5 px-2 py-1 text-left",
		onpick &&
			"hover:bg-accent/60 focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none",
		onpick && selected && "bg-accent/50"
	)}
	role={onpick ? "button" : undefined}
	aria-pressed={onpick ? selected : undefined}
	onclick={onpick}
>
	<div class="flex min-w-0 items-baseline justify-between gap-2">
		<span class="truncate text-xs font-medium">{monthLabel(month)}</span>
		<span class="text-muted-foreground shrink-0 text-[10px]">
			{over ? "Overbudgeted:" : "To budget"}
		</span>
	</div>
	<BudgetAmount
		units={toBudget}
		{currency}
		tone="to-budget"
		showSymbol
		class="text-sm font-semibold"
	/>
	{#if totals}
		<div class="mt-1 grid grid-cols-3 gap-2 border-t pt-1">
			{#each [{ label: "Budgeted", units: totals.budgeted }, { label: "Spent", units: totals.spent }, { label: "Balance", units: totals.balance }] as figure (figure.label)}
				<div class="flex min-w-0 flex-col gap-0.5">
					<span class="text-muted-foreground text-[10px] tracking-wide uppercase">
						{figure.label}
					</span>
					<BudgetAmount units={figure.units} {currency} class="text-xs" />
				</div>
			{/each}
		</div>
	{/if}
</svelte:element>
