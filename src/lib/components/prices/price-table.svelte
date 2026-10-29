<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { PriceSourceInfo, PriceTarget } from "../../../bindings";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import { pairKey, type RowState } from "$lib/components/prices/prices.js";

	/**
	 * What the ledger can price, one row per commodity-and-currency pair: the
	 * pair, the price the ledger already holds, the source the next quote would
	 * come from, and a fetch that goes and asks it.
	 *
	 * The row is a grid rather than a table element: on a phone the six cells
	 * stack into three two-cell lines, and the fetched answer — or the refusal —
	 * takes a line of its own under them. Nothing about fetching lives here; the
	 * page owns the state and hands each row what it knows.
	 */
	let {
		targets,
		sources,
		stateOf,
		sourceOf,
		onchange,
		onfetch,
	}: {
		targets: PriceTarget[];
		/** The registry, for the per-row source picker. */
		sources: PriceSourceInfo[];
		stateOf: (target: PriceTarget) => RowState;
		sourceOf: (target: PriceTarget) => string;
		onchange: (target: PriceTarget, source: string) => void;
		onfetch: (target: PriceTarget) => void;
	} = $props();

	/** What a source says about itself, for the picker's tooltip. */
	const descriptions = $derived(new Map(sources.map((source) => [source.name, source.description])));
</script>

{#if targets.length === 0}
	<p class="text-muted-foreground text-xs">
		Nothing to price: the ledger reports no commodity-and-currency pair to fetch a quote for.
	</p>
{:else}
	<div class="rounded-lg border">
		<!--
			The column names, on a wide screen only: the stacked phone layout
			labels each value with the thing it is, so a header row would sit
			above nothing it lines up with.
		-->
		<div
			class="text-muted-foreground hidden grid-cols-[minmax(0,1fr)_4rem_5rem_10rem_10rem_6rem] items-center gap-2 border-b px-3 py-1.5 text-xs sm:grid"
		>
			<span>Commodity</span>
			<span>Kind</span>
			<span>Quote</span>
			<span class="text-right">Ledger price</span>
			<span>Source</span>
			<span></span>
		</div>
		<ul class="divide-border divide-y">
			{#each targets as target (pairKey(target.commodity, target.currency))}
				{@const state = stateOf(target)}
				{@const source = sourceOf(target)}
				{@const fetching = state.status === "busy"}
				<li
					class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1.5 px-3 py-2 sm:grid-cols-[minmax(0,1fr)_4rem_5rem_10rem_10rem_6rem] sm:gap-2"
					aria-busy={fetching}
				>
					<span class="min-w-0 truncate font-medium" title={target.commodity}>
						{target.commodity}
					</span>
					<span class="justify-self-end sm:justify-self-start">
						<Badge variant="outline">{target.kind}</Badge>
					</span>
					<span class="text-muted-foreground min-w-0 truncate text-xs">
						{target.currency ? `in ${target.currency}` : "no currency"}
					</span>
					<span class="justify-self-end text-right sm:justify-self-stretch">
						{#if target.latest}
							<span class="inline-flex flex-wrap items-baseline justify-end gap-x-1">
								<!-- A price is not a flow: the sign colours of the register would read as gain or loss. -->
								<MoneyAmount
									units={target.latest.price}
									currency={target.latest.currency}
									class="text-foreground"
								/>
								<span class="text-muted-foreground text-[0.625rem]">{target.latest.date}</span>
							</span>
						{:else}
							<span class="text-muted-foreground text-xs">none</span>
						{/if}
					</span>
					<Select.Root
						type="single"
						value={source}
						onValueChange={(next) => next && onchange(target, next)}
					>
						<Select.Trigger
							type="button"
							class="data-[size=default]:h-8 w-full sm:data-[size=default]:h-7"
							aria-label={`Price source for ${target.commodity}`}
							title={descriptions.get(source) ?? "Where the next quote comes from"}
							disabled={sources.length === 0}
						>
							<span data-slot="select-value" class="min-w-0 truncate">
								{source || "No source"}
							</span>
						</Select.Trigger>
						<Select.Content>
							{#each sources as candidate (candidate.name)}
								<Select.Item
									value={candidate.name}
									label={candidate.name}
									title={candidate.description}
								/>
							{/each}
						</Select.Content>
					</Select.Root>
					<Button
						class="h-8 justify-self-end px-2.5 sm:justify-self-stretch"
						disabled={fetching || source === ""}
						aria-label={fetching ? `Fetching ${target.commodity}` : `Fetch ${target.commodity}`}
						onclick={() => onfetch(target)}
					>
						{fetching ? "…" : "Fetch"}
					</Button>

					{#if state.status === "busy"}
						<p class="text-muted-foreground col-span-full text-xs">
							Fetching from {source}…
						</p>
					{:else if state.status === "quote"}
						<!--
							The answer inline, under its row: the date and the price exactly
							as the source gave them, so what would be written is what is
							read here.
						-->
						<p
							class="text-muted-foreground col-span-full flex flex-wrap items-baseline gap-x-1.5 text-xs"
						>
							<span>fetched</span>
							<MoneyAmount
								units={state.quote.price}
								currency={state.quote.currency}
								class="text-foreground"
							/>
							<span>{state.quote.date}</span>
							<span class="truncate font-mono text-[0.625rem]">{state.quote.source}</span>
						</p>
					{:else if state.status === "failed"}
						<p class="text-destructive col-span-full text-xs">{state.error}</p>
					{/if}
				</li>
			{/each}
		</ul>
	</div>
{/if}
