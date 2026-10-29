<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { Component } from "svelte";
	import type { ChartDatum, ChartSeries } from "$lib/charts/types.js";
	import { chartLoaders, type ChartName } from "$lib/dashboard/index.js";

	/**
	 * The registry's chart wrapper for a widget, loaded only when the widget is
	 * actually drawn.
	 *
	 * A dashboard may hold a chart of a kind the visitor never scrolls to, and
	 * every wrapper drags in the charting library; the registry's loaders keep
	 * that cost off the initial page. `name` is the registry's chart name, so a
	 * widget never names a component directly.
	 */
	let {
		name,
		series = [],
		data = [],
		class: className,
	}: {
		name: ChartName;
		series?: readonly ChartSeries[];
		data?: readonly ChartDatum[];
		class?: string;
	} = $props();

	/**
	 * Every wrapper takes one of these two shapes and ignores the other; the
	 * registry's four entries differ in the prop they read, not in the data
	 * they receive.
	 */
	type ChartProps = {
		series?: readonly ChartSeries[];
		data?: readonly ChartDatum[];
		class?: string;
	};
</script>

{#await chartLoaders[name]()}
	<div class="h-56 w-full animate-pulse rounded-md bg-muted"></div>
{:then loaded}
	{@const Chart = loaded.default as unknown as Component<ChartProps>}
	{#if name === "donut"}
		<Chart {data} class={className} />
	{:else}
		<Chart {series} class={className} />
	{/if}
{:catch}
	<p
		class="text-muted-foreground grid h-56 w-full place-items-center rounded-md border border-dashed border-border px-4 text-center text-xs"
	>
		The chart could not be loaded.
	</p>
{/await}
