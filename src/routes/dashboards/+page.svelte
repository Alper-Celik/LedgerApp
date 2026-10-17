<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import Panel from "$lib/components/dashboards/panel.svelte";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Dashboards</h1>
		<span class="text-muted-foreground text-xs">read-only views of the loaded ledger</span>
	</div>

	<!--
		One card per registry entry, drawn from the array rather than from a list
		written here: a dashboard that is added to the registry shows up without
		this file changing.
	-->
	<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
		{#each data.dashboards as dashboard (dashboard.id)}
			<a
				class="transition-motion block rounded-xl outline-none focus-visible:ring-2 focus-visible:ring-ring"
				href={resolve("/dashboards/[id]", { id: dashboard.id })}
				data-dashboard-id={dashboard.id}
			>
				<Panel
					title={dashboard.title}
					icon={dashboard.icon}
					class="h-full transition-motion hover:border-primary/40 hover:bg-accent/40"
				>
					<p class="text-muted-foreground text-xs">{dashboard.blurb}</p>
				</Panel>
			</a>
		{/each}
	</div>

	<!--
		The extension point, said where someone looking at the running app can
		find it: the list above is `src/lib/dashboards.ts`, and that array is all
		a route reads.
	-->
	<Panel title="Adding a dashboard" hint="one entry, one component">
		<p class="text-muted-foreground text-xs">
			Every card above is one entry in <code>src/lib/dashboards.ts</code>: an id, a title, a blurb,
			an icon and the component that draws it. This page and
			<code>/dashboards/&lt;id&gt;</code> read that array and handle no dashboard by name, so a new
			one is one entry plus one file under <code>src/lib/components/dashboards/</code>. A dashboard
			with its own data source gives its entry a <code>load</code> that reads whatever command it
			needs; without one it is handed the ledger's accounts and transactions.
		</p>
	</Panel>
</div>
