<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { figureClass, type FigureTone } from "$lib/budget.js";
	import { moneySegments, signOf } from "$lib/money.js";
	import { cn } from "$lib/utils.js";

	let {
		units,
		currency,
		tone = "plain",
		showSymbol = false,
		class: className,
	}: {
		/** The exact decimal string the backend reported; nothing is parsed or rounded here. */
		units: string;
		currency: string;
		tone?: FigureTone;
		/**
		 * Print the currency symbol or code here. The grid states the commodity
		 * once in its summary strip instead of in every cell; a figure that
		 * stands on its own (a month's To budget) carries it.
		 */
		showSymbol?: boolean;
		class?: string;
	} = $props();

	/**
	 * The digits and commodity runs, placed by `moneySegments` — the budget grid
	 * omits the "+" a positive would carry, since colour and the column heading
	 * say the direction; a minus stays, so a figure is never read as its
	 * magnitude where colour alone would not carry it.
	 */
	const segments = $derived.by(() => {
		const negative = signOf(units) < 0;
		const rendered = moneySegments(units, currency, { signed: false }).filter(
			(segment) => showSymbol || segment.kind !== "symbol"
		);
		return negative && rendered.length > 0
			? rendered.map((segment, index) =>
					index === 0 ? { ...segment, text: `-${segment.text}` } : segment
				)
			: rendered;
	});
</script>

<span
	class={cn(
		"inline-flex items-baseline tabular-nums whitespace-nowrap",
		figureClass(units, tone),
		className
	)}
	data-tone={tone}
>
	{#each segments as segment, index (index)}
		<span class={cn(segment.kind === "symbol" && "text-foreground font-semibold")}>{segment.text}</span>
	{/each}
</span>
