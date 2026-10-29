<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands, type BranchInfo, type ConflictFile } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import { messageOf } from "$lib/errors.js";
	import ConfirmBar from "./confirm-bar.svelte";
	import ConflictResolver from "./conflict-resolver.svelte";
	import { kindLabel, shortId } from "./git.js";

	/**
	 * Merging one branch into the one checked out.
	 *
	 * `git_merge` answers with exactly three shapes, and each gets its own
	 * sentence: a fast-forward (the branch moved), `up_to_date` (nothing to do),
	 * or a conflict list — which is also how a clean non-fast-forward merge is
	 * reported, with an empty list, because the merge is staged either way and
	 * waits for `git_merge_continue`. So an empty list is not "no conflicts to
	 * show", it is "staged and only needs finishing", and the footer says so.
	 *
	 * Conflicts are resolved one path at a time, and the merge is finished only
	 * after every one of them reports a staged text. Abort is always offered:
	 * while the panel knows about a merge it abandons that one, and otherwise the
	 * button still exists because a merge can be in progress from an earlier
	 * session (the page was reloaded, or another tool started it), and the backend
	 * is the authority on whether there is one. Its refusal is shown as it comes.
	 */
	let {
		branches,
		journal,
		onwrote,
		onresolved,
	}: {
		branches: BranchInfo[];
		/** The journal's files, absolute and repository-relative, for the resolver's path. */
		journal: { relative: string; absolute: string }[];
		/** A write that changed the working tree: reload the ledger and every view, then show the sentence. */
		onwrote: (message: string) => Promise<void>;
		/** One path staged: refresh the working-tree strip and show the sentence. */
		onresolved: (message: string) => Promise<void>;
	} = $props();

	type Outcome =
		| { kind: "fast_forward"; head: string }
		| { kind: "up_to_date" }
		| { kind: "conflicts"; files: ConflictFile[] };

	let target = $state("");
	const current = $derived(branches.find((branch) => branch.is_current)?.name ?? "");
	const options = $derived(branches.filter((branch) => branch.name !== current));
	const selected = $derived(
		options.some((branch) => branch.name === target) ? target : (options[0]?.name ?? "")
	);

	let confirmingMerge = $state(false);
	let confirmingAbort = $state(false);
	let busy = $state(false);
	let error = $state("");
	let outcome = $state<Outcome | null>(null);
	/** The path whose three-way view is open. */
	let resolving = $state<string | null>(null);
	/** Paths this session has staged a resolution for. */
	let resolved = $state<Record<string, true>>({});

	const unresolved = $derived(
		outcome?.kind === "conflicts"
			? outcome.files.filter((file) => !(file.path in resolved)).length
			: 0
	);
	const finished = $derived(outcome?.kind === "conflicts" && unresolved === 0);

	/**
	 * The path the resolver's commands take: the journal file's absolute path
	 * when the conflict is one of them.
	 *
	 * The exact repository-relative match is the normal case. The suffix match
	 * covers a repository root spelled differently from the loader's canonical
	 * paths (a symlink above the work tree, a trailing separator), where the
	 * relative form would not line up; the absolute path is what both
	 * `git_conflict_texts` and `git_resolve` can take regardless of how deep the
	 * ledger sits below the repository root.
	 */
	function applyPathFor(path: string): string {
		const exact = journal.find((file) => file.relative === path);
		if (exact) return exact.absolute;
		const suffix = `/${path}`;
		return journal.find((file) => file.absolute.endsWith(suffix))?.absolute ?? path;
	}

	/** Whether the app can stage a text for this path at all (it writes only the journal's files). */
	function writable(path: string): boolean {
		if (journal.length === 0) return true;
		const suffix = `/${path}`;
		return journal.some(
			(file) => file.relative === path || file.absolute.endsWith(suffix)
		);
	}

	async function merge() {
		if (selected === "") return;
		const theirs = selected;
		const into = current || "HEAD";
		busy = true;
		error = "";
		confirmingMerge = false;
		// A refusal must not leave the previous merge's sentence on screen.
		outcome = null;
		try {
			const result = await commands.gitMerge(theirs);
			if (result.status === "error") {
				error = result.error;
				return;
			}
			const value = result.data;
			if (typeof value === "string") {
				outcome = { kind: "up_to_date" };
				await onwrote(`Nothing to merge: ${theirs} is already in ${into}`);
				return;
			}
			if (value.fast_forward) {
				const head = value.fast_forward.new_head;
				outcome = { kind: "fast_forward", head };
				await onwrote(`Fast-forwarded ${into} to ${shortId(head)}`);
				return;
			}
			const files = value.conflicts ?? [];
			outcome = { kind: "conflicts", files };
			resolving = null;
			resolved = {};
			await onwrote(
				files.length === 0
					? `Merged ${theirs} into ${into} — the staged merge only needs finishing`
					: `Merging ${theirs} into ${into}: ${files.length} path${files.length === 1 ? "" : "s"} in conflict`
			);
		} catch (cause) {
			console.error("gitMerge failed", cause);
			error = messageOf(cause);
		} finally {
			busy = false;
		}
	}

	async function finish() {
		busy = true;
		error = "";
		try {
			const result = await commands.gitMergeContinue();
			if (result.status === "error") {
				error = result.error;
				return;
			}
			outcome = null;
			resolving = null;
			resolved = {};
			await onwrote("The merge is committed");
		} catch (cause) {
			console.error("gitMergeContinue failed", cause);
			error = messageOf(cause);
		} finally {
			busy = false;
		}
	}

	async function abort() {
		busy = true;
		error = "";
		try {
			const result = await commands.gitMergeAbort();
			if (result.status === "error") {
				error = result.error;
				return;
			}
			outcome = null;
			resolving = null;
			resolved = {};
			await onwrote("Merge aborted");
		} catch (cause) {
			console.error("gitMergeAbort failed", cause);
			error = messageOf(cause);
		} finally {
			busy = false;
			confirmingAbort = false;
		}
	}

	async function resolvedPath(path: string) {
		resolved = { ...resolved, [path]: true };
		resolving = null;
		await onresolved(`Staged the resolution for ${path}`);
	}
</script>

<section class="flex flex-col gap-2">
	<div class="flex flex-wrap items-baseline gap-2">
		<h2 class="text-sm font-medium">Merge</h2>
		<span class="text-muted-foreground text-xs">
			{current ? `into ${current}` : "nothing checked out"}
		</span>
	</div>

	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{/if}

	{#if options.length === 0}
		<p class="text-muted-foreground text-sm">
			No branch to merge into {current || "this one"} yet. Create another branch and commit to
			it first.
		</p>
	{:else}
		<div class="flex flex-wrap items-center gap-2">
			<Select.Root
				type="single"
				value={selected}
				onValueChange={(next) => next && (target = next)}
			>
				<Select.Trigger type="button" class="w-full sm:w-72" aria-label="Branch to merge">
					<span data-slot="select-value" class="min-w-0 truncate font-mono">{selected}</span>
				</Select.Trigger>
				<Select.Content>
					{#each options as branch (branch.name)}
						<Select.Item value={branch.name} label={branch.name} />
					{/each}
				</Select.Content>
			</Select.Root>
			<Button
				size="sm"
				variant="secondary"
				disabled={busy || selected === "" || outcome?.kind === "conflicts"}
				onclick={() => {
					error = "";
					confirmingMerge = true;
				}}
			>
				Merge
			</Button>
		</div>

		{#if confirmingMerge}
			<ConfirmBar
				question={`Merge ${selected} into ${current || "the current commit"}? The working tree must be clean; files are written, and a clean merge still has to be finished before it is committed.`}
				confirm="Merge"
				busy={busy}
				busyLabel="Merging…"
				onconfirm={() => void merge()}
				oncancel={() => (confirmingMerge = false)}
			/>
		{/if}
	{/if}

	{#if outcome?.kind === "fast_forward"}
		<Alert>
			Fast-forward: {current || "HEAD"} now points at
			<span class="font-mono">{shortId(outcome.head)}</span>
			. No merge commit was made, because the branch had not moved since the merged one.
		</Alert>
	{:else if outcome?.kind === "up_to_date"}
		<Alert>Nothing to do: the branch is already contained in {current}.</Alert>
	{:else if outcome?.kind === "conflicts"}
		{#if outcome.files.length === 0}
			<Alert>
				The merge is staged and clean: there were no conflicts to settle, and the merge
				commit is waiting to be finished.
			</Alert>
		{:else}
			<div class="flex flex-col gap-1.5">
				<div class="flex flex-wrap items-baseline gap-2">
					<h3 class="text-xs font-medium">Conflicted paths</h3>
					<span class="text-muted-foreground text-[0.625rem]">
						{unresolved} of {outcome.files.length} still need a resolved text
					</span>
				</div>
				<ul class="divide-border divide-y rounded-lg border">
					{#each outcome.files as file (file.path)}
						<li>
							<div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2">
								<div class="min-w-0 flex-1">
									<p class="font-mono text-[0.6875rem] break-all" title={file.path}>
										{file.path}
									</p>
									<p class="text-muted-foreground text-[0.625rem]">
										{kindLabel(file.kind)} · {file.stages}
										{file.stages === 1 ? "stage" : "stages"}
									</p>
									{#if !writable(file.path)}
										<p class="text-muted-foreground text-[0.625rem]">
											Not one of the journal's files: the app writes merges through the
											ledger, so it cannot stage a text for this path. Resolve it outside
											the app, or abort the merge.
										</p>
									{/if}
								</div>
								{#if file.path in resolved}
									<Badge variant="secondary">resolved</Badge>
								{:else}
									<Button
										size="sm"
										variant={resolving === file.path ? "secondary" : "outline"}
										onclick={() => (resolving = resolving === file.path ? null : file.path)}
									>
										{resolving === file.path ? "Close" : "Resolve"}
									</Button>
								{/if}
							</div>
							{#if resolving === file.path}
								{#key file.path}
									<ConflictResolver
										path={file.path}
										applyPath={applyPathFor(file.path)}
										oursLabel={current || "this branch"}
										theirsLabel={selected || "the merged branch"}
										onapplied={resolvedPath}
										onclose={() => (resolving = null)}
									/>
								{/key}
							{/if}
						</li>
					{/each}
				</ul>
			</div>
		{/if}
	{/if}

	<div class="flex flex-wrap items-center gap-2">
		{#if outcome?.kind === "conflicts"}
			{#if finished}
				<Button size="sm" disabled={busy} onclick={() => void finish()}>
					{busy ? "Finishing…" : "Finish merge"}
				</Button>
			{:else}
				<span class="text-muted-foreground text-[0.625rem]">
					Every path needs a staged resolution before the merge can be finished.
				</span>
			{/if}
			<Button
				size="sm"
				variant="destructive"
				disabled={busy}
				onclick={() => (confirmingAbort = true)}
			>
				Abort merge
			</Button>
		{:else}
			<span class="text-muted-foreground min-w-0 flex-1 text-[0.625rem]">
				A merge left in progress — after a reload, say — can be finished or abandoned here;
				git answers with its reason when there is none. If it still has conflicted paths this
				panel never saw, abort it and merge again to see them.
			</span>
			<Button size="sm" variant="outline" disabled={busy} onclick={() => void finish()}>
				{busy ? "Finishing…" : "Finish merge"}
			</Button>
			<Button
				size="sm"
				variant="destructive"
				disabled={busy}
				onclick={() => (confirmingAbort = true)}
			>
				Abort merge
			</Button>
		{/if}
	</div>

	{#if confirmingAbort}
		<ConfirmBar
			question="Abort the merge? Every working-tree change made since it began is discarded — resolutions and other edits — and the files return to where the branch was."
			confirm="Abort merge"
			destructive
			busy={busy}
			busyLabel="Aborting…"
			onconfirm={() => void abort()}
			oncancel={() => (confirmingAbort = false)}
		/>
	{/if}
</section>
