<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands, type BranchInfo } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import { Toggle } from "$lib/components/ui/toggle/index.js";
	import { messageOf } from "$lib/errors.js";
	import ConfirmBar from "./confirm-bar.svelte";

	/**
	 * The repository's branches, where the user is, and the two writes that move
	 * that: creating a branch and switching to one.
	 *
	 * Both are confirmed before they run, because a switch rewrites the working
	 * tree and a creation writes a ref. A switch's refusal is shown exactly as
	 * the backend wrote it: it names the uncommitted files, which is the whole
	 * reason the command refuses instead of carrying them across.
	 *
	 * Remote branches are listed because they are what a local branch tracks, but
	 * only local branches can be checked out — the command takes a local name, so
	 * a remote row gets no button rather than one that always refuses.
	 */
	let {
		branches,
		onchanged,
	}: {
		branches: BranchInfo[];
		/** Called after a write lands, with the sentence to show; awaited so the row stays busy until every view is current. */
		onchanged: (message: string) => Promise<void>;
	} = $props();

	const current = $derived(branches.find((branch) => branch.is_current)?.name ?? "");
	let name = $state("");
	let switchAfter = $state(true);
	/** The branch whose switch is waiting for its confirmation. */
	let confirmingSwitch = $state<string | null>(null);
	let confirmingCreate = $state(false);
	let busy = $state(false);
	let error = $state("");

	const target = $derived(name.trim());

	async function switchTo(branch: BranchInfo) {
		busy = true;
		error = "";
		try {
			const result = await commands.gitCheckout(branch.name);
			if (result.status === "error") {
				// The refusal names the files that block the switch; it is the answer.
				error = result.error;
				return;
			}
			confirmingSwitch = null;
			await onchanged(`Switched to ${branch.name}`);
		} catch (cause) {
			console.error("gitCheckout failed", cause);
			error = messageOf(cause);
		} finally {
			busy = false;
			confirmingSwitch = null;
		}
	}

	async function create() {
		if (target === "") return;
		busy = true;
		error = "";
		try {
			const result = await commands.gitCreateBranch(target, switchAfter);
			if (result.status === "error") {
				error = result.error;
				return;
			}
			const created = target;
			name = "";
			await onchanged(
				switchAfter ? `Created ${created} and switched to it` : `Created ${created}`
			);
		} catch (cause) {
			console.error("gitCreateBranch failed", cause);
			error = messageOf(cause);
		} finally {
			busy = false;
			confirmingCreate = false;
		}
	}
</script>

<section class="flex flex-col gap-1.5">
	<div class="flex flex-wrap items-baseline gap-2">
		<h2 class="text-sm font-medium">Branches</h2>
		<span class="text-muted-foreground text-xs">
			{current ? `on ${current}` : "no branch checked out"}
		</span>
	</div>

	{#if branches.length === 0}
		<div class="rounded-lg border px-3 py-4">
			<p class="text-sm font-medium">No branches yet</p>
			<p class="text-muted-foreground mt-1 text-xs">
				A repository with no commits has no branch to show or switch to. Make the first commit
				from the Diff view — it creates the first branch.
			</p>
		</div>
	{:else}
		<ul class="divide-border divide-y rounded-lg border">
			{#each branches as branch (branch.name)}
				<li>
					<div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2">
						<div class="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 gap-y-0.5">
							<span class="min-w-0 truncate font-mono text-xs" title={branch.name}>
								{branch.name}
							</span>
							{#if branch.is_current}
								<Badge variant="default">current</Badge>
							{/if}
							{#if branch.is_remote}
								<Badge variant="outline">remote</Badge>
							{/if}
							{#if branch.upstream}
								<span class="text-muted-foreground font-mono text-[0.625rem]">
									tracks {branch.upstream}
								</span>
							{:else if !branch.is_remote}
								<span class="text-muted-foreground text-[0.625rem]">no upstream</span>
							{/if}
						</div>
						{#if !branch.is_current && !branch.is_remote}
							<Button
								size="sm"
								variant="outline"
								disabled={busy}
								aria-label={`Switch to ${branch.name}`}
								onclick={() => {
									error = "";
									confirmingSwitch = branch.name;
								}}
							>
								Switch
							</Button>
						{:else if branch.is_remote}
							<span class="text-muted-foreground text-[0.625rem]">remote-tracking only</span>
						{/if}
					</div>

					{#if confirmingSwitch === branch.name}
						<div class="px-3 pb-2">
							<ConfirmBar
								question={`Switch to ${branch.name}? The working tree is rewritten to that branch's files.`}
								confirm="Switch"
								busy={busy}
								busyLabel="Switching…"
								onconfirm={() => void switchTo(branch)}
								oncancel={() => (confirmingSwitch = null)}
							/>
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}

	<form
		class="flex flex-col gap-1.5"
		onsubmit={(event) => {
			event.preventDefault();
			if (target !== "" && !busy) confirmingCreate = true;
		}}
	>
		<div class="flex flex-wrap items-center gap-2">
			<Input
				class="min-w-0 flex-1 font-mono sm:max-w-64"
				bind:value={name}
				placeholder="feature/new-thing"
				aria-label="New branch name"
				autocomplete="off"
				spellcheck={false}
				disabled={busy}
			/>
			<Toggle
				bind:pressed={switchAfter}
				variant="outline"
				size="sm"
				type="button"
				disabled={busy}
				aria-label="Switch to the new branch after creating it"
			>
				Switch to it
			</Toggle>
			<Button
				size="sm"
				variant="secondary"
				type="submit"
				disabled={target === "" || busy}
			>
				New branch
			</Button>
		</div>
		{#if confirmingCreate}
			<ConfirmBar
				question={switchAfter
					? `Create branch ${target} and switch to it? It starts at the current commit; switching needs a clean working tree.`
					: `Create branch ${target}? It starts at the current commit and stays unchecked.`}
				confirm={switchAfter ? "Create & switch" : "Create branch"}
				busy={busy}
				busyLabel="Creating…"
				onconfirm={() => void create()}
				oncancel={() => (confirmingCreate = false)}
			/>
		{/if}
	</form>

	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{/if}
</section>
