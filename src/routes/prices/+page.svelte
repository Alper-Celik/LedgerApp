<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { invalidateAll } from "$app/navigation";
	import PriceTable from "$lib/components/prices/price-table.svelte";
	import WritePricesSheet from "$lib/components/prices/write-prices-sheet.svelte";
	import {
		defaultFile,
		defaultSource,
		pairKey,
		type RowState,
	} from "$lib/components/prices/prices.js";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { commands, type PriceQuote, type PriceTarget } from "../../bindings";
	import { messageOf } from "$lib/errors.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import type { PageProps } from "./$types";

	/**
	 * Prices: what the ledger can price, what a source says a pair is worth
	 * today, and the way a fetched answer becomes a directive in a journal file.
	 *
	 * The two halves of the screen are deliberately apart. Each row fetches on
	 * its own, so a source that is down — or a ticker it does not carry — answers
	 * on its row and leaves every other row alone. The write is a choice made
	 * after the fact, in a sheet, and it goes through the backend's own edit
	 * pipeline rather than the browser's; this page never touches a file.
	 */
	let { data }: PageProps = $props();

	/** Each row's own fetch, by pair. */
	let states = $state<Record<string, RowState>>({});
	/** The source picked per row, where one has been picked; rows fall back to the kind's default. */
	let picked = $state<Record<string, string>>({});
	/** Quotes fetched this visit that the ledger does not hold yet, by pair. */
	let fetched = $state<Record<string, PriceQuote>>({});
	let writeOpen = $state(false);

	const busy = $derived(Object.values(states).some((state) => state.status === "busy"));
	const quotes = $derived(Object.values(fetched));
	/** The file a write defaults to: the root journal, or the journal's first file. */
	const targetFile = $derived(defaultFile(data.files, data.root));

	function stateOf(target: PriceTarget): RowState {
		return states[pairKey(target.commodity, target.currency)] ?? { status: "idle" };
	}

	function sourceOf(target: PriceTarget): string {
		return (
			picked[pairKey(target.commodity, target.currency)] ??
			defaultSource(target.kind, data.sources)
		);
	}

	/**
	 * Ask one row's source for its quote. The request is a list of one so a
	 * refusal belongs to this row: a batch would answer with one sentence for
	 * the whole table and the rest of it would go unquoted.
	 */
	async function fetchTarget(target: PriceTarget) {
		const key = pairKey(target.commodity, target.currency);
		const source = sourceOf(target);
		if (source === "") {
			states[key] = { status: "failed", error: "This build offers no price source." };
			return;
		}
		states[key] = { status: "busy" };
		try {
			// A target without a currency lets the source choose the quote's, which
			// the empty string asks for; `date: null` asks for the newest quote.
			const result = await commands.fetchPrices([
				{ commodity: target.commodity, currency: target.currency ?? "", source, date: null },
			]);
			if (result.status === "error") {
				states[key] = { status: "failed", error: result.error };
				return;
			}
			// One outcome per request, in request order: this row's own answer.
			const outcome = result.data[0];
			if (!outcome) {
				states[key] = { status: "failed", error: `${source} returned no answer.` };
				return;
			}
			if ("failed" in outcome && outcome.failed) {
				states[key] = { status: "failed", error: outcome.failed.message };
				return;
			}
			const quote = outcome.quote;
			if (!quote) {
				states[key] = { status: "failed", error: `${source} returned no quote.` };
				return;
			}
			states[key] = { status: "quote", quote };
			fetched[pairKey(quote.commodity, quote.currency)] = quote;
		} catch (cause) {
			states[key] = { status: "failed", error: messageOf(cause) };
		}
	}

	/** Every visible row at once; each settles on its own row. */
	async function fetchAll() {
		await Promise.all(data.targets.map((target) => fetchTarget(target)));
	}

	/**
	 * What the sheet wrote: those quotes are the file's now, so they leave the
	 * waiting list and the route reloads — every row's ledger price comes back
	 * as the new directive, and the rest of the app sees the new revision.
	 */
	async function written(saved: PriceQuote[]) {
		for (const quote of saved) delete fetched[pairKey(quote.commodity, quote.currency)];
		bumpLedgerRevision();
		await invalidateAll();
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Prices</h1>
		<span class="text-muted-foreground text-xs">what the ledger can price, and where to ask</span>
		<div class="ms-auto flex items-center gap-2">
			<Button
				class="h-8"
				variant="secondary"
				disabled={busy || data.targets.length === 0}
				onclick={fetchAll}
			>
				{busy ? "Fetching…" : "Fetch all"}
			</Button>
			<Button
				class="h-8"
				disabled={quotes.length === 0}
				aria-label="Add fetched prices to a journal file"
				onclick={() => (writeOpen = true)}
			>
				{quotes.length > 0 ? `Add to file… (${quotes.length})` : "Add to file…"}
			</Button>
		</div>
	</div>

	{#if data.error}
		<!-- Nothing was read, so there is no table to show under the reason. -->
		<Alert variant="destructive">{data.error}</Alert>
	{:else}
		<PriceTable
			targets={data.targets}
			sources={data.sources}
			{stateOf}
			{sourceOf}
			onchange={(target, source) => {
				picked[pairKey(target.commodity, target.currency)] = source;
			}}
			onfetch={fetchTarget}
		/>
	{/if}

	<WritePricesSheet
		open={writeOpen}
		{quotes}
		files={data.files}
		defaultFile={targetFile}
		onclose={() => (writeOpen = false)}
		onwritten={written}
	/>
</div>
