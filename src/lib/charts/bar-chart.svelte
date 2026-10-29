<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { BarChart, Tooltip } from "layerchart";
	import { ChartContainer } from "$lib/components/ui/chart/index.js";
	import { cn } from "$lib/utils.js";
	import ChartTooltipBox from "./chart-tooltip-box.svelte";
	import { compactNumber } from "./format.js";
	import { prepareChart, rowLabel, tooltipRows } from "./prepare.js";
	import type { ChartSeries } from "./types.js";

	/**
	 * Bars over months — side by side (`group`) to compare two series month by
	 * month, or stacked to show what a total is made of.
	 *
	 * Amounts stay exact decimal strings: the tooltip and the summary are built
	 * by `$lib/money.js` from the strings, never from the floats the bars are
	 * measured with. The container is the accessible half of the chart — a
	 * `role="img"` with every series in words.
	 */
	let {
		series,
		layout = "group",
		tickFormat = compactNumber,
		legend = true,
		class: className,
	}: {
		series: readonly ChartSeries[];
		/** `group` puts the series side by side in each month; `stack` stacks them. */
		layout?: "group" | "stack";
		/** Value-axis tick label. Defaults to compact numbers (a scale, not an amount). */
		tickFormat?: (value: number) => string;
		legend?: boolean;
		class?: string;
	} = $props();

	const chart = $derived(prepareChart(series, "Bar chart"));
</script>

<ChartContainer
	config={chart.config}
	class={cn("aspect-auto h-56 w-full", className)}
	role="img"
	aria-label={chart.summary}
>
	{#if chart.rows.length === 0}
		<p class="text-muted-foreground grid h-full w-full place-items-center text-xs">No data yet.</p>
	{:else}
		<BarChart
			data={chart.rows}
			x="label"
			series={chart.series}
			seriesLayout={layout}
			{legend}
			props={{ yAxis: { format: (value: number) => tickFormat(value) } }}
		>
			{#snippet tooltip()}
				<Tooltip.Root variant="none">
					{#snippet children({ data })}
						<ChartTooltipBox
							title={rowLabel(data)}
							rows={tooltipRows(data, chart.prepared)}
						/>
					{/snippet}
				</Tooltip.Root>
			{/snippet}
		</BarChart>
	{/if}
</ChartContainer>
