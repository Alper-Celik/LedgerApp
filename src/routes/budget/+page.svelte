<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<!--
	The ledger's envelope budget, read-only.

	Every allocation the screen shows is the envelope plugin's own arithmetic:
	the plugin reads its configuration out of the ledger (`fund`, `adjust`,
	`zero`, `pad`, `rollover` under a `custom "envelope"` directive) and writes
	the resulting bookkeeping into `Equity:Envelope:` on every load. Editing a
	figure here would therefore be a second writer of the same numbers, and the
	next load — of the same ledger, after any edit anywhere — would overwrite
	whatever was typed. Until the configuration is the thing being edited, this
	page only reads it; the plugin stays the source of truth for what is
	assigned where.
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import BudgetMonthView from "$lib/components/budget/budget-month.svelte";
	import { Alert, AlertDescription } from "$lib/components/ui/alert/index.js";
	import { commands, type BudgetMonth } from "../../bindings";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();

	/**
	 * A month read since the page loaded, or `null` while the route's own load
	 * still stands. A jump refetches rather than working a month out of its
	 * neighbours: each month's figures are the backend's to derive.
	 */
	let fetched = $state<{ budget: BudgetMonth | null; error: string } | null>(null);
	let busy = $state(false);
	const budget = $derived(fetched ? fetched.budget : data.budget);
	const error = $derived(fetched ? fetched.error : data.error);

	/** The revision already reflected on screen, so the effect below runs on a bump. */
	let revision = ledgerRevision.value;

	async function show(next: string | null) {
		busy = true;
		try {
			const result = await commands.budget(next);
			// A failed month keeps the one on screen: the alert says what went
			// wrong rather than the page blanking out under it.
			const shown = untrack(() => budget);
			fetched =
				result.status === "error"
					? { budget: shown, error: result.error }
					: { budget: result.data, error: "" };
		} catch (cause) {
			console.error("budget failed", cause);
			const message = cause instanceof Error ? cause.message : String(cause);
			fetched = { budget: untrack(() => budget), error: message };
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		// The plugin's entries are re-derived on every load of the ledger, so an
		// edit saved anywhere can move the month on screen; the revision brings
		// it back in step rather than leaving the figures stale.
		if (ledgerRevision.value === revision) return;
		revision = ledgerRevision.value;
		void show(untrack(() => budget?.month ?? null));
	});
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Budget</h1>
		<span class="text-muted-foreground text-xs">
			the envelope plugin's months, as it wrote them — read-only
		</span>
	</div>

	{#if error && budget}
		<Alert variant="destructive">{error}</Alert>
	{/if}

	{#if budget}
		<BudgetMonthView {budget} {busy} onmonth={show} />
	{:else}
		<!--
			Nothing to page through, and the backend's own sentence below says why.
			The empty state stays calm rather than destructive: a ledger that has
			not been budgeted yet is the normal state of one, not a failure.
		-->
		<div class="flex flex-col gap-3">
			<div class="flex flex-col gap-0.5">
				<h2 class="text-sm font-medium">No budget to show</h2>
				<p class="text-muted-foreground text-xs">
					The envelope plugin writes the budget: once the ledger configures it, its funding,
					spending and rollovers land under <code class="font-mono">Equity:Envelope:</code> and
					the months appear here.
				</p>
			</div>
			{#if error}
				<Alert variant="destructive">{error}</Alert>
			{/if}
		</div>
	{/if}
</div>
