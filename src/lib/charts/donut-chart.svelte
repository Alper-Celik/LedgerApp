<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { PieChart, Text, Tooltip } from "layerchart";
	import { ChartContainer } from "$lib/components/ui/chart/index.js";
	import { cn } from "$lib/utils.js";
	import ChartTooltipBox from "./chart-tooltip-box.svelte";
	import { moneyText } from "./format.js";
	import { prepareShare, shareTooltipRows } from "./prepare.js";
	import type { ChartDatum } from "./types.js";

	/**
	 * A category's share of a total, as a donut with the exact total in its middle.
	 *
	 * One currency per chart, non-negative amounts only: a share is only a share
	 * while that holds, and a refund drawn beside spending would not be one. The
	 * total in the center is summed from the decimal strings, so it is exact;
	 * the arcs are sized from floats, which is all a slice angle ever is.
	 */
	let {
		data,
		totalLabel = "Total",
		legend = true,
		class: className,
	}: {
		/** One amount per slice; slices are drawn in the order given. */
		data: readonly ChartDatum[];
		/** What the number in the middle counts, written under it. */
		totalLabel?: string;
		legend?: boolean;
		class?: string;
	} = $props();

	const share = $derived(prepareShare(data));
</script>

<ChartContainer
	config={share.config}
	class={cn("mx-auto aspect-square w-full max-w-56", className)}
	role="img"
	aria-label={share.summary}
>
	{#if share.rows.length === 0}
		<p class="text-muted-foreground grid h-full w-full place-items-center text-xs">No data yet.</p>
	{:else}
		<PieChart
			data={share.rows}
			key="label"
			label="label"
			value="value"
			c="color"
			innerRadius={0.62}
			padding={2}
			{legend}
		>
			{#snippet aboveMarks()}
				<Text
					value={moneyText(share.total.value, share.total.currency)}
					textAnchor="middle"
					verticalAnchor="middle"
					class="fill-foreground text-base! font-semibold"
					dy={-2}
				/>
				<Text
					value={totalLabel}
					textAnchor="middle"
					verticalAnchor="middle"
					class="fill-muted-foreground text-xs!"
					dy={14}
				/>
			{/snippet}
			{#snippet tooltip()}
				<Tooltip.Root variant="none">
					{#snippet children({ data })}
						<ChartTooltipBox rows={shareTooltipRows(data)} />
					{/snippet}
				</Tooltip.Root>
			{/snippet}
		</PieChart>
	{/if}
</ChartContainer>
