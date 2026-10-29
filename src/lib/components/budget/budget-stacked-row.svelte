<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { ChevronRightIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import BudgetAmount from "$lib/components/budget/budget-amount.svelte";
	import { monthLabel } from "$lib/budget.js";
	import type { BudgetCell, BudgetRow } from "$lib/budget-grid.js";
	import { cn } from "$lib/utils.js";

	let {
		row,
		cell,
		month,
		currency,
		collapsed = false,
		ontoggle,
	}: {
		row: BudgetRow;
		/** The row's figures in the one month the phone layout shows. */
		cell: BudgetCell;
		/** That month, `YYYY-MM`, for the register links' labels. */
		month: string;
		currency: string;
		/** A group's envelopes are folded away; only a group row has any. */
		collapsed?: boolean;
		ontoggle: () => void;
	} = $props();

	/** A group's own row and one of its envelopes are drawn differently, not by two components. */
	const grouped = $derived(row.envelope === "");
</script>

<!--
	One entry of the phone grid: two lines, the label over three labelled
	figures — the columns a wide screen draws side by side, stacked so a 360px
	viewport never scrolls sideways. The group row keeps its fold button and its
	own totals, exactly as the desktop row does.
-->
<div class={cn("border-b px-3 py-2", grouped && "bg-muted/40")}>
	{#if grouped}
		<button
			type="button"
			class="focus-visible:ring-ring flex min-w-0 items-center gap-1 text-sm font-semibold focus-visible:ring-2 focus-visible:outline-none"
			aria-expanded={!collapsed}
			onclick={ontoggle}
		>
			<HugeiconsIcon
				icon={ChevronRightIcon}
				strokeWidth={2}
				class={cn("size-4 shrink-0 transition-transform", !collapsed && "rotate-90")}
			/>
			<span class="truncate">{row.group}</span>
		</button>
	{:else}
		<div class="truncate text-sm" title={row.account}>{row.envelope}</div>
	{/if}
	<div class="mt-1 grid grid-cols-3 gap-2">
		{#each [{ label: "Budgeted", units: cell.budgeted }, { label: "Spent", units: cell.spent }, { label: "Balance", units: cell.balance }] as figure (figure.label)}
			<div class="flex min-w-0 flex-col gap-0.5">
				<span class="text-muted-foreground text-[10px] tracking-wide uppercase">
					{figure.label}
				</span>
				{#if figure.label === "Spent" && row.fullName}
					<!-- As on the desktop grid: the envelope's spending opens its register. -->
					<a
						class="focus-visible:ring-ring rounded-sm text-sm underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:outline-none"
						href={resolve("/account/[id]", { id: row.fullName })}
						aria-label={`${row.account} register, ${monthLabel(month)}`}
						title="Open this envelope's register"
					>
						<BudgetAmount units={figure.units} {currency} />
					</a>
				{:else}
					<span class="text-sm">
						<BudgetAmount units={figure.units} {currency} />
					</span>
				{/if}
			</div>
		{/each}
	</div>
</div>
