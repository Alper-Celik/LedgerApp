<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { onMount } from "svelte";
	import { commands, type Candidate, type ConflictView } from "../../../bindings";
	import {
		CHOICES,
		classLabel,
		dateOf,
		isUnchanged,
		nameOf,
		type ConflictPick,
	} from "./conflicts.js";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";

	/**
	 * One conflicted copy, resolved at directive granularity.
	 *
	 * The panel compares the copy with its original and puts the merge's own
	 * words at the top: how many directives it left alone, how many it settled
	 * and how many need a person. Directives that need one show both sides in
	 * full — the pick is about the directive, not about a line, because a
	 * half-transaction is not a ledger — and each pick is the backend's own
	 * `ours`/`theirs`/`both`/`drop` choice. Directives the merge settled are
	 * listed collapsed: they are context, not work.
	 *
	 * On a phone the two sides stack, in source order, so neither column is
	 * squeezed to unreadable width; the pick control follows them, inside the
	 * same card.
	 */
	let {
		candidate,
		onresolved,
		onclose,
	}: {
		candidate: Candidate;
		/** Called with the backend's success sentence once the merge is written. */
		onresolved: (message: string) => void;
		/** Dismiss the panel without writing anything. */
		onclose: () => void;
	} = $props();

	let comparison = $state<ConflictView | null>(null);
	let loading = $state(true);
	let error = $state("");
	/** The pick per decision key; a needs-user directive without one blocks Apply. */
	let picks = $state<Record<string, ConflictPick>>({});
	let applying = $state(false);
	let autoOpen = $state(false);

	async function load() {
		loading = true;
		error = "";
		const result = await commands.conflictCompare(candidate.original, candidate.copy);
		loading = false;
		if (result.status === "error") {
			error = result.error;
			return;
		}
		comparison = result.data;
	}

	onMount(() => {
		void load();
	});

	const needs = $derived(
		comparison?.decisions.filter((decision) => decision.needs_user) ?? []
	);
	/** Settled by the merge with a change to show; untouched directives stay out. */
	const auto = $derived(
		comparison?.decisions.filter(
			(decision) => !decision.needs_user && !isUnchanged(decision)
		) ?? []
	);
	/** Needs-user directives still waiting for a pick. */
	const missing = $derived(needs.filter((decision) => !picks[decision.key]).length);
	const headline = $derived.by(() => {
		const summary = comparison?.summary;
		if (!summary) return "";
		return `${summary.unchanged} unchanged, ${summary.auto} auto-merged, ${summary.needs_user} need you`;
	});

	async function apply() {
		if (!comparison) return;
		applying = true;
		error = "";
		const chosen = needs.flatMap((decision) => {
			const pick = picks[decision.key];
			return pick ? [[decision.key, pick] as [string, ConflictPick]] : [];
		});
		const result = await commands.conflictResolve(comparison.original, chosen);
		applying = false;
		if (result.status === "error") {
			error = result.error;
			return;
		}
		onresolved(result.data);
	}

	/** A side as the panel prints it: an absent or blank directive says so. */
	function textOf(text: string | null | undefined): string {
		return text && text.trim() !== "" ? text : "(nothing)";
	}
</script>

<section
	class="bg-muted/20 flex min-w-0 flex-col gap-2.5 border-t px-3 py-3"
	aria-label="Conflict resolver"
>
	<div class="flex flex-wrap items-center gap-x-2 gap-y-1">
		<h3 class="text-sm font-semibold tracking-tight">Resolve {nameOf(candidate.copy)}</h3>
		<span class="text-muted-foreground min-w-0 font-mono text-[0.625rem] break-all" title={candidate.copy}>
			{candidate.copy}
		</span>
		<Button size="xs" variant="ghost" class="ms-auto" onclick={onclose}>Close</Button>
	</div>

	{#if loading}
		<p class="text-muted-foreground text-xs">Comparing the two copies…</p>
	{:else if comparison === null}
		{#if error}
			<Alert variant="destructive">{error}</Alert>
		{/if}
		<div class="flex items-center gap-2">
			<Button size="sm" variant="outline" onclick={load}>Try again</Button>
		</div>
	{:else}
		<div class="flex flex-wrap items-center gap-1.5 text-xs">
			<span class="font-medium">{headline}</span>
			<span class="text-muted-foreground">of {comparison.summary.total} directives</span>
			{#each comparison.summary.kinds as [kind, count] (kind)}
				<Badge variant="outline">{kind} ×{count}</Badge>
			{/each}
			{#if comparison.summary.ambiguous > 0}
				<Badge variant="destructive">{comparison.summary.ambiguous} ambiguous</Badge>
			{/if}
		</div>

		{#if !comparison.base_found}
			<Alert>
				No history for this file — everything shows as added. Without a common base the
				merge cannot tell an edit from an addition, so every difference is a whole directive
				to choose.
			</Alert>
		{/if}

		{#if needs.length === 0}
			<p class="text-muted-foreground text-xs">
				Nothing here needs a choice — the merge settled every directive itself.
			</p>
		{:else}
			<div class="flex flex-col gap-2.5">
				{#each needs as decision (decision.key)}
					<article class="bg-background flex min-w-0 flex-col gap-2 rounded-md border p-2.5">
						<div class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
							<Badge variant="secondary">{decision.kind}</Badge>
							{#if dateOf(decision)}
								<span class="text-muted-foreground font-mono text-[0.625rem]">{dateOf(decision)}</span>
							{/if}
							<span class="min-w-0 text-sm font-medium break-words">{decision.label}</span>
							<span
								class="text-muted-foreground ms-auto font-mono text-[0.625rem]"
								title="how the directive stands across the three versions"
							>
								{classLabel(decision.class)}
							</span>
						</div>

						<div class="grid gap-2 md:grid-cols-2">
							<div class="flex min-w-0 flex-col gap-0.5">
								<span class="text-muted-foreground text-[0.625rem] font-medium tracking-wide uppercase">
									ours · {nameOf(candidate.original)}
								</span>
								<pre
									class="bg-muted/40 overflow-x-auto rounded-sm border p-1.5 font-mono text-[0.6875rem] leading-relaxed break-words whitespace-pre-wrap">{textOf(
										decision.ours
									)}</pre>
							</div>
							<div class="flex min-w-0 flex-col gap-0.5">
								<span class="text-muted-foreground text-[0.625rem] font-medium tracking-wide uppercase">
									theirs · {nameOf(candidate.copy)}
								</span>
								<pre
									class="bg-muted/40 overflow-x-auto rounded-sm border p-1.5 font-mono text-[0.6875rem] leading-relaxed break-words whitespace-pre-wrap">{textOf(
										decision.theirs
									)}</pre>
							</div>
						</div>

						<div class="flex flex-wrap items-center gap-2">
							<ToggleGroup.Root
								type="single"
								size="sm"
								value={picks[decision.key] ?? ""}
								onValueChange={(next) =>
									next && (picks = { ...picks, [decision.key]: next as ConflictPick })}
								aria-label={`Resolution for ${decision.label}`}
							>
								{#each CHOICES as choice (choice.id)}
									<ToggleGroup.Item value={choice.id} title={choice.hint}>
										{choice.label}
									</ToggleGroup.Item>
								{/each}
							</ToggleGroup.Root>
							{#if !picks[decision.key]}
								<span class="text-muted-foreground text-[0.625rem]">pick one</span>
							{/if}
						</div>
					</article>
				{/each}
			</div>
		{/if}

		{#if auto.length > 0}
			<Collapsible.Root bind:open={autoOpen}>
				<Collapsible.Trigger
					class="hover:bg-muted/60 focus-visible:ring-ring/50 border-border flex w-full items-center justify-between gap-2 rounded-md border px-2.5 py-1.5 text-left text-xs transition-colors focus-visible:ring-[3px] focus-visible:outline-none"
				>
					<span>
						{auto.length} auto-merged
						{auto.length === 1 ? "directive" : "directives"}
					</span>
					<span class="text-muted-foreground">{autoOpen ? "Hide" : "Show"}</span>
				</Collapsible.Trigger>
				<Collapsible.Content>
					<ul class="divide-border bg-background mt-1 divide-y rounded-md border">
						{#each auto as decision (decision.key)}
							<li class="flex flex-wrap items-baseline gap-x-2 gap-y-1 px-2.5 py-1.5 text-xs">
								<Badge variant="outline">{decision.kind}</Badge>
								{#if dateOf(decision)}
									<span class="text-muted-foreground font-mono text-[0.625rem]">{dateOf(decision)}</span>
								{/if}
								<span class="min-w-0 flex-1 truncate" title={decision.label}>{decision.label}</span>
								<span class="text-muted-foreground font-mono text-[0.625rem]">
									{classLabel(decision.class)}
								</span>
							</li>
						{/each}
					</ul>
				</Collapsible.Content>
			</Collapsible.Root>
		{/if}

		{#if error}
			<Alert variant="destructive">{error}</Alert>
		{/if}

		<div class="flex flex-wrap items-center gap-2">
			<Button size="sm" onclick={apply} disabled={applying || missing > 0}>
				{applying ? "Applying…" : "Apply resolution"}
			</Button>
			<Button size="sm" variant="ghost" onclick={load} disabled={applying}>
				Re-compare
			</Button>
			{#if missing > 0}
				<span class="text-muted-foreground text-xs">
					pick {missing} more {missing === 1 ? "directive" : "directives"} to apply
				</span>
			{:else}
				<span class="text-muted-foreground text-xs">
					writes the merged result to {nameOf(candidate.original)}
				</span>
			{/if}
		</div>
		<p class="text-muted-foreground text-[0.625rem]">
			ours is the file the ledger reads; theirs is the copy the sync client left beside it.
		</p>
	{/if}
</section>
