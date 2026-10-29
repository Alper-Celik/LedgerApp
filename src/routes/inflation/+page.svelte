<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<!--
	Inflation: the monthly price levels the app can restate money with, and the
	comparison they exist for — how the user's own spending moved once the
	index's movement is taken out of it.

	Two halves, deliberately apart. An index card belongs to its publisher: a
	refresh answers on that card alone, so a provider that is down or a series
	with no public endpoint does not stop the rest of the screen. The report
	belongs to the ledger, and the backend is what runs it; this page chooses
	which series and how many months, and shows the refusal it gets back.
-->
<script lang="ts">
	import { untrack } from "svelte";
	import ImportIndexSheet from "$lib/components/inflation/import-index-sheet.svelte";
	import IndexCard from "$lib/components/inflation/index-card.svelte";
	import LifestylePanel from "$lib/components/inflation/lifestyle-panel.svelte";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { commands, type IndexSeries, type LifestyleReport } from "../../bindings";
	import { messageOf } from "$lib/errors.js";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();

	/**
	 * What this visit changed, or `null` while the page load's own answer stands.
	 * The route's data stays the truth until an action here replaces part of it,
	 * so a navigation that reloads the route is reflected rather than shadowed by
	 * a snapshot taken at mount.
	 */
	let session = $state<{
		series: IndexSeries[];
		report: LifestyleReport | null;
		indexId: string;
		months: number;
		loadError: string;
	} | null>(null);

	/** The load the answer above was computed against (plain: see `branches`). */
	let adopted = untrack(() => data);

	$effect(() => {
		// A report, a series list and a chosen index all describe one ledger.
		// Another ledger's load (or any re-run of this route's) is the truth
		// again: keeping this visit's answer would show the previous books'
		// spending under the new ledger's name.
		if (data === adopted) return;
		adopted = data;
		session = null;
	});

	const view = $derived(
		session ?? {
			series: data.series,
			report: data.report,
			indexId: data.report?.index.id ?? data.series[0]?.id ?? "",
			months: 12,
			loadError: data.error,
		}
	);
	const series = $derived(view.series);
	const report = $derived(view.report);
	/** The series the report is for, or the one the next run will ask for. */
	const indexId = $derived(view.indexId);
	/** The range the report covers, in months. */
	const months = $derived(view.months);
	/** What the page load could not do, until something proves it can now. */
	const loadError = $derived(view.loadError);

	/** The last report run's refusal. */
	let reportError = $state("");
	let busy = $state(false);
	let importOpen = $state(false);
	/** Per-series refresh in flight, and its publisher's refusal by id. */
	let refreshing = $state<Record<string, boolean>>({});
	let refreshError = $state<Record<string, string>>({});

	/** Replace the part of the state an answer changed. */
	function save(changes: Partial<typeof view>) {
		session = { ...view, ...changes };
	}

	/**
	 * Run the report for a series and a range. The choice on screen only moves
	 * once the backend has answered, so a refused range keeps the figures that
	 * did work under an alert saying what went wrong.
	 */
	async function runReport(nextMonths: number, nextIndex: string) {
		if (nextIndex === "") return;
		busy = true;
		reportError = "";
		try {
			const result = await commands.lifestyleReport(nextMonths, nextIndex);
			if (result.status === "error") {
				reportError = result.error;
				return;
			}
			// A report this page can run is proof the index store reads.
			save({
				report: result.data,
				indexId: nextIndex,
				months: nextMonths,
				loadError: "",
			});
		} catch (cause) {
			reportError = messageOf(cause);
		} finally {
			busy = false;
		}
	}

	/** The list as the backend keeps it: one series per id, id-sorted. */
	function withSeries(one: IndexSeries) {
		return [...series.filter((other) => other.id !== one.id), one].sort((a, b) =>
			a.id.localeCompare(b.id)
		);
	}

	/**
	 * Ask one series' publisher for its newest months. A provider without a
	 * public endpoint answers with its explanation, which is shown on the card
	 * as it came — the sentence is the useful part, not a code.
	 */
	async function refresh(one: IndexSeries) {
		refreshing[one.id] = true;
		delete refreshError[one.id];
		try {
			const result = await commands.indexRefresh(one.id);
			if (result.status === "error") {
				refreshError[one.id] = result.error;
				return;
			}
			save({ series: withSeries(result.data), loadError: "" });
			// The refreshed months are what the report's real figures rest on.
			if (result.data.id === indexId) await runReport(months, indexId);
		} catch (cause) {
			refreshError[one.id] = messageOf(cause);
		} finally {
			refreshing[one.id] = false;
		}
	}

	/** An imported series is stored: list it, and compare against it if nothing was picked. */
	async function imported(one: IndexSeries) {
		save({ series: withSeries(one), loadError: "" });
		importOpen = false;
		// The first series to arrive, and a series the report is already drawn
		// from whose months just changed, both need the report run again.
		if (indexId === "" || one.id === indexId) await runReport(months, one.id);
	}
</script>

<div class="flex flex-col gap-6 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Inflation</h1>
		<span class="text-muted-foreground text-xs">the price levels your spending is read against</span>
	</div>

	{#if loadError}
		<Alert variant="destructive" class="whitespace-pre-line">{loadError}</Alert>
	{/if}

	<section class="flex flex-col gap-3" aria-labelledby="indexes-heading">
		<div class="flex flex-wrap items-center gap-2">
			<h2 id="indexes-heading" class="text-sm font-semibold tracking-tight">Indexes</h2>
			<span class="text-muted-foreground text-xs">stored monthly series</span>
			<Button class="ms-auto h-8" variant="secondary" onclick={() => (importOpen = true)}>
				Import CSV
			</Button>
		</div>

		<p class="text-muted-foreground max-w-prose text-xs">
			A point is the price of one unit of the currency at that month — a level, not a percentage.
			Because every month has a level, a month's spending can be restated in an earlier month's
			money: multiply by the earlier level, divide by the later one. That is all “real” means
			here.
		</p>

		{#if series.length === 0}
			<!--
				Two ways in, and neither is the other's fallback: a publisher's CSV
				needs nothing but the paste, while a refresh needs an id a provider
				knows — and with no series stored there is no card to press.
			-->
			<p class="text-muted-foreground text-xs">
				No index is stored yet — import or refresh an index first. Import takes a publisher's
				<code class="font-mono">date,value</code> rows; a refresh asks a stored series' publisher
				for its newest months.
			</p>
		{:else}
			<!-- Cards stack on a phone and pair up as the window widens. -->
			<div class="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
				{#each series as one (one.id)}
					<IndexCard
						series={one}
						refreshing={refreshing[one.id] === true}
						error={refreshError[one.id] ?? ""}
						onrefresh={() => void refresh(one)}
					/>
				{/each}
			</div>
		{/if}
	</section>

	<section class="flex flex-col gap-3" aria-labelledby="lifestyle-heading">
		<div class="flex flex-wrap items-center gap-2">
			<h2 id="lifestyle-heading" class="text-sm font-semibold tracking-tight">
				Lifestyle inflation
			</h2>
			<span class="text-muted-foreground text-xs">
				your spending, restated in an earlier month's money
			</span>
		</div>

		<LifestylePanel
			{series}
			{report}
			{indexId}
			{months}
			{busy}
			error={reportError}
			onselect={(id) => void runReport(months, id)}
			onmonths={(count) => void runReport(count, indexId)}
		/>
	</section>

	<ImportIndexSheet
		open={importOpen}
		onclose={() => (importOpen = false)}
		onimported={(one) => void imported(one)}
	/>
</div>
