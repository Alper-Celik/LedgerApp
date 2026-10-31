<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<!--
	One commodity's price history: every day the journal says what it is worth,
	and where each statement came from.

	A page rather than a panel on the Prices screen. That screen is about
	fetching a quote and writing it into the journal — it has a row per
	commodity-and-currency pair, each with its own fetch state and source picker
	— while a history is a read-only view of what one commodity has been worth.
	Putting a chart, three source choices, a currency picker, a range and a row
	per day inside the fetch table would make one screen two screens. A route of
	its own also gives a history an address: a table row links here and the
	commodity is in the path.

	The page owns its controls and asks the backend again for each change, the
	way the inflation screen runs its report: the backend is the only thing that
	reads the ledger, and a failed query leaves the last answer under an alert
	rather than an empty screen.
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { resolve } from "$app/paths";
	import LineChart from "$lib/charts/line-chart.svelte";
	import type { ChartDatum, ChartSeries } from "$lib/charts/types.js";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import { commands, type PriceHistory } from "../../../bindings";
	import { messageOf } from "$lib/errors.js";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();

	type Source = "explicit" | "transactions" | "both";

	const SOURCES: readonly { id: Source; label: string; hint: string }[] = [
		{
			id: "explicit",
			label: "Explicit prices",
			hint: "Only the journal's own `price` directives",
		},
		{
			id: "transactions",
			label: "From transactions",
			hint: "Only rates implied by `{cost}` and `@price` on trades",
		},
		{
			id: "both",
			label: "Both",
			hint: "Every day; an explicit price wins a day a trade also prices",
		},
	];

	/** The currency picker's "no filter" value — bits-ui needs a non-empty value. */
	const ALL = "all";

	/**
	 * What the page shows. The route's load is the first answer; a control
	 * change replaces it, and a later route load (another ledger, a refresh)
	 * is adopted again.
	 */
	let history = $state<PriceHistory | null>(untrack(() => data.history));
	/** The load `history` was taken from, as the re-adoption check. */
	let adopted = untrack(() => data);

	let source = $state<Source>("both");
	let currency = $state(ALL);
	let from = $state("");
	let to = $state("");
	let busy = $state(false);
	/** The last query's refusal, shown above the answer it did not replace. */
	let queryError = $state("");
	/**
	 * Which query is the current one. Controls can be changed again before an
	 * answer arrives, and a slower earlier answer must not overwrite the newer
	 * control's — nor a previous ledger's answer outlive the switch.
	 */
	let latest = 0;

	$effect(() => {
		// A history, its controls and its rows describe one ledger. A new route
		// load is the truth again; keeping this visit's answer would show the
		// previous book's prices under the new ledger's name.
		if (data === adopted) return;
		adopted = data;
		latest += 1;
		history = data.history;
		queryError = "";
		source = "both";
		currency = ALL;
		from = "";
		to = "";
		busy = false;
	});

	const commodity = $derived(data.commodity);
	const points = $derived(history?.points ?? []);
	const loadError = $derived(data.error);

	/** The currencies actually present in the rows — the summary's min/max are money only for one. */
	const pointCurrencies = $derived([...new Set(points.map((point) => point.currency))]);
	/** The one currency the figures are in, when the answer has just one. */
	const singleCurrency = $derived(
		pointCurrencies.length === 1 ? (pointCurrencies[0] as string) : null
	);

	/**
	 * One chart per currency, never one chart for several. A commodity can be
	 * priced in more than one currency and those are not the same scale — a
	 * shared y-axis would draw 0.92 euro and 42 lira as the same line. Each
	 * panel holds one series, so its scale is the currency's own and its dates
	 * stay in the order the backend sorted them.
	 */
	const chartPanels = $derived.by(() => {
		const byCurrency = new Map<string, ChartDatum[]>();
		for (const point of points) {
			const series = byCurrency.get(point.currency) ?? [];
			series.push({ label: point.date, value: point.value, currency: point.currency });
			byCurrency.set(point.currency, series);
		}
		return [...byCurrency].map(([currency, series]) => ({
			currency,
			series: [{ name: currency, points: series }] satisfies ChartSeries[],
		}));
	});

	/** Show the currency picker only when there is a choice to make. */
	const showCurrencies = $derived((history?.currencies.length ?? 0) > 1);

	/**
	 * Why the answer is empty, said in terms of what the ledger has rather than
	 * a bare "no data": nothing at all, only the other source, or nothing in
	 * the chosen months. The counts are the backend's, before the range and
	 * limit narrow the answer.
	 */
	const emptyMessage = $derived.by(() => {
		if (!history || points.length > 0) return "";
		const counts = history.counts;
		if (counts.explicit === 0 && counts.transactions === 0) {
			return `The ledger does not price ${commodity} yet: it has no price directive and no trade with a cost or price for it.`;
		}
		// The other source is only worth naming when it has something to show
		// for the same commodity and currency; otherwise "switch source" would
		// send the reader to another empty answer.
		if (history.source === "explicit" && counts.explicit === 0 && counts.transactions > 0) {
			return `No explicit prices for ${commodity}: the ledger states it only through trades. Switch to “From transactions” to see them.`;
		}
		if (
			history.source === "transactions" &&
			counts.transactions === 0 &&
			counts.explicit > 0
		) {
			return `No trade prices ${commodity}: the ledger states it only with explicit price directives. Switch to “Explicit prices” to see them.`;
		}
		const scope = currency === ALL ? "" : ` in ${currency}`;
		const range = from !== "" || to !== "" ? " in this range" : "";
		return `No ${commodity} prices${scope}${range}.`;
	});

	/** Ask the backend for the same commodity under the chosen controls. */
	async function reload(next: {
		source?: Source;
		currency?: string;
		from?: string;
		to?: string;
	} = {}) {
		const wantedSource = next.source ?? source;
		const wantedCurrency = next.currency ?? currency;
		const wantedFrom = next.from ?? from;
		const wantedTo = next.to ?? to;
		const mine = ++latest;
		busy = true;
		queryError = "";
		try {
			const result = await commands.priceHistory(
				commodity,
				wantedCurrency === ALL ? null : wantedCurrency,
				wantedSource,
				wantedFrom === "" ? null : wantedFrom,
				wantedTo === "" ? null : wantedTo,
				null
			);
			// A newer control (or another ledger) has asked since; this answer
			// describes a question the screen no longer shows.
			if (mine !== latest) return;
			if (result.status === "error") {
				queryError = result.error;
				return;
			}
			history = result.data;
		} catch (cause) {
			if (mine !== latest) return;
			queryError = messageOf(cause);
		} finally {
			if (mine === latest) busy = false;
		}
	}

	function pickSource(next: string) {
		if (!next || next === source) return;
		source = next as Source;
		void reload({ source });
	}

	function pickCurrency(next: string | undefined) {
		if (!next || next === currency) return;
		currency = next;
		void reload({ currency });
	}

	function pickFrom(next: string) {
		from = next;
		void reload({ from });
	}

	function pickTo(next: string) {
		to = next;
		void reload({ to });
	}

	function clearRange() {
		from = "";
		to = "";
		void reload({ from: "", to: "" });
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<a class="text-muted-foreground hover:text-foreground text-xs" href={resolve("/prices")}>← Prices</a>
		<h1 class="text-lg font-semibold tracking-tight">{commodity}</h1>
		<span class="text-muted-foreground text-xs">price history</span>
		{#if busy}
			<span class="text-muted-foreground text-xs" role="status">Reading…</span>
		{/if}
	</div>

	{#if loadError && !history}
		<Alert variant="destructive">{loadError}</Alert>
	{:else if history}
		<div class="flex flex-wrap items-end gap-x-4 gap-y-2">
			<div class="flex flex-col gap-1">
				<span class="text-muted-foreground text-xs">Source</span>
				<ToggleGroup.Root
					type="single"
					value={source}
					onValueChange={pickSource}
					aria-label="Price source"
				>
					{#each SOURCES as option (option.id)}
						<ToggleGroup.Item value={option.id} title={option.hint}>
							{option.label}
						</ToggleGroup.Item>
					{/each}
				</ToggleGroup.Root>
			</div>

			{#if showCurrencies}
				<div class="flex flex-col gap-1">
					<span class="text-muted-foreground text-xs">Currency</span>
					<Select.Root type="single" value={currency} onValueChange={pickCurrency}>
						<Select.Trigger
							type="button"
							class="data-[size=default]:h-8 min-w-28"
							aria-label="Price currency"
						>
							<span data-slot="select-value">
								{currency === ALL ? "All currencies" : currency}
							</span>
						</Select.Trigger>
						<Select.Content>
							<Select.Item value={ALL} label="All currencies" />
							{#each history.currencies as code (code)}
								<Select.Item value={code} label={code} />
							{/each}
						</Select.Content>
					</Select.Root>
				</div>
			{/if}

			<div class="flex flex-col gap-1">
				<label class="text-muted-foreground text-xs" for="price-history-from">From month</label>
				<Input
					id="price-history-from"
					type="month"
					class="h-8 w-36"
					value={from}
					onchange={(event) => pickFrom(event.currentTarget.value)}
				/>
			</div>
			<div class="flex flex-col gap-1">
				<label class="text-muted-foreground text-xs" for="price-history-to">To month</label>
				<Input
					id="price-history-to"
					type="month"
					class="h-8 w-36"
					value={to}
					onchange={(event) => pickTo(event.currentTarget.value)}
				/>
			</div>
			{#if from !== "" || to !== ""}
				<button
					type="button"
					class="text-muted-foreground hover:text-foreground h-8 text-xs underline-offset-2 hover:underline"
					onclick={clearRange}
				>
					Clear range
				</button>
			{/if}
		</div>

		{#if queryError}
			<Alert variant="destructive">{queryError}</Alert>
		{/if}

		{#if points.length > 0}
			<p class="text-muted-foreground text-xs">
				{history.summary.points}
				{history.summary.points === 1 ? "point" : "points"}
				{#if history.summary.first}
					· {history.summary.first} → {history.summary.last}
				{/if}
				{#if singleCurrency && history.summary.min && history.summary.max}
					· min
					<MoneyAmount units={history.summary.min} currency={singleCurrency} class="text-foreground" />
					· max
					<MoneyAmount units={history.summary.max} currency={singleCurrency} class="text-foreground" />
				{/if}
			</p>

			{#each chartPanels as panel (panel.currency)}
				<div class="flex flex-col gap-1">
					<p class="text-muted-foreground text-xs">{commodity} in {panel.currency}</p>
					<LineChart series={panel.series} unit="day" legend={false} />
				</div>
			{/each}

			<div class="rounded-lg border">
				<div
					class="text-muted-foreground hidden grid-cols-[7rem_1fr_6rem_9rem] items-center gap-2 border-b px-3 py-1.5 text-xs sm:grid"
				>
					<span>Date</span>
					<span class="text-right">Price</span>
					<span>Currency</span>
					<span>Source</span>
				</div>
				<ul class="divide-border divide-y" data-slot="price-history-points">
					{#each points as point (`${point.date}|${point.currency}`)}
						<li
							class="grid grid-cols-[6rem_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 px-3 py-2 sm:grid-cols-[7rem_1fr_6rem_9rem] sm:gap-2"
						>
							<span class="text-xs tabular-nums">{point.date}</span>
							<span class="justify-self-end text-right sm:justify-self-stretch">
								<MoneyAmount
									units={point.value}
									currency={point.currency}
									class="text-foreground"
								/>
							</span>
							<span class="text-muted-foreground text-xs">{point.currency}</span>
							<span class="col-start-3 justify-self-end sm:col-start-auto sm:justify-self-start">
								<Badge variant={point.source === "explicit" ? "secondary" : "outline"}>
									{point.source}
								</Badge>
							</span>
						</li>
					{/each}
				</ul>
			</div>
		{:else}
			<p class="text-muted-foreground rounded-lg border border-dashed px-4 py-8 text-center text-xs">
				{emptyMessage}
			</p>
		{/if}
	{:else}
		<Alert variant="destructive">{loadError}</Alert>
	{/if}
</div>
