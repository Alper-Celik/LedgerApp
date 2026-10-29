<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { IndexSeries, LifestyleReport } from "../../../bindings";
	import CategoryTable from "$lib/components/inflation/category-table.svelte";
	import {
		RANGES,
		percentText,
		outpaces,
		rangeLabel,
		rangeText,
		spendingRealChange,
	} from "$lib/components/inflation/inflation.js";
	import LineChart from "$lib/charts/line-chart.svelte";
	import { monthLabel } from "$lib/budget.js";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import { cn } from "$lib/utils.js";

	/**
	 * The lifestyle comparison: the picker for which index restates the money
	 * and how many months to walk back, the report those choices run to, and
	 * what it says — a line of monthly totals nominal against real, the index's
	 * own movement set beside the spending's, and one row per expense category.
	 *
	 * Everything shown is the report's own figures; the one arithmetic this panel
	 * does is the pair of changes it sets side by side — the index's, which the
	 * report returns, and the spending's, worked out from the report's monthly
	 * totals over the same first-to-last span with exact decimal integers.
	 */
	let {
		series,
		report,
		indexId,
		months,
		busy = false,
		error = "",
		onselect,
		onmonths,
	}: {
		series: IndexSeries[];
		/** The report in hand, or `null` before one has been run or after it failed. */
		report: LifestyleReport | null;
		/** The series the report is for (or the one a pick would run). */
		indexId: string;
		/** The range the report covers, in months. */
		months: number;
		/** A report is in flight; the controls and the old figures wait for it. */
		busy?: boolean;
		/** The backend's refusal of the last run, verbatim. */
		error?: string;
		onselect: (id: string) => void;
		onmonths: (months: number) => void;
	} = $props();

	const selected = $derived(series.find((one) => one.id === indexId) ?? null);
	const newest = $derived(report?.months.at(-1)?.month ?? "");
	const oldest = $derived(report?.months[0]?.month ?? "");
	const base = $derived(oldest === "" ? "the first month" : monthLabel(oldest));

	/** The spending's own real movement over the range, to set beside the index's. */
	const spending = $derived(report ? spendingRealChange(report) : null);
	const faster = $derived(outpaces(spending, report?.index.change ?? null));

	/** Monthly totals, nominal and in the range's first month's money. */
	const chart = $derived(
		report
			? [
					{
						name: "Nominal",
						points: report.totals.map((month) => ({
							label: month.month,
							value: month.nominal,
							currency: month.currency,
						})),
					},
					{
						name: `Real (${base} money)`,
						points: report.totals.map((month) => ({
							label: month.month,
							value: month.real,
							currency: month.currency,
						})),
					},
				]
			: []
	);
</script>

<div class="flex flex-col gap-3" aria-busy={busy}>
	{#if series.length === 0}
		<p class="text-muted-foreground text-xs">
			No index is stored yet — import or refresh an index first.
		</p>
	{:else}
		<div class="flex flex-wrap items-end gap-x-3 gap-y-2">
			<div class="flex min-w-0 flex-col gap-0.5">
				<span class="text-muted-foreground text-xs">Index</span>
				<Select.Root
					type="single"
					value={indexId}
					onValueChange={(next) => next && onselect(next)}
				>
					<Select.Trigger
						size="sm"
						class="max-w-56"
						aria-label="Price index to compare against"
						disabled={busy}
					>
						<span data-slot="select-value" class="min-w-0 truncate">
							{selected?.label ?? (indexId || "Choose an index")}
						</span>
					</Select.Trigger>
					<Select.Content>
						{#each series as one (one.id)}
							<Select.Item
								value={one.id}
								label={one.label}
								title={one.unofficial ? `${one.source} — unofficial` : one.source}
							/>
						{/each}
					</Select.Content>
				</Select.Root>
			</div>

			<div class="flex flex-col gap-0.5">
				<span class="text-muted-foreground text-xs">Range</span>
				<ToggleGroup.Root
					type="single"
					value={String(months)}
					aria-label="Months to compare"
					onValueChange={(next) => {
						const count = Number(next);
						if (next && Number.isFinite(count) && count !== months) onmonths(count);
					}}
				>
					{#each RANGES as range (range)}
						<ToggleGroup.Item value={String(range)} disabled={busy}>
							{rangeLabel(range)}
						</ToggleGroup.Item>
					{/each}
				</ToggleGroup.Root>
			</div>

			{#if busy}
				<span class="text-muted-foreground ms-auto text-xs">recounting…</span>
			{/if}
		</div>

		{#if error}
			<Alert variant="destructive">{error}</Alert>
		{/if}

		{#if selected && selected.points.length === 0}
			<p class="text-muted-foreground text-xs">
				<code class="font-mono">{selected.id}</code> holds no points, so there is no index value to
				restate the spending with. Refresh it, or import its months as CSV.
			</p>
		{:else if report}
			<div class={cn("flex flex-col gap-3", busy && "opacity-60")}>
				<div class="flex flex-col gap-0.5">
					<p class="text-sm">
						prices
						<span class="font-semibold">{percentText(report.index.change)}</span>
						vs your spending
						<span class={cn("font-semibold", faster && "text-red-600 dark:text-red-400")}>
							{percentText(spending)}
						</span>
						real
					</p>
					<p class="text-muted-foreground flex flex-wrap items-center gap-x-1.5 text-xs">
						<span>{report.index.label}</span>
						{#if selected?.unofficial}
							<Badge
								variant="destructive"
								title="Published by an independent body, not a national statistical agency"
							>
								unofficial
							</Badge>
						{/if}
						<span>
							over <span class="font-mono">{rangeText(report)}</span>; real figures are in {base}
							money. The line below is every month of the range, the table only its
							newest{newest ? ` (${monthLabel(newest)})` : ""}.
						</span>
					</p>
				</div>

				<LineChart series={chart} />

				<CategoryTable {report} />
			</div>
		{:else}
			<!--
				No report in hand. When the load or a run refused one, its own
				sentence is the alert above (or the page's); this only says what to
				do next.
			-->
			<p class="text-muted-foreground text-xs">
				No report is loaded for this index and range yet — pick a series or a range above to run
				one.
			</p>
		{/if}
	{/if}
</div>
