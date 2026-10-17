<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { dashboards } from "$lib/dashboards.js";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();

	/**
	 * The entry this route was asked for: the heading and the component to
	 * render, both read from the registry rather than carried through the load,
	 * which only ever holds data.
	 */
	const entry = $derived(dashboards.find((dashboard) => dashboard.id === data.id));
</script>

<div class="flex flex-col gap-4 p-4">
	{#if !data.known || !entry}
		<div class="flex flex-wrap items-center gap-2">
			<h1 class="text-lg font-semibold tracking-tight">No dashboard “{data.id}”</h1>
		</div>
		<p class="text-muted-foreground text-sm">
			Nothing in the dashboard registry answers to <code>{data.id}</code>.
			<a class="underline underline-offset-2" href={resolve("/dashboards")}>All dashboards</a>
		</p>
	{:else}
		<div class="flex flex-wrap items-center gap-2">
			<HugeiconsIcon icon={entry.icon} strokeWidth={2} class="size-4 text-muted-foreground" />
			<h1 class="text-lg font-semibold tracking-tight">{entry.title}</h1>
			<span class="text-muted-foreground text-xs">{entry.blurb}</span>
		</div>

		{#if data.error}
			<Alert variant="destructive">{data.error}</Alert>
		{:else}
			{@const Dashboard = entry.component}
			<Dashboard data={data.data} />
		{/if}
	{/if}
</div>
