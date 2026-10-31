<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";
	import { commands } from "../../bindings";
	import { dismissSyncWatch, noteTreeReport, refreshSyncWatch, syncWatch } from "$lib/sync.svelte.js";
	import { openLedgerPath, ledger } from "$lib/ledger.svelte.js";
	import { git } from "$lib/git.svelte.js";

	/**
	 * Whether the bar has anything of its own to say. In a git repository it
	 * never does: git owns the ledger's versions there, and its merge conflicts
	 * live in the merge view. The backend already answers both watch commands
	 * empty in that mode; this keeps the bar silent even if a report left over
	 * from the ledger opened before is still in the store (`leaveTreeMode` clears
	 * it, but the gate must not depend on that call having run first).
	 */
	const shows = $derived(
		!git.repo &&
			(syncWatch.changes.length > 0 ||
				syncWatch.external.length > 0 ||
				syncWatch.conflicts.length > 0 ||
				syncWatch.treeError !== "" ||
				syncWatch.error !== "")
	);

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

{#if shows}
	<div
		class="bg-secondary text-secondary-foreground flex flex-wrap items-center gap-2 border-b px-3 py-1.5 text-xs"
		role="status"
	>
		<span class="min-w-0 flex-1 truncate">
			{#if syncWatch.treeError !== ""}
				Writing this ledger's folder failed — {syncWatch.treeError}
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
			{:else if syncWatch.error !== ""}
				This folder cannot be read — {syncWatch.error}
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
				disabled={
					syncWatch.reloading ||
					(syncWatch.treeError !== "" && syncWatch.changes.length === 0)
				}
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
