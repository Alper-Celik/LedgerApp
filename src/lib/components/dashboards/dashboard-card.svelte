<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { Button } from "$lib/components/ui/button/index.js";
	import { isWidgetSpan } from "$lib/dashboard/index.js";
	import type { Dashboard } from "../../../bindings";

	/**
	 * One saved dashboard in the list: its name, how many widgets it holds, and a
	 * miniature of the grid those widgets make.
	 *
	 * The miniature is drawn from the saved spans — a full-width widget is a bar
	 * across all four columns — so the list says what a dashboard looks like
	 * without rendering any of its charts.
	 */
	let { dashboard }: { dashboard: Dashboard } = $props();

	const widgets = $derived([...dashboard.widgets].sort((left, right) => left.order - right.order));
	const count = $derived(widgets.length);
</script>

<section class="flex h-full flex-col gap-3 rounded-xl border border-border bg-card p-3">
	<div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
		<div class="min-w-0">
			<h2 class="truncate text-sm font-semibold tracking-tight">
				{dashboard.name || "Untitled dashboard"}
			</h2>
			<p class="text-muted-foreground text-xs">
				{count}
				{count === 1 ? "widget" : "widgets"}
			</p>
		</div>
		<Button
			variant="outline"
			size="lg"
			class="h-8 shrink-0"
			href={resolve("/dashboards/[id]", { id: dashboard.id })}
		>
			Open
		</Button>
	</div>

	{#if count === 0}
		<p
			class="text-muted-foreground flex h-12 items-center justify-center rounded-md border border-dashed border-border text-xs"
		>
			No widgets yet
		</p>
	{:else}
		<div class="grid grid-cols-4 gap-1" aria-hidden="true">
			{#each widgets as widget, index (`${widget.id}:${index}`)}
				<span
					class="h-1.5 rounded-full bg-primary/40"
					style:grid-column={`span ${isWidgetSpan(widget.span) ? widget.span : 1}`}
				></span>
			{/each}
		</div>
	{/if}
</section>
