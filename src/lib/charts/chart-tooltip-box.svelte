<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import { cn } from "$lib/utils.js";
	import type { TooltipRow } from "./prepare.js";

	/**
	 * The one tooltip body every chart wrapper shows: a heading and one line per
	 * series, with the amount rendered by `MoneyAmount` — the app's single money
	 * renderer, so a tooltip formats no number of its own and the sign colour and
	 * currency emphasis match every other amount in the app.
	 *
	 * It is presentational on purpose: no layerchart context. A wrapper decides
	 * what the pointer is over; this only draws it.
	 */
	let {
		title = "",
		rows,
		class: className,
	}: {
		title?: string;
		rows: readonly TooltipRow[];
		class?: string;
	} = $props();
</script>

<div
	class={cn(
		"grid min-w-32 items-start gap-1.5 rounded-lg border border-border/50 bg-background px-2.5 py-1.5 text-xs shadow-xl",
		className
	)}
>
	{#if title}
		<div class="font-medium">{title}</div>
	{/if}
	<div class="grid gap-1.5">
		{#each rows as row, index (index)}
			<div class="flex w-full items-center justify-between gap-3">
				<span class="flex items-center gap-1.5 text-muted-foreground">
					<span
						class="size-2.5 shrink-0 rounded-[2px]"
						style="background-color: {row.color}"
					></span>
					{row.name}
				</span>
				<MoneyAmount units={row.units} currency={row.currency} />
			</div>
		{/each}
	</div>
</div>
