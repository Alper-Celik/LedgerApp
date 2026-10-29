<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";
	import { commands } from "../../bindings";
	import { dismissSyncWatch, noteTreeReport, refreshSyncWatch, syncWatch } from "$lib/sync.svelte.js";
	import { openLedgerPath, ledger } from "$lib/ledger.svelte.js";

	/**
	 * The banner for a synced folder that moved on: names how many files did, and
	 * offers the reload. `ledger.summary.path` is the journal this session was
	 * opened with, so reloading means re-opening the same path.
	 */
	/** Settle every conflicted file on one side, then read the ledger again. */
	async function resolve(take: "mine" | "folder") {
		if (syncWatch.reloading) return;
		syncWatch.reloading = true;
		try {
			const result = await commands.resolveTreeConflicts(take);
			if (result.status === "ok") {
				noteTreeReport({ external: result.data.external, conflicts: result.data.conflicts });
			} else {
				syncWatch.error = result.error;
			}
			if (ledger.summary?.path) await openLedgerPath(ledger.summary.path);
		} finally {
			syncWatch.reloading = false;
		}
	}

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

{#if syncWatch.changes.length > 0 || syncWatch.external.length > 0 || syncWatch.conflicts.length > 0 || syncWatch.error !== ""}
	<div
		class="bg-secondary text-secondary-foreground flex flex-wrap items-center gap-2 border-b px-3 py-1.5 text-xs"
		role="status"
	>
		<span class="min-w-0 flex-1 truncate">
			{#if syncWatch.error !== ""}
				Writing this ledger's folder failed — {syncWatch.error}
			{:else if syncWatch.conflicts.length > 0}
				{syncWatch.conflicts.length}
				{syncWatch.conflicts.length === 1 ? "file was" : "files were"} changed here and in the folder; neither
				side was overwritten —
				<span class="text-muted-foreground" title={syncWatch.conflicts.join("\n")}>
					{syncWatch.conflicts[0].split("/").pop()}
					{syncWatch.conflicts.length > 1 ? ` and ${syncWatch.conflicts.length - 1} more` : ""}
				</span>
			{:else if syncWatch.external.length > 0}
				{syncWatch.external.length}
				{syncWatch.external.length === 1 ? "file" : "files"} changed in this folder outside the app —
				<span class="text-muted-foreground" title={syncWatch.external.join("\n")}>
					{syncWatch.external[0].split("/").pop()}
					{syncWatch.external.length > 1 ? ` and ${syncWatch.external.length - 1} more` : ""}
				</span>
			{:else}
				{syncWatch.changes.length}
				{syncWatch.changes.length === 1 ? "file" : "files"} changed in this folder outside the app —
				{@render paths()}
			{/if}
		</span>
		{#if syncWatch.conflicts.length > 0}
			<Button size="sm" onclick={() => resolve("mine")} disabled={syncWatch.reloading}>
				Keep mine
			</Button>
			<Button size="sm" variant="outline" onclick={() => resolve("folder")} disabled={syncWatch.reloading}>
				Take the folder's
			</Button>
		{:else}
			<Button
				size="sm"
				onclick={reload}
				disabled={syncWatch.reloading || (syncWatch.error !== "" && syncWatch.changes.length === 0)}
			>
				{syncWatch.reloading ? "Reloading…" : "Reload ledger"}
			</Button>
		{/if}
		<Button size="sm" variant="ghost" onclick={dismissSyncWatch}>Dismiss</Button>
	</div>
{/if}

{#snippet paths()}
	<span class="text-muted-foreground" title={syncWatch.changes.map((change) => change.path).join("\n")}>
		{syncWatch.changes[0].path.split("/").pop()}
		{syncWatch.changes.length > 1 ? ` and ${syncWatch.changes.length - 1} more` : ""}
	</span>
{/snippet}
