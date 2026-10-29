<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { ChartSpec } from "$lib/assistant/artifacts/types.js";
	import AreaChart from "$lib/charts/area-chart.svelte";
	import BarChart from "$lib/charts/bar-chart.svelte";
	import DonutChart from "$lib/charts/donut-chart.svelte";
	import LineChart from "$lib/charts/line-chart.svelte";
	import type { ChartDatum, ChartSeries } from "$lib/charts/types.js";
	import { cn } from "$lib/utils.js";

	/**
	 * One chart a Python run asked for, drawn inline in the transcript.
	 *
	 * The spec is the sandbox's own wire shape: exact decimal strings and one
	 * currency for the whole chart. The currency is folded back into every point
	 * here, because that is what the chart wrappers read; the wrappers draw the
	 * marks from floats and every number a reader is shown from the strings.
	 * A donut draws the first series as its slices — a share of a total is one
	 * series by nature — while the cartesian kinds draw every series.
	 */
	let { spec, class: className }: { spec: ChartSpec; class?: string } = $props();

	const series = $derived<ChartSeries[]>(
		spec.series.map((one) => ({
			name: one.name,
			points: one.points.map((point) => ({
				label: point.label,
				value: point.value,
				currency: spec.currency,
			})),
		}))
	);

	const slices = $derived<readonly ChartDatum[]>(series[0]?.points ?? []);
	const legend = $derived(series.length > 1);
</script>

<div class={cn("bg-card/60 w-full min-w-0 overflow-hidden rounded-lg border p-2", className)}>
	{#if spec.title}
		<p class="mb-1 truncate px-1 text-xs font-medium">{spec.title}</p>
	{/if}

	{#if spec.kind === "line"}
		<LineChart {series} {legend} />
	{:else if spec.kind === "bar"}
		<BarChart {series} {legend} />
	{:else if spec.kind === "area"}
		<AreaChart {series} {legend} />
	{:else}
		<DonutChart data={slices} {legend} />
	{/if}
</div>
