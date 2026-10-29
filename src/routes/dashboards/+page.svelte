<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { goto } from "$app/navigation";
	import { resolve } from "$app/paths";
	import DashboardCard from "$lib/components/dashboards/dashboard-card.svelte";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import type { PageProps } from "./$types";

	/**
	 * The dashboards a ledger holds.
	 *
	 * The list is the backend's store rather than a fixed menu: a dashboard is
	 * created from here, edited on its own page, and disappears when deleted.
	 * `New` opens the editor on an id that is not saved yet; nothing is written
	 * until that page's Save.
	 */
	let { data }: PageProps = $props();

	/** Opens the editor on a fresh id. The editor treats an unknown id as a new dashboard. */
	async function create() {
		const id = `dash-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
		await goto(resolve("/dashboards/[id]/edit", { id }));
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<header class="flex flex-wrap items-center gap-2">
		<div class="min-w-0 flex-1">
			<h1 class="text-lg font-semibold tracking-tight">Dashboards</h1>
			<p class="text-muted-foreground text-xs">
				Saved grids of widgets over the loaded ledger.
			</p>
		</div>
		<Button size="lg" class="h-8" onclick={create}>New dashboard</Button>
	</header>

	{#if data.error}
		<Alert variant="destructive">{data.error}</Alert>
	{/if}

	{#if data.dashboards.length > 0}
		<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
			{#each data.dashboards as dashboard (dashboard.id)}
				<DashboardCard {dashboard} />
			{/each}
		</div>
	{:else if !data.error}
		<section
			class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border px-4 py-10 text-center"
		>
			<p class="text-sm font-medium">No dashboards yet</p>
			<p class="text-muted-foreground max-w-sm text-xs">
				A dashboard is a saved layout of widgets — charts, a balances table, the budget's
				figures. Create one and add the widgets you want to see.
			</p>
			<Button size="lg" class="mt-1 h-8" onclick={create}>New dashboard</Button>
		</section>
	{/if}
</div>
