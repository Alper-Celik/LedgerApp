<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { LineChart, Tooltip } from "layerchart";
	import { ChartContainer } from "$lib/components/ui/chart/index.js";
	import { cn } from "$lib/utils.js";
	import ChartTooltipBox from "./chart-tooltip-box.svelte";
	import { compactNumber } from "./format.js";
	import { prepareChart, rowLabel, tooltipRows } from "./prepare.js";
	import type { ChartSeries } from "./types.js";

	/**
	 * A trend over months, with an optional second series to compare against —
	 * nominal against inflation-adjusted, budgeted against actual.
	 *
	 * Amounts arrive as exact decimal strings and are shown as exact decimal
	 * strings: the tooltip and the summary are built by `$lib/money.js`, and the
	 * float the line is drawn from is never displayed. The container is the
	 * accessible half of the chart — a `role="img"` with the series in words,
	 * because the marks themselves say nothing to a screen reader.
	 */
	let {
		series,
		tickFormat = compactNumber,
		legend = true,
		unit = "month",
		class: className,
	}: {
		series: readonly ChartSeries[];
		/** Value-axis tick label. Defaults to compact numbers (a scale, not an amount). */
		tickFormat?: (value: number) => string;
		/** Series names under the chart; turn off when the widget's title already says it. */
		legend?: boolean;
		/** What one point on the category axis measures, for the accessible summary. */
		unit?: "month" | "day";
		class?: string;
	} = $props();

	const chart = $derived(prepareChart(series, "Line chart", unit));
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
		<LineChart
			data={chart.rows}
			x="label"
			series={chart.series}
			{legend}
			props={{
				spline: { strokeWidth: 2 },
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
		</LineChart>
	{/if}
</ChartContainer>
