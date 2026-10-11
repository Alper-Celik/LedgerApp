<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { invalidateAll } from "$app/navigation";
	import { resolve } from "$app/paths";
	import { Button } from "$lib/components/ui/button/index.js";
	import GitDiff from "$lib/components/git-diff.svelte";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Diff</h1>
		<span class="text-muted-foreground text-xs">ledger changes vs HEAD, staged and unstaged</span>
		<div class="ml-auto flex items-center gap-1">
			<Button size="sm" variant="ghost" onclick={() => invalidateAll()}>Refresh</Button>
			<Button size="sm" variant="secondary" href={resolve("/commit")}>Commit…</Button>
		</div>
	</div>

	{#if data.error}
		<p class="text-sm text-red-600 dark:text-red-400">{data.error}</p>
	{:else if data.diff.trim() === ""}
		<p class="text-muted-foreground text-sm">No uncommitted changes.</p>
	{:else}
		<GitDiff diff={data.diff} />
	{/if}
</div>
