<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands, type CommitInfo } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import { messageOf } from "$lib/errors.js";
	import GitDiff from "$lib/components/git-diff.svelte";
	import ConfirmBar from "./confirm-bar.svelte";
	import { commitTime, fileCount, shortId } from "./git.js";

	/**
	 * The repository's history, newest first, with each commit's patch a click
	 * away and its undo a confirmation away.
	 *
	 * A row opens `git_commit_diff`, rendered by the same diff component the
	 * diff view uses, so a commit reads exactly like the uncommitted changes it
	 * once was. The limit is a limit, not a page: 50, 100 or 200 commits from
	 * HEAD, so the list on screen is always the newest prefix of the history.
	 *
	 * Revert writes a commit, so it never happens on one click: the row asks
	 * first, and the answer — the new commit's id, or the backend's refusal
	 * (a merge commit has no single change to undo; later edits may conflict with
	 * the undo) — is shown where the row is.
	 */
	let {
		commits,
		limit,
		error,
		onlimit,
		onreverted,
	}: {
		commits: CommitInfo[];
		limit: number;
		/** A failed log read, shown above the list. */
		error: string;
		/** Read a different limit; the page owns the list. */
		onlimit: (next: number) => Promise<void>;
		/** After a revert landed, with the sentence to show. */
		onreverted: (message: string) => Promise<void>;
	} = $props();

	const LIMITS = [50, 100, 200] as const;

	/** The commit whose patch is open. */
	let selected = $state<string | null>(null);
	/** Which patch load owns the panel; an older answer is discarded. */
	let request = 0;
	let diff = $state("");
	let diffError = $state("");
	let diffLoading = $state(false);
	/** The commit whose revert is waiting for its confirmation. */
	let confirmingRevert = $state<string | null>(null);
	let reverting = $state<string | null>(null);
	let revertError = $state<{ id: string; message: string } | null>(null);
	/** The new commit each reverted commit became. */
	let reverted = $state<Record<string, string>>({});
	let changingLimit = $state(false);

	async function toggle(commit: CommitInfo) {
		if (selected === commit.id) {
			selected = null;
			return;
		}
		const id = commit.id;
		selected = id;
		diff = "";
		diffError = "";
		diffLoading = true;
		const ticket = ++request;
		try {
			const result = await commands.gitCommitDiff(id);
			// Another row may have been clicked while this one was reading.
			if (ticket !== request) return;
			if (result.status === "error") {
				diffError = result.error;
				return;
			}
			diff = result.data;
		} catch (cause) {
			console.error("gitCommitDiff failed", cause);
			if (ticket === request) diffError = messageOf(cause);
		} finally {
			if (ticket === request) diffLoading = false;
		}
	}

	async function revert(commit: CommitInfo) {
		reverting = commit.id;
		revertError = null;
		try {
			const result = await commands.gitRevertCommit(commit.id);
			if (result.status === "error") {
				revertError = { id: commit.id, message: result.error };
				return;
			}
			confirmingRevert = null;
			const written = shortId(result.data);
			reverted = { ...reverted, [commit.id]: written };
			await onreverted(`Reverted ${commit.short} as ${written}`);
		} catch (cause) {
			console.error("gitRevertCommit failed", cause);
			revertError = { id: commit.id, message: messageOf(cause) };
		} finally {
			reverting = null;
			confirmingRevert = null;
		}
	}

	async function changeLimit(next: string | undefined) {
		if (!next) return;
		const value = Number(next);
		if (!Number.isFinite(value) || value === limit) return;
		changingLimit = true;
		try {
			await onlimit(value);
		} finally {
			changingLimit = false;
		}
	}
</script>

<section class="flex flex-col gap-1.5">
	<div class="flex flex-wrap items-center gap-2">
		<h2 class="text-sm font-medium">Commits</h2>
		<span class="text-muted-foreground text-xs">newest first — pick a row for its patch</span>
		<div class="ms-auto flex items-center gap-2">
			{#if changingLimit}
				<span class="text-muted-foreground text-[0.625rem]">reading…</span>
			{/if}
			<Select.Root type="single" value={String(limit)} onValueChange={changeLimit}>
				<Select.Trigger
					type="button"
					class="w-32"
					disabled={changingLimit}
					aria-label="How many commits to show"
				>
					<span data-slot="select-value">{limit} commits</span>
				</Select.Trigger>
				<Select.Content>
					{#each LIMITS as value (value)}
						<Select.Item value={String(value)} label={`${value} commits`} />
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
	</div>

	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{/if}

	{#if commits.length === 0 && !error}
		<div class="rounded-lg border px-3 py-6 text-center">
			<p class="text-sm font-medium">No commits yet</p>
			<p class="text-muted-foreground mx-auto mt-1 max-w-md text-xs">
				Nothing has been committed in this repository. The Diff view stages the ledger's
				files and makes the first commit, which is also what gives the repository its first
				branch.
			</p>
		</div>
	{:else if commits.length > 0}
		<ul class="divide-border divide-y rounded-lg border">
			{#each commits as commit (commit.id)}
				<li>
					<div class="flex items-start gap-1 pr-2">
						<button
							type="button"
							class="hover:bg-muted/50 grid w-full min-w-0 grid-cols-[5rem_minmax(0,1fr)] items-baseline gap-x-2 gap-y-1 rounded-sm px-3 py-2 text-left sm:grid-cols-[5rem_minmax(0,1fr)_9rem_8rem_4rem] sm:items-center sm:gap-2"
							aria-expanded={selected === commit.id}
							onclick={() => void toggle(commit)}
						>
							<span class="font-mono text-[0.6875rem]">{commit.short}</span>
							<span class="min-w-0 truncate text-sm" title={commit.summary}>
								{commit.summary || "(no message)"}
							</span>
							<span
								class="text-muted-foreground col-span-2 flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5 text-[0.625rem] sm:contents"
								title={`${commit.author} <${commit.email}> · ${commitTime(commit.time)}`}
							>
								<span class="min-w-0 truncate">{commit.author || "unknown author"}</span>
								<span class="font-mono">{commitTime(commit.time)}</span>
								<span>{fileCount(commit.files)}</span>
							</span>
						</button>
						<Button
							size="sm"
							variant="ghost"
							class="mt-1 shrink-0"
							disabled={reverting !== null}
							aria-label={`Revert ${commit.short}`}
							onclick={() => {
								revertError = null;
								confirmingRevert = commit.id;
							}}
						>
							Revert
						</Button>
					</div>

					{#if confirmingRevert === commit.id}
						<div class="px-3 pt-1 pb-2">
							<ConfirmBar
								question={`Revert ${commit.short} ${commit.summary ? `“${commit.summary}”` : ""}? This writes a new commit that undoes it; history stays as it is.`}
								confirm="Revert"
								destructive
								busy={reverting === commit.id}
								busyLabel="Reverting…"
								onconfirm={() => void revert(commit)}
								oncancel={() => (confirmingRevert = null)}
							/>
						</div>
					{/if}

					{#if reverted[commit.id]}
						<p class="text-muted-foreground px-3 pb-2 text-[0.625rem]">
							reverted as <span class="font-mono">{reverted[commit.id]}</span>
						</p>
					{/if}

					{#if revertError?.id === commit.id}
						<div class="px-3 pb-2">
							<Alert variant="destructive">{revertError.message}</Alert>
						</div>
					{/if}

					{#if selected === commit.id}
						<div class="border-t px-3 py-2">
							{#if diffLoading}
								<p class="text-muted-foreground text-xs">Reading the patch…</p>
							{:else if diffError}
								<Alert variant="destructive">{diffError}</Alert>
							{:else if diff.trim() === ""}
								<p class="text-muted-foreground text-xs">
									This commit changed no files — an empty commit, or a merge with nothing to
									show against its first parent.
								</p>
							{:else}
								<GitDiff {diff} />
							{/if}
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</section>
