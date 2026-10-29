<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { Snippet } from "svelte";
	import { cn } from "$lib/utils.js";

	/**
	 * One surface on a dashboard: a heading, the registry's one-line
	 * description, and whatever the widget draws below.
	 *
	 * Placement is the page's business — the grid applies the span classes —
	 * so this card only makes sure a widget with little to say still fills its
	 * row rather than shrinking beside a taller neighbour.
	 */
	let {
		title,
		hint = "",
		class: className,
		children,
	}: {
		title: string;
		/** The registry's description, shown under the title. */
		hint?: string;
		class?: string;
		children?: Snippet;
	} = $props();
</script>

<section
	class={cn(
		"flex h-full min-w-0 flex-col gap-2 rounded-xl border border-border bg-card p-3",
		className
	)}
>
	<header class="flex flex-col gap-0.5">
		<h2 class="text-sm font-semibold tracking-tight">{title}</h2>
		{#if hint}
			<p class="text-muted-foreground text-xs">{hint}</p>
		{/if}
	</header>
	{@render children?.()}
</section>
