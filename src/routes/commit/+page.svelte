<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { invalidateAll } from "$app/navigation";
	import { commands } from "../../bindings";
	import { Button } from "$lib/components/ui/button/index.js";
	import GitDiff from "$lib/components/git-diff.svelte";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();

	let message = $state("");
	let committing = $state(false);
	let error = $state("");
	let committed = $state("");
	/** Added/removed line counts, ignoring the diff's own header lines. */
	let counts = $derived(
		data.diff.split("\n").reduce(
			(acc: { added: number; removed: number }, line: string) => {
				if (/^\+\+\+|^---/.test(line)) return acc;
				if (line.startsWith("+")) acc.added += 1;
				else if (line.startsWith("-")) acc.removed += 1;
				return acc;
			},
			{ added: 0, removed: 0 }
		)
	);
	let changed = $derived(data.diff.trim() !== "");

	async function commit(event: Event) {
		event.preventDefault();
		committing = true;
		error = "";
		const result = await commands.commitLedger(message);
		committing = false;
		if (result.status === "error") {
			error = result.error;
			return;
		}
		committed = result.data;
		message = "";
		bumpLedgerRevision();
		await invalidateAll();
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Commit</h1>
		<span class="text-muted-foreground text-xs">
			{changed ? `+${counts.added} −${counts.removed} lines staged` : "nothing staged"}
		</span>
	</div>

	<form class="flex max-w-2xl flex-col gap-2" onsubmit={commit}>
		<textarea
			class="border-input bg-background min-h-20 rounded-md border px-3 py-2 font-mono text-xs"
			placeholder="Commit message"
			bind:value={message}
			aria-label="Commit message"
		></textarea>
		<div class="flex items-center gap-2">
			<Button size="sm" type="submit" disabled={!changed || message.trim() === "" || committing}>
				Commit staged changes
			</Button>
			{#if committed}
				<span class="text-muted-foreground text-xs">committed {committed}</span>
			{/if}
		</div>
		{#if error}
			<p class="text-xs text-red-600 dark:text-red-400">{error}</p>
		{/if}
	</form>

	{#if changed}
		<GitDiff diff={data.diff} />
	{:else}
		<p class="text-muted-foreground text-sm">No uncommitted changes.</p>
	{/if}
</div>
