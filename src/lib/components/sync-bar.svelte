<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";
	import { dismissSyncWatch, refreshSyncWatch, syncWatch } from "$lib/sync.svelte.js";
	import { openLedgerPath, ledger } from "$lib/ledger.svelte.js";

	/**
	 * The banner for a synced folder that moved on: names how many files did, and
	 * offers the reload. `ledger.summary.path` is the journal this session was
	 * opened with, so reloading means re-opening the same path.
	 */
	async function reload() {
		const path = ledger.summary?.path;
		if (!path || syncWatch.reloading) return;
		syncWatch.reloading = true;
		try {
			await openLedgerPath(path);
			await refreshSyncWatch();
		} finally {
			syncWatch.reloading = false;
		}
	}
</script>

{#if syncWatch.changes.length > 0}
	<div
		class="bg-secondary text-secondary-foreground flex flex-wrap items-center gap-2 border-b px-3 py-1.5 text-xs"
		role="status"
	>
		<span class="min-w-0 flex-1 truncate">
			{syncWatch.changes.length}
			{syncWatch.changes.length === 1 ? "file" : "files"} changed in this folder outside the app —
			{@render paths()}
		</span>
		<Button size="sm" onclick={reload} disabled={syncWatch.reloading}>
			{syncWatch.reloading ? "Reloading…" : "Reload ledger"}
		</Button>
		<Button size="sm" variant="ghost" onclick={dismissSyncWatch}>Dismiss</Button>
	</div>
{/if}

{#snippet paths()}
	<span class="text-muted-foreground" title={syncWatch.changes.map((change) => change.path).join("\n")}>
		{syncWatch.changes[0].path.split("/").pop()}
		{syncWatch.changes.length > 1 ? ` and ${syncWatch.changes.length - 1} more` : ""}
	</span>
{/snippet}
