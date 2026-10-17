<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import type { IconSvgElement } from "@hugeicons/svelte";
	import type { Snippet } from "svelte";
	import { cn } from "$lib/utils.js";

	/**
	 * The one surface the dashboards are built from: a bordered card with a
	 * heading, an optional icon and hint beside it, and the caller's content
	 * below. It is a box and stays one — a dashboard that needs two of them uses
	 * it twice, and nothing about the layout of a dashboard lives in here.
	 */
	let {
		title,
		hint = "",
		icon = null,
		class: className,
		children,
	}: {
		title: string;
		hint?: string;
		/** Shown before the title; the list page's cards carry the registry's icon. */
		icon?: IconSvgElement | null;
		class?: string;
		children?: Snippet;
	} = $props();
</script>

<section class={cn("flex flex-col gap-2 rounded-xl border border-border bg-card p-3", className)}>
	<div class="flex flex-wrap items-center gap-x-2 gap-y-1">
		{#if icon}
			<HugeiconsIcon {icon} strokeWidth={2} class="size-4 shrink-0 text-muted-foreground" />
		{/if}
		<h2 class="text-sm font-semibold tracking-tight">{title}</h2>
		{#if hint}
			<span class="text-muted-foreground text-xs">{hint}</span>
		{/if}
	</div>
	{@render children?.()}
</section>
