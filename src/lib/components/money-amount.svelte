<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { amountColorClass } from "$lib/accounts.js";
	import { moneySegments } from "$lib/money.js";
	import { cn } from "$lib/utils.js";

	let {
		units,
		currency,
		signed = true,
		class: className,
	}: {
		units: string | null;
		currency: string;
		/** `false` renders the magnitude with no +/- (column implies direction). */
		signed?: boolean;
		class?: string;
	} = $props();
</script>

<!--
	Amounts render the same way everywhere: digits in the sign colour, the
	currency symbol or commodity code in its own brighter, heavier run so ₺/€/$
	don't blur into the number. `moneySegments` decides the order and spacing.
-->
<span class={cn("inline-flex items-baseline tabular-nums", amountColorClass(units), className)}>
	{#each moneySegments(units, currency, { signed }) as segment, index (index)}
		<span class={cn(segment.kind === "symbol" && "font-semibold text-foreground")}>
			{segment.text}
		</span>
	{/each}
</span>
