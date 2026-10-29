<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { cn } from "$lib/utils.js";

	/**
	 * What a widget says when it has nothing honest to draw.
	 *
	 * A missing report, an empty ledger and a filter that matches no accounts are
	 * different facts, so the caller supplies both lines; this only keeps them
	 * the same size as a chart, so the grid does not jump while some widgets are
	 * still waiting for data.
	 */
	let {
		title,
		detail = "",
		class: className,
	}: {
		title: string;
		detail?: string;
		class?: string;
	} = $props();
</script>

<div
	class={cn(
		"flex h-56 flex-col items-center justify-center gap-1 rounded-md border border-dashed border-border px-4 text-center",
		className
	)}
>
	<p class="text-xs font-medium">{title}</p>
	{#if detail}
		<p class="text-muted-foreground max-w-sm text-xs">{detail}</p>
	{/if}
</div>
