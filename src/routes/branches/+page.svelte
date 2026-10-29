<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { resolve } from "$app/paths";
	import { commands, type BranchInfo, type CommitInfo } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import BranchPanel from "$lib/components/git/branch-panel.svelte";
	import CommitLog from "$lib/components/git/commit-log.svelte";
	import MergePanel from "$lib/components/git/merge-panel.svelte";
	import { changedPaths, repoRelative } from "$lib/components/git/git.js";
	import { messageOf } from "$lib/errors.js";
	import { ledger, openLedgerPath } from "$lib/ledger.svelte.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import type { PageProps } from "./$types";

	/**
	 * The repository screen: everything the app can do to the ledger's git
	 * history without a shell — list branches, create one, switch, read and
	 * revert commits, and merge another branch in, conflicts included.
	 *
	 * Every write goes through a confirmation inside the panel that owns it, and
	 * after it lands the page re-reads the repository and has the ledger reloaded
	 * from disk: a switch or a merge rewrites the journal's files, so a register
	 * still showing the old branch's numbers would be a lie. The working-tree
	 * strip is the ledger's own files (that is what `ledger_diff` covers), while
	 * git's guard for switching covers the whole repository — so a refusal may
	 * name a file the strip does not, and it is shown exactly as written.
	 */
	let { data }: PageProps = $props();

	/** The route's own read, taken once; the screen's state takes over from here. */
	const loaded = untrack(() => data);
	const root = loaded.root;

	let branches = $state<BranchInfo[]>(loaded.branches);
	let commits = $state<CommitInfo[]>(loaded.commits);
	let journal = $state(loaded.journal);
	let tree = $state<string[]>(loaded.tree);
	let limit = $state(loaded.limit);
	let branchesError = $state(loaded.branchesError);
	let logError = $state(loaded.logError);
	let treeError = $state(loaded.treeError);
	/** A failed read of the screen's own data, or a ledger that would not reload. */
	let failure = $state(loaded.error);
	/** The last write's sentence. */
	let notice = $state("");
	let refreshing = $state(false);

	const current = $derived(branches.find((branch) => branch.is_current)?.name ?? "");

	async function refreshBranches() {
		try {
			const answer = await commands.gitBranches();
			if (answer.status === "error") {
				branchesError = answer.error;
				return;
			}
			branches = answer.data;
			branchesError = "";
		} catch (cause) {
			console.error("gitBranches failed", cause);
			branchesError = messageOf(cause);
		}
	}

	async function refreshLog() {
		try {
			const answer = await commands.gitLog(limit, null);
			if (answer.status === "error") {
				logError = answer.error;
				return;
			}
			commits = answer.data;
			logError = "";
		} catch (cause) {
			console.error("gitLog failed", cause);
			logError = messageOf(cause);
		}
	}

	async function refreshTree() {
		try {
			const answer = await commands.ledgerDiff();
			if (answer.status === "error") {
				treeError = answer.error;
				return;
			}
			tree = changedPaths(answer.data);
			treeError = "";
		} catch (cause) {
			console.error("ledgerDiff failed", cause);
			treeError = messageOf(cause);
		}
	}

	/** The journal's files, re-read after a branch changed which includes exist. */
	async function refreshJournal() {
		try {
			journal = (await commands.journalFiles()).map((file) => ({
				relative: repoRelative(root, file.path),
				absolute: file.path,
			}));
		} catch (cause) {
			// The merge's own paths still work; only the pre-check loses its list.
			console.error("journalFiles failed", cause);
		}
	}

	async function refresh() {
		refreshing = true;
		try {
			await Promise.all([
				refreshBranches(),
				refreshLog(),
				refreshTree(),
				refreshJournal(),
			]);
		} finally {
			refreshing = false;
		}
	}

	/**
	 * Re-read the ledger from disk after a repository operation changed the
	 * working tree — a switch, a merge, an aborted merge, a revert.
	 *
	 * Two reasons this is not optional. The app's ledger is a snapshot, so after
	 * git rewrites files the register would show another branch's numbers. And
	 * `git_resolve` refuses a file that changed on disk since the load, so a
	 * merge whose conflict markers are not in the loaded copy could not be
	 * resolved at all. A merge's markers load as parse errors but the loader
	 * still loads the journal, which is exactly the state the resolver needs.
	 *
	 * Returns the complaint to show, or `""` when the app now matches the disk.
	 */
	async function reloadLedger(): Promise<string> {
		const path = ledger.summary?.path;
		if (!path) {
			return "no ledger is open, so the ledger in memory cannot be re-read after this change";
		}
		const loaded = await openLedgerPath(path);
		return loaded ? "" : ledger.error || "the ledger could not be re-read";
	}

	/**
	 * A write that changed files on disk. The ledger is reloaded first, so the
	 * screen's own journal list and the ledger in memory match the branch before
	 * anything else is read.
	 */
	async function afterWrite(message: string) {
		notice = message;
		failure = "";
		const reloadFailure = await reloadLedger();
		if (reloadFailure) {
			failure = `The repository changed, but the ledger did not reload: ${reloadFailure}`;
		}
		await refresh();
	}

	/** One conflicted path staged: the ledger's files changed, the history did not. */
	async function afterResolve(message: string) {
		notice = message;
		bumpLedgerRevision();
		await refreshTree();
	}

	async function afterLimit(next: number) {
		try {
			const answer = await commands.gitLog(next, null);
			if (answer.status === "error") {
				logError = answer.error;
				return;
			}
			commits = answer.data;
			limit = next;
			logError = "";
		} catch (cause) {
			console.error("gitLog failed", cause);
			logError = messageOf(cause);
		}
	}

	/**
	 * Refresh is the screen's recovery button: it re-reads the ledger from disk
	 * (an outside editor may have rewritten it) and then the repository. That is
	 * what makes the backend's "changed on disk — reload the ledger" refusal
	 * actionable from here.
	 */
	async function refreshAll() {
		notice = "";
		failure = "";
		const reloadFailure = await reloadLedger();
		if (reloadFailure) {
			failure = `The ledger did not reload: ${reloadFailure}`;
		}
		await refresh();
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Branches</h1>
		<span class="text-muted-foreground text-xs">
			{loaded.repo && current
				? `branches, history and merges — on ${current}`
				: "branches, history and merges of the ledger's repository"}
		</span>
		{#if loaded.repo}
			<div class="ml-auto flex items-center gap-1">
				<Button size="sm" variant="ghost" disabled={refreshing} onclick={() => void refreshAll()}>
					{refreshing ? "Reading…" : "Refresh"}
				</Button>
				<Button size="sm" variant="outline" href={resolve("/diff")}>Diff</Button>
			</div>
		{/if}
	</div>

	{#if !loaded.repo}
		<div class="flex max-w-2xl flex-col gap-2 rounded-lg border px-3 py-4">
			{#if loaded.error}
				<p class="text-sm font-medium">The repository state could not be read</p>
				<p class="text-muted-foreground text-xs">
					The screen first has to know whether the ledger's folder is a repository, and that
					read failed.
				</p>
				<Alert variant="destructive">{loaded.error}</Alert>
			{:else if loaded.noLedger}
				<p class="text-sm font-medium">No ledger is open</p>
				<p class="text-muted-foreground text-xs">
					Branches are a property of the repository holding the journal, and no journal is
					loaded yet. Open a ledger folder, then come back.
				</p>
			{:else}
				<p class="text-sm font-medium">This ledger is not in a git repository</p>
				<p class="text-muted-foreground text-xs">
					Branches, history and merges need a repository beside the journal. Settings → Git
					can create one in the ledger's folder and set the identity commits are attributed
					to; nothing is pushed anywhere. Come back here once the repository exists.
				</p>
			{/if}
			<div>
				<Button size="sm" variant="secondary" href={resolve("/settings")}>
					{loaded.noLedger || loaded.error ? "Open Settings" : "Open Settings → Git"}
				</Button>
			</div>
		</div>
	{:else}
		{#if failure}
			<Alert variant="destructive">{failure}</Alert>
		{/if}
		{#if notice}
			<Alert>{notice}</Alert>
		{/if}

		<section class="flex flex-col gap-1.5">
			<div class="flex flex-wrap items-baseline gap-2">
				<h2 class="text-sm font-medium">Working tree</h2>
				<span class="text-muted-foreground text-xs">the ledger's files against HEAD</span>
				{#if root}
					<span
						class="text-muted-foreground ms-auto min-w-0 truncate font-mono text-[0.625rem]"
						title={root ?? ""}
					>
						{root}
					</span>
				{/if}
			</div>
			{#if treeError}
				<Alert variant="destructive">{treeError}</Alert>
			{:else if tree.length === 0}
				<p class="text-muted-foreground text-sm">
					Working tree clean — no journal file differs from the last commit.
				</p>
			{:else}
				<ul class="flex flex-wrap gap-1">
					{#each tree as path (path)}
						<li class="bg-muted/40 rounded-sm border px-1.5 py-0.5 font-mono text-[0.625rem]">
							{path}
						</li>
					{/each}
				</ul>
				<p class="text-muted-foreground text-[0.625rem]">
					Switching and merging refuse while tracked changes are uncommitted; the refusal
					names them, and it covers the whole repository, so it can name a file outside the
					ledger. An untracked file blocks a switch only when the target branch would
					overwrite it.
				</p>
			{/if}
		</section>

		{#if branchesError}
			<Alert variant="destructive">{branchesError}</Alert>
		{:else}
			<BranchPanel {branches} onchanged={afterWrite} />

			<MergePanel
				{branches}
				{journal}
				onwrote={afterWrite}
				onresolved={afterResolve}
			/>
		{/if}

		<CommitLog
			{commits}
			{limit}
			error={logError}
			onlimit={afterLimit}
			onreverted={afterWrite}
		/>
	{/if}
</div>
