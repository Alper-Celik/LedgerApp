<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { ChevronDownIcon, ChevronRightIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import type { LifestyleReport } from "../../../bindings";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import {
		nominalChange,
		outpaces,
		percentText,
		realChange,
	} from "$lib/components/inflation/inflation.js";
	import { monthLabel } from "$lib/budget.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { cn } from "$lib/utils.js";

	/**
	 * What each expense category did over the report's range: the newest month's
	 * spending nominal and in the first month's money, and how far each moved —
	 * from the range's first month to its last, which is the same span the
	 * index's own change covers, so the two percentages are comparable.
	 *
	 * The report also carries a month-over-month change per category; it is not
	 * shown here because it answers a different question (the latest month
	 * against the one before it) and would sit beside a range percentage as if
	 * the two measured the same thing.
	 *
	 * A category whose real spending grew faster than prices is red: the money
	 * bought more, not less, and that is the fact this screen exists to show.
	 */
	let { report }: { report: LifestyleReport } = $props();

	/** Rows opened to their months, by account. */
	let expanded = $state<Record<string, boolean>>({});

	const rows = $derived(
		report.categories.map((category) => {
			const real = realChange(category.months);
			return {
				category,
				latest: category.months.at(-1) ?? null,
				real,
				nominal: nominalChange(category.months),
				faster: outpaces(real, report.index.change),
			};
		})
	);

	const newest = $derived(report.months.at(-1)?.month ?? "");
</script>

{#if rows.length === 0}
	<p class="text-muted-foreground text-xs">
		No expense category shows spending in this range, so there is nothing to compare against the
		index.
	</p>
{:else}
	<div class="flex flex-col">
		<!--
			The column names, on a wide screen only: the stacked phone layout
			labels every value with what it is, so a header row would sit above
			nothing it lines up with.
		-->
		<div
			class="text-muted-foreground hidden items-center gap-2 border-b px-3 py-1.5 text-xs sm:grid sm:grid-cols-[minmax(0,1fr)_7rem_7rem_6rem_6rem]"
		>
			<span>Category</span>
			<span class="text-right" title={newest ? `Spending in ${monthLabel(newest)}` : undefined}>
				Latest nominal
			</span>
			<span class="text-right" title="The same spending in the first month's money">
				Latest real
			</span>
			<span class="text-right" title="First month to last month of the range">
				Change nominal
			</span>
			<span class="text-right" title="First month to last month of the range, in the first month's money">
				Change real
			</span>
		</div>

		<ul class="divide-border divide-y">
			{#each rows as row (row.category.account)}
				{@const open = expanded[row.category.account] === true}
				<li class="grid grid-cols-2 items-baseline gap-x-3 gap-y-1 px-3 py-2 sm:grid-cols-[minmax(0,1fr)_7rem_7rem_6rem_6rem] sm:items-center sm:gap-2">
					<div class="col-span-full flex min-w-0 items-center gap-1 sm:col-span-1">
						<Button
							size="icon-sm"
							variant="ghost"
							class="-ms-1 size-6 shrink-0"
							aria-expanded={open}
							aria-controls={`months-${row.category.account}`}
							aria-label={`${open ? "Hide" : "Show"} the months of ${row.category.label}`}
							onclick={() => (expanded[row.category.account] = !open)}
						>
							<HugeiconsIcon
								icon={open ? ChevronDownIcon : ChevronRightIcon}
								strokeWidth={2}
							/>
						</Button>
						<span class="min-w-0 truncate font-medium" title={row.category.account}>
							{row.category.label}
						</span>
					</div>

					<div class="flex flex-wrap items-baseline gap-x-1 sm:block sm:text-right">
						<span class="text-muted-foreground text-[0.625rem] sm:hidden">Latest</span>
						{#if row.latest}
							<MoneyAmount
								units={row.latest.nominal}
								currency={row.category.currency}
								signed={false}
								class="text-foreground"
							/>
						{:else}
							<span class="text-muted-foreground">—</span>
						{/if}
					</div>

					<div class="flex flex-wrap items-baseline gap-x-1 sm:block sm:text-right">
						<span class="text-muted-foreground text-[0.625rem] sm:hidden">Real</span>
						{#if row.latest}
							<MoneyAmount
								units={row.latest.real}
								currency={row.category.currency}
								signed={false}
								class="text-foreground"
							/>
						{:else}
							<span class="text-muted-foreground">—</span>
						{/if}
					</div>

					<div class="flex flex-wrap items-baseline gap-x-1 sm:block sm:text-right">
						<span class="text-muted-foreground text-[0.625rem] sm:hidden">Δ nominal</span>
						<span class="tabular-nums">{percentText(row.nominal)}</span>
					</div>

					<div class="flex flex-wrap items-baseline gap-x-1 sm:block sm:text-right">
						<span class="text-muted-foreground text-[0.625rem] sm:hidden">Δ real</span>
						<span
							class={cn(
								"tabular-nums",
								row.faster && "font-medium text-red-600 dark:text-red-400"
							)}
							title={row.faster
								? "Real spending grew faster than the index — more was bought, not only more paid"
								: undefined}
						>
							{percentText(row.real)}
						</span>
					</div>

					{#if open}
						<!--
							The category's own months, month by month, in the same two
							measures: what was spent, and what that spending would have
							bought in the first month's money.
						-->
						<ul
							id={`months-${row.category.account}`}
							class="col-span-full flex flex-col gap-0.5 border-s-2 ps-3 sm:ms-3"
						>
							{#each row.category.months as month (month.month)}
								<li
									class="grid grid-cols-2 gap-x-3 gap-y-0.5 text-xs sm:grid-cols-[minmax(0,1fr)_7rem_7rem]"
								>
									<span class="text-muted-foreground">{monthLabel(month.month)}</span>
									<span class="flex flex-wrap items-baseline gap-x-1 sm:justify-end">
										<span class="text-muted-foreground text-[0.625rem] sm:hidden">Nominal</span>
										<MoneyAmount
											units={month.nominal}
											currency={row.category.currency}
											signed={false}
											class="text-foreground"
										/>
									</span>
									<span class="flex flex-wrap items-baseline gap-x-1 sm:justify-end">
										<span class="text-muted-foreground text-[0.625rem] sm:hidden">Real</span>
										<MoneyAmount
											units={month.real}
											currency={row.category.currency}
											signed={false}
											class="text-foreground"
										/>
									</span>
								</li>
							{/each}
						</ul>
					{/if}
				</li>
			{/each}
		</ul>
	</div>

	<p class="text-muted-foreground text-[0.625rem]">
		Changes run from the range's first month to its last, in the first month's money. Red means
		real spending grew faster than the index.
	</p>
{/if}
