<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { IndexSeries } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { lastPoint, pointsLabel } from "$lib/components/inflation/inflation.js";

	/**
	 * One stored series as a card: what it is, where it came from, how far it
	 * reaches, and the one action that changes it — a refresh that goes back to
	 * its publisher. The publisher's answer is this card's business alone, so a
	 * provider that is down or a series with no endpoint reports here and leaves
	 * the other cards untouched.
	 */
	let {
		series,
		refreshing = false,
		error = "",
		onrefresh,
	}: {
		series: IndexSeries;
		/** A refresh of this series is in flight. */
		refreshing?: boolean;
		/** The provider's refusal, verbatim, from the last refresh. */
		error?: string;
		onrefresh: () => void;
	} = $props();

	const last = $derived(lastPoint(series));
	const first = $derived(series.points[0] ?? null);
</script>

<section class="flex flex-col gap-2 rounded-xl border border-border bg-card p-3">
	<div class="flex flex-wrap items-center gap-x-2 gap-y-1">
		<h3 class="text-sm font-semibold tracking-tight">{series.label}</h3>
		{#if series.unofficial}
			<!--
				The one warning the store itself carries: ENAG is an independent
				research group, not a statistical agency, and a series it publishes
				is not an official figure however often it is quoted.
			-->
			<Badge
				variant="destructive"
				title="Published by an independent body, not a national statistical agency"
			>
				unofficial
			</Badge>
		{/if}
		<span class="text-muted-foreground ms-auto font-mono text-[0.625rem]">{series.id}</span>
	</div>

	<div class="text-muted-foreground flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-xs">
		<span>{series.source || "no source given"}</span>
		<span aria-hidden="true">·</span>
		<span title="The money the index measures — a report in another currency is refused">
			{series.currency}
		</span>
		<span aria-hidden="true">·</span>
		<span>{pointsLabel(series.points.length)}</span>
	</div>

	<div class="text-muted-foreground flex flex-wrap items-baseline gap-x-1.5 text-xs">
		{#if last && first}
			<!-- The dates the store holds: the first of the month each value belongs to. -->
			<span>last <span class="text-foreground font-mono">{last.date}</span></span>
			<span aria-hidden="true">·</span>
			<span>from <span class="text-foreground font-mono">{first.date}</span></span>
			<span aria-hidden="true">·</span>
			<span class="truncate" title={last.value}>
				value <span class="text-foreground font-mono">{last.value}</span>
			</span>
		{:else}
			<span>No points stored, so nothing can be compared against this series yet.</span>
		{/if}
	</div>

	<div class="mt-auto flex flex-wrap items-center gap-2 pt-1">
		<Button
			class="h-8"
			variant="secondary"
			disabled={refreshing}
			aria-label={`Refresh ${series.label} from its publisher`}
			onclick={onrefresh}
		>
			{refreshing ? "Refreshing…" : "Refresh"}
		</Button>
		<span class="text-muted-foreground text-[0.625rem]">
			asks {series.source || "the publisher"} for its newest months
		</span>
	</div>

	{#if error}
		<!-- Verbatim: a provider with no public endpoint answers with an explanation, not a code. -->
		<Alert variant="destructive">{error}</Alert>
	{/if}
</section>
