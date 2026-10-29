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
	import type { BudgetRow } from "$lib/budget-grid.js";
	import { cn } from "$lib/utils.js";

	let {
		row,
		template,
		currency,
		months,
		collapsed = false,
		ontoggle,
	}: {
		row: BudgetRow;
		/** The `grid-template-columns` every band and row shares, so the columns line up down the screen. */
		template: string;
		currency: string;
		/** The visible months, in the same order as `row.cells`, for the register links' labels. */
		months: readonly { month: string }[];
		/** A group's envelopes are folded away; only a group row has any. */
		collapsed?: boolean;
		ontoggle: () => void;
	} = $props();

	/** A group's own row and one of its envelopes are drawn differently, not by two components. */
	const grouped = $derived(row.envelope === "");
</script>

<!--
	One line of the multi-month grid: a group's own totals (bold, foldable, the
	only place its envelopes' figures are summed) or one envelope's three figures
	under each visible month. The whole row is a grid on the shared template, so
	its cells sit exactly under the header band's.
-->
<div
	role="row"
	class={cn("grid h-8 items-stretch border-b", grouped && "bg-muted/40")}
	style:grid-template-columns={template}
>
	{#if grouped}
		<div role="rowheader" class="min-w-0">
			<button
				type="button"
				class="hover:bg-accent/60 focus-visible:ring-ring flex h-full w-full min-w-0 items-center gap-1 px-2 text-left text-xs font-semibold focus-visible:ring-2 focus-visible:outline-none"
				aria-expanded={!collapsed}
				onclick={ontoggle}
			>
				<HugeiconsIcon
					icon={ChevronRightIcon}
					strokeWidth={2}
					class={cn("size-3.5 shrink-0 transition-transform", !collapsed && "rotate-90")}
				/>
				<span class="truncate">{row.group}</span>
			</button>
		</div>
	{:else}
		<div role="rowheader" class="flex min-w-0 items-center px-2 pl-7 text-xs">
			<span class="truncate" title={row.fullName}>{row.envelope}</span>
		</div>
	{/if}
	{#each row.cells as cell, index (index)}
		<div role="cell" class="grid grid-cols-3 items-center border-s">
			<div class="overflow-hidden px-1 text-right text-xs">
				<BudgetAmount units={cell.budgeted} {currency} />
			</div>
			<div class="overflow-hidden px-1 text-right text-xs">
				{#if row.fullName}
					<!--
						The one thing a cell does: an envelope's spending opens the account
						it was spent from. The register takes no month filter, so the whole
						account is the honest target.
					-->
					<a
						class="focus-visible:ring-ring rounded-sm underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:outline-none"
						href={resolve("/account/[id]", { id: row.fullName })}
						aria-label={`${row.fullName} register, ${monthLabel(months[index]?.month ?? "")}`}
						title="Open this envelope's register"
					>
						<BudgetAmount units={cell.spent} {currency} />
					</a>
				{:else}
					<BudgetAmount units={cell.spent} {currency} />
				{/if}
			</div>
			<div class="overflow-hidden px-1 text-right text-xs">
				<BudgetAmount units={cell.balance} {currency} />
			</div>
		</div>
	{/each}
</div>
