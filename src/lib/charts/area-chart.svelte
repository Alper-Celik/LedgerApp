<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { AreaChart, Tooltip } from "layerchart";
	import { ChartContainer } from "$lib/components/ui/chart/index.js";
	import { cn } from "$lib/utils.js";
	import ChartTooltipBox from "./chart-tooltip-box.svelte";
	import { compactNumber } from "./format.js";
	import { prepareChart, rowLabel, tooltipRows } from "./prepare.js";
	import type { ChartSeries } from "./types.js";

	/**
	 * A filled trend — net worth, a cumulative balance — over months.
	 *
	 * Series overlap rather than stack by default: the point of a net-worth
	 * chart is the height of each line, and stacking one on the other would draw
	 * a total nobody asked for. `stack` is there for a chart whose series really
	 * are parts of a whole.
	 *
	 * Amounts stay exact decimal strings; the area is drawn from floats, but
	 * every number a reader is given comes from `$lib/money.js`. The container
	 * carries `role="img"` and the series in words.
	 */
	let {
		series,
		layout = "overlap",
		tickFormat = compactNumber,
		legend = true,
		class: className,
	}: {
		series: readonly ChartSeries[];
		/** `overlap` draws the series over each other; `stack` adds them up. */
		layout?: "overlap" | "stack";
		/** Value-axis tick label. Defaults to compact numbers (a scale, not an amount). */
		tickFormat?: (value: number) => string;
		legend?: boolean;
		class?: string;
	} = $props();

	const chart = $derived(prepareChart(series, "Area chart"));
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
		<AreaChart
			data={chart.rows}
			x="label"
			series={chart.series}
			seriesLayout={layout}
			{legend}
			props={{
				area: { fillOpacity: 0.28 },
				yAxis: { format: (value: number) => tickFormat(value) },
			}}
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
		</AreaChart>
	{/if}
</ChartContainer>
