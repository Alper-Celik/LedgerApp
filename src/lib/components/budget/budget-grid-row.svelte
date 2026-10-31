<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { ChevronRightIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import type { BudgetSettings } from "../../../bindings";
	import BudgetAmount from "$lib/components/budget/budget-amount.svelte";
	import BudgetBudgetedCell from "$lib/components/budget/budget-budgeted-cell.svelte";
	import BudgetRolloverControl from "$lib/components/budget/budget-rollover-control.svelte";
	import { monthLabel } from "$lib/budget.js";
	import type { BudgetRow } from "$lib/budget-grid.js";
	import {
		categoryAccount,
		envelopeFile,
		envelopeSetting,
	} from "$lib/budget-settings.js";
	import { cn } from "$lib/utils.js";

	let {
		row,
		template,
		currency,
		months,
		settings = null,
		collapsed = false,
		ontoggle,
	}: {
		row: BudgetRow;
		/** The `grid-template-columns` every band and row shares, so the columns line up down the screen. */
		template: string;
		currency: string;
		/** The visible months, in the same order as `row.cells`, for the register links' labels. */
		months: readonly { month: string }[];
		/** The budget's writable configuration, for the row's funding and rollover controls. */
		settings?: BudgetSettings | null;
		/** A group's envelopes are folded away; only a group row has any. */
		collapsed?: boolean;
		ontoggle: () => void;
	} = $props();

	/** A group's own row and one of its envelopes are drawn differently, not by two components. */
	const grouped = $derived(row.envelope === "");
	/** The setting this envelope joins to, joined by the category path the plugin mirrors. */
	const setting = $derived(envelopeSetting(settings, row));
	/** The account a fund/rollover directive names for this row. */
	const account = $derived(categoryAccount(settings, row));
	/** The file those directives go into. */
	const file = $derived(envelopeFile(settings, row));
	/**
	 * A policy is dated from its month on. The row control sets it from the
	 * newest visible month: that governs the future the reader is looking at,
	 * rather than rewriting the leftmost column's history.
	 */
	const policyMonth = $derived(months[months.length - 1]?.month ?? "");
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
		<div role="rowheader" class="flex min-w-0 items-center gap-1 px-2 pl-7 text-xs">
			<span class="min-w-0 flex-1 truncate" title={row.account}>{row.envelope}</span>
			{#if policyMonth !== ""}
				<BudgetRolloverControl
					{setting}
					account={account}
					{file}
					month={policyMonth}
					label={account || row.envelope}
					{currency}
				/>
			{/if}
		</div>
	{/if}
	{#each row.cells as cell, index (index)}
		{@const cellMonth = months[index]?.month ?? ""}
		<div role="cell" class="grid grid-cols-3 items-center border-s">
			<div class="min-w-0 px-1 text-right text-xs">
				{#if grouped || cellMonth === ""}
					<BudgetAmount units={cell.budgeted} {currency} />
				{:else}
					<!--
						The Budgeted third of an envelope's cell is the one figure that is
						editable: a click (or Enter, since it is a button) turns it into a
						dated fund draft. The displayed figure stays the plugin's output.
					-->
					<BudgetBudgetedCell
						units={cell.budgeted}
						{currency}
						month={cellMonth}
						account={account}
						{file}
						label={account || row.envelope}
					/>
				{/if}
			</div>
			<div class="overflow-hidden px-1 text-right text-xs">
				{#if row.fullName}
					<!--
						The Spent cell's one action: an envelope's spending opens the
						account it was spent from — the ledger account the envelope mirrors,
						not the envelope's own bookkeeping account, which the register
						hides. The register takes no month filter, so the whole account is
						the honest target.
					-->
					<a
						class="focus-visible:ring-ring rounded-sm underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:outline-none"
						href={resolve("/account/[id]", { id: row.fullName })}
						aria-label={`${row.account} register, ${monthLabel(months[index]?.month ?? "")}`}
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
