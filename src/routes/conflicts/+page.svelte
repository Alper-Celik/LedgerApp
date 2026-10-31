<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { resolve } from "$app/paths";
	import { commands, type Report } from "../../bindings";
	import { byClient, clientLabel, nameOf } from "$lib/components/conflicts/conflicts.js";
	import ConflictResolver from "$lib/components/conflicts/conflict-resolver.svelte";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { messageOf } from "$lib/errors.js";
	import { git, refreshGit } from "$lib/git.svelte.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import type { PageProps } from "./$types";

	/**
	 * The ledger folder's conflict copies, grouped by the client that named them.
	 *
	 * The list is a scan, not a merge: a row says which two files the sync client
	 * left side by side and when the copy carries a stamp, and Review opens the
	 * resolver in place. A copy whose original is gone is listed apart and not
	 * offered for merge — the resolver would have to invent the file it merged
	 * into.
	 */
	let { data }: PageProps = $props();

	/** The route's own load, taken once to seed the screen. */
	const first = untrack(() => data);
	/** The load the screen's state was last taken from (plain: see `branches`). */
	let adopted = first;

	let report = $state<Report | null>(first.report);
	let error = $state(first.error);
	/** The backend's sentence after a successful resolve. */
	let notice = $state("");
	let refreshing = $state(false);
	/** The copy whose resolver is open, by path. */
	let reviewing = $state<string | null>(null);

	// The scan answers empty in a git repository (git owns that ledger's
	// versions), so the screen has to say why rather than claim the folder has
	// no copies. `git.repo` is the same answer Settings and the merge view use.
	void refreshGit();

	$effect(() => {
		// A scan belongs to one folder: after another ledger is opened (which
		// re-runs this route's load) the previous folder's conflict copies — and a
		// resolver opened on one of them — are not this ledger's.
		if (data === adopted) return;
		adopted = data;
		report = data.report;
		error = data.error;
		notice = "";
		reviewing = null;
	});

	const groups = $derived(byClient(report?.candidates ?? []));
	const orphans = $derived(report?.orphans ?? []);
	const empty = $derived(
		report !== null && report.candidates.length === 0 && report.orphans.length === 0
	);

	async function refresh() {
		refreshing = true;
		error = "";
		try {
			const result = await commands.syncConflicts();
			if (result.status === "error") {
				error = result.error;
				return;
			}
			report = result.data;
		} catch (cause) {
			console.error("syncConflicts failed", cause);
			error = messageOf(cause);
		} finally {
			refreshing = false;
		}
	}

	/** A resolved copy is gone from the list, and the ledger behind it changed. */
	async function resolved(message: string) {
		reviewing = null;
		error = "";
		notice = message || "the merge was written";
		bumpLedgerRevision();
		await refresh();
	}

	function toggle(copy: string) {
		reviewing = reviewing === copy ? null : copy;
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Conflicts</h1>
		<span class="text-muted-foreground text-xs">
			{git.repo
				? "git owns this ledger's versions; sync-client copies are not scanned"
				: "copies the sync client left beside the ledger's files"}
		</span>
		<div class="ml-auto flex items-center gap-1">
			{#if git.repo}
				<!-- There is nothing to rescan where git is the tracker; a merge
				     git could not settle is the merge view's, on Branches. -->
				<Button size="sm" variant="outline" href={resolve("/branches")}>Branches</Button>
			{:else}
				<Button size="sm" variant="ghost" onclick={refresh} disabled={refreshing}>
					{refreshing ? "Scanning…" : "Rescan"}
				</Button>
			{/if}
			<Button size="sm" variant="outline" href={resolve("/history")}>History</Button>
		</div>
	</div>

	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{/if}
	{#if notice}
		<Alert>{notice}</Alert>
	{/if}

	{#if report === null}
		{#if !error}
			<p class="text-muted-foreground text-sm">Scanning the ledger folder…</p>
		{/if}
	{:else if empty}
		<div class="flex flex-col gap-0.5">
			<h2 class="text-sm font-medium">No conflict copies</h2>
			<p class="text-muted-foreground text-xs">
				{#if git.repo}
					This ledger's folder is a git repository, so git tracks its versions: a sync
					client's second copy of a file is not what settles a difference here. A merge git
					could not finish is on the Branches screen.
				{:else}
					Every file the sync client touched is still under its own name; nothing was left
					beside it to reconcile.
				{/if}
			</p>
		</div>
	{:else}
		{#each groups as group (group.client)}
			<section class="flex flex-col gap-1.5">
				<h2 class="text-sm font-medium">{clientLabel(group.client)}</h2>
				<ul class="divide-border divide-y rounded-lg border">
					{#each group.candidates as candidate (candidate.copy)}
						<li>
							<div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2">
								<div class="min-w-0 flex-1">
									<p class="truncate text-sm font-medium" title={candidate.original}>
										<span class="text-muted-foreground text-[0.625rem] font-normal">
											original ·
										</span>
										{nameOf(candidate.original)}
									</p>
									<p
										class="text-muted-foreground min-w-0 truncate font-mono text-[0.625rem]"
										title={candidate.copy}
									>
										<span>copy ·</span>
										{nameOf(candidate.copy)}
									</p>
								</div>
								{#if candidate.stamp}
									<span class="text-muted-foreground font-mono text-[0.625rem]">
										{candidate.stamp}
									</span>
								{:else}
									<span class="text-muted-foreground text-[0.625rem]">no stamp</span>
								{/if}
								<Button
									size="sm"
									variant={reviewing === candidate.copy ? "secondary" : "outline"}
									onclick={() => toggle(candidate.copy)}
								>
									{reviewing === candidate.copy ? "Close" : "Review"}
								</Button>
							</div>
							{#if reviewing === candidate.copy}
								{#key candidate.copy}
									<ConflictResolver
										{candidate}
										onresolved={resolved}
										onclose={() => (reviewing = null)}
									/>
								{/key}
							{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/each}

		{#if orphans.length > 0}
			<section class="flex flex-col gap-1.5">
				<h2 class="text-sm font-medium">Copies without an original</h2>
				<p class="text-muted-foreground text-xs">
					The file each copy is named after is not in the folder, so there is nothing to merge
					into.
				</p>
				<ul class="divide-border divide-y rounded-lg border">
					{#each orphans as orphan (orphan.copy)}
						<li class="flex flex-wrap items-baseline gap-x-2 gap-y-1 px-3 py-2 text-xs">
							<Badge variant="outline">{clientLabel(orphan.client)}</Badge>
							<span class="text-muted-foreground">the original is missing —</span>
							<span class="min-w-0 font-mono text-[0.6875rem] break-all">{orphan.copy}</span>
							{#if orphan.stamp}
								<span class="text-muted-foreground font-mono text-[0.625rem]">
									{orphan.stamp}
								</span>
							{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/if}
	{/if}
</div>
