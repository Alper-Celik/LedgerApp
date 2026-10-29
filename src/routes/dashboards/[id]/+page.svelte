<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import WidgetView from "$lib/components/dashboards/widget-view.svelte";
	import { prepareWidgets } from "$lib/components/dashboards/data.js";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { GRID_CLASS, spanClasses } from "$lib/dashboard/index.js";
	import type { PageProps } from "./$types";

	/**
	 * One dashboard, rendered read-only.
	 *
	 * The page owns the grid and the data; a widget owns neither. Widgets are
	 * placed with the registry layout's span classes, and an unknown kind draws a
	 * card that says so rather than taking the page down. Editing lives one route
	 * over (`/dashboards/[id]/edit`), so nothing here can move a widget by
	 * accident.
	 */
	let { data }: PageProps = $props();

	const widgets = $derived(data.dashboard ? prepareWidgets(data.dashboard) : []);
</script>

<div class="flex flex-col gap-4 p-4">
	<header class="flex flex-wrap items-start gap-2">
		<div class="min-w-0 flex-1">
			<h1 class="truncate text-lg font-semibold tracking-tight">
				{data.dashboard?.name || "Dashboard"}
			</h1>
			<p class="text-muted-foreground text-xs">
				{widgets.length}
				{widgets.length === 1 ? "widget" : "widgets"} · saved with the ledger
			</p>
		</div>
		<div class="flex flex-wrap gap-2">
			<Button variant="outline" size="lg" class="h-8" href={resolve("/dashboards")}>
				All dashboards
			</Button>
			<Button size="lg" class="h-8" href={resolve("/dashboards/[id]/edit", { id: data.id })}>
				Edit
			</Button>
		</div>
	</header>

	{#if data.loadError}
		<Alert variant="destructive">{data.loadError}</Alert>
	{/if}

	{#if !data.dashboard}
		<section
			class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border px-4 py-10 text-center"
		>
			<p class="text-sm font-medium">No dashboard “{data.id}”</p>
			<p class="text-muted-foreground max-w-sm text-xs">
				No saved dashboard answers to that id. Creating one edits an empty dashboard under it —
				nothing is written until you save.
			</p>
			<div class="mt-1 flex flex-wrap justify-center gap-2">
				<Button variant="outline" size="lg" class="h-8" href={resolve("/dashboards")}>
					All dashboards
				</Button>
				<Button
					size="lg"
					class="h-8"
					href={resolve("/dashboards/[id]/edit", { id: data.id })}
				>
					Create it
				</Button>
			</div>
		</section>
	{:else if widgets.length === 0}
		<section
			class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border px-4 py-10 text-center"
		>
			<p class="text-sm font-medium">This dashboard has no widgets yet</p>
			<p class="text-muted-foreground max-w-sm text-xs">
				Add charts and tables from the editor; its kind picker lists everything this build can
				draw.
			</p>
			<Button
				size="lg"
				class="mt-1 h-8"
				href={resolve("/dashboards/[id]/edit", { id: data.id })}
			>
				Edit dashboard
			</Button>
		</section>
	{:else}
		<div class={GRID_CLASS}>
			{#each widgets as widget, index (`${widget.id}:${index}`)}
				<div class={spanClasses(widget.span)}>
					<WidgetView {widget} data={data.sources} />
				</div>
			{/each}
		</div>
	{/if}
</div>
