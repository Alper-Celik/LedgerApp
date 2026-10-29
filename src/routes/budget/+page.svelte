<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<!--
	The ledger's envelope budget as a multi-month grid, read-only.

	Every allocation the screen shows is the envelope plugin's own arithmetic:
	the plugin reads its configuration out of the ledger (`fund`, `adjust`,
	`zero`, `pad`, `rollover` under a `custom "envelope"` directive) and writes
	the resulting bookkeeping into `Equity:Envelope:` on every load. Editing a
	figure here would therefore be a second writer of the same numbers, and the
	next load — of the same ledger, after any edit anywhere — would overwrite
	whatever was typed. Until the configuration is the thing being edited, this
	page only reads it; the plugin stays the source of truth for what is
	assigned where.

	The shape follows Actual Budget's grid: a pinned month-summary strip (what
	each visible month still has to assign), a pinned column header (the month's
	Budgeted/Spent/Balance totals over three labelled columns), and the rows —
	group totals then its envelopes — as the only region that scrolls. Months are
	a sliding window, never a horizontally scrolled table: the width decides how
	many fit, the arrow keys and the strip move the window, and `0` returns it to
	the newest month.
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { ChevronLeftIcon, ChevronRightIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { commands, type BudgetRange } from "../../bindings";
	import BudgetAmount from "$lib/components/budget/budget-amount.svelte";
	import BudgetGridRow from "$lib/components/budget/budget-grid-row.svelte";
	import BudgetMonthCard from "$lib/components/budget/budget-month-card.svelte";
	import BudgetStackedRow from "$lib/components/budget/budget-stacked-row.svelte";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import {
		budgetColumns,
		budgetGridTemplate,
		budgetRows,
		clampWindowStart,
		newestWindowStart,
	} from "$lib/budget-grid.js";
	import {
		monthLabel,
		readCollapsedGroups,
		shortMonthLabel,
		writeCollapsedGroups,
	} from "$lib/budget.js";
	import { messageOf } from "$lib/errors.js";
	import { IsMobile } from "$lib/hooks/is-mobile.svelte.js";
	import { ledgerEpoch, ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import { cn } from "$lib/utils.js";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();

	/**
	 * A range read since the page loaded, or `null` while the route's own load
	 * still stands. A revision refetches the same window in one call rather than
	 * a month at a time: the grid holds several and they are one IPC.
	 */
	let fetched = $state<{ range: BudgetRange | null; error: string } | null>(null);
	const range = $derived(fetched ? fetched.range : data.range);
	const error = $derived(fetched ? fetched.error : data.error);
	const currency = $derived(range?.currency ?? "");
	let busy = $state(false);

	const months = $derived(range?.months ?? []);
	const total = $derived(months.length);

	/** The content width the scroll region reports, so the breakpoints read the room the grid actually has. */
	let width = $state(0);
	const isMobile = new IsMobile();
	/**
	 * How many months fit: the phone layout shows one, wider screens one more per
	 * 250px up to six. Measured on the scrolling region rather than the viewport,
	 * so a scrollbar or the sidebar is already paid for.
	 */
	const columns = $derived(isMobile.current ? 1 : budgetColumns(width));

	/**
	 * The window's first month as an index into `months`, or `null` for "the
	 * newest window" — the anchor that follows a resize instead of pinning a
	 * stale index, the position `0` returns to, and where the screen opens.
	 */
	let anchor = $state<number | null>(null);
	const first = $derived(
		clampWindowStart(anchor ?? newestWindowStart(columns, total), columns, total)
	);
	const visible = $derived(months.slice(first, first + columns));
	/** The shared column template: the bands and every row lay themselves out on it. */
	const template = $derived(budgetGridTemplate(visible.length));

	/** Groups the reader has folded away, remembered across reloads. */
	let collapsed = $state<string[]>(readCollapsedGroups());
	const rows = $derived(budgetRows(visible));
	/** Folded groups keep their own totals row; only their envelopes go. */
	const shownRows = $derived(
		rows.filter((row) => row.envelope === "" || !collapsed.includes(row.group))
	);

	/** The window as a heading for the empty states ("October 2026", or "August – October 2026"). */
	const windowLabel = $derived(
		visible.length === 0
			? ""
			: visible.length === 1
				? monthLabel(visible[0].month)
				: `${monthLabel(visible[0].month)} – ${monthLabel(visible[visible.length - 1].month)}`
	);

	function shift(delta: number) {
		if (total === 0) return;
		anchor = clampWindowStart(first + delta, columns, total);
	}

	/** A strip card or a phone tab: that month becomes the window's first. */
	function pick(month: string) {
		const index = months.findIndex((candidate) => candidate.month === month);
		if (index !== -1) anchor = clampWindowStart(index, columns, total);
	}

	function toggleGroup(name: string) {
		collapsed = collapsed.includes(name)
			? collapsed.filter((candidate) => candidate !== name)
			: [...collapsed, name];
		writeCollapsedGroups(collapsed);
	}

	/**
	 * The window is keyboard-navigable — ←/→ one month, `0` the newest — except
	 * while a field, a menu or the sidebar's phone sheet has the focus, where
	 * those keys mean something else.
	 */
	function onKeydown(event: KeyboardEvent) {
		if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
		const target = event.target;
		if (
			target instanceof HTMLElement &&
			target.closest(
				"input, textarea, select, [contenteditable='true'], [role='listbox'], [role='dialog']"
			)
		) {
			return;
		}
		if (event.key === "ArrowLeft") {
			event.preventDefault();
			shift(-1);
		} else if (event.key === "ArrowRight") {
			event.preventDefault();
			shift(1);
		} else if (event.key === "0") {
			event.preventDefault();
			anchor = null;
		}
	}

	/** The revision already reflected on screen, so the effect below runs on a bump. */
	let revision = ledgerRevision.value;
	/** The ledger the window on screen was read from. */
	let epoch = ledgerEpoch.value;

	async function refetch() {
		const current = untrack(() => range);
		busy = true;
		try {
			const result = await commands.budgetRange(current?.from || null, current?.to || null);
			// A failed read keeps the window on screen: the alert says what went
			// wrong rather than the grid blanking out under it.
			fetched =
				result.status === "error"
					? { range: current, error: result.error }
					: { range: result.data, error: "" };
		} catch (cause) {
			console.error("budgetRange failed", cause);
			fetched = { range: current, error: messageOf(cause) };
		} finally {
			busy = false;
		}
	}

	$effect(() => {
		// The plugin's entries are re-derived on every load of the ledger, so an
		// edit saved anywhere can move every month on screen; the revision brings
		// the grid back in step rather than leaving the figures stale.
		if (ledgerRevision.value === revision) return;
		revision = ledgerRevision.value;
		// Another journal's budget windows have nothing to do with the previous
		// one's: the route load already carries the new ledger's, so this visit's
		// own read is dropped rather than refetched against the old window.
		if (ledgerEpoch.value !== epoch) {
			epoch = ledgerEpoch.value;
			fetched = null;
			return;
		}
		void refetch();
	});
</script>

<svelte:window onkeydown={onKeydown} />

<!--
	The page fills the viewport the way the assistant's does — the bands are
	pinned inside it, so the grid must own the height rather than grow the
	document. Below `md` the app's own header (h-11) is above this box.
-->
<div class="flex h-[calc(100dvh-2.75rem)] w-full flex-col overflow-hidden md:h-dvh">
	<header class="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 px-4 pt-3 pb-2">
		<h1 class="text-lg font-semibold tracking-tight">Budget</h1>
		<span class="text-muted-foreground hidden text-xs md:inline">
			the envelope plugin's months, as it wrote them — read-only
		</span>
		<span class="text-muted-foreground ms-auto text-[10px]" aria-live="polite">
			{busy ? "updating…" : ""}
		</span>
		{#if !isMobile.current && months.length > 0}
			<!-- The rest of the navigation: the strip picks a start, these step the window. -->
			<div class="flex items-center gap-1">
				<Button
					variant="outline"
					size="icon-sm"
					aria-label="Earlier months"
					disabled={first === 0}
					onclick={() => shift(-1)}
				>
					<HugeiconsIcon icon={ChevronLeftIcon} strokeWidth={2} />
				</Button>
				<span class="text-muted-foreground min-w-24 text-center text-xs tabular-nums">
					{windowLabel}
				</span>
				<Button
					variant="outline"
					size="icon-sm"
					aria-label="Later months"
					disabled={first + columns >= total}
					onclick={() => shift(1)}
				>
					<HugeiconsIcon icon={ChevronRightIcon} strokeWidth={2} />
				</Button>
			</div>
		{/if}
	</header>

	{#if error && range}
		<div class="shrink-0 px-4 pb-2">
			<Alert variant="destructive">{error}</Alert>
		</div>
	{/if}

	{#if range && months.length > 0}
		{#if isMobile.current}
			<!--
				Phones: one month at a time. The month strip becomes tabs — every month
				of the range, thumb-sized, the current one marked — and the rows stack
				their three figures under each label instead of beside them.
			-->
			<div class="min-h-0 flex-1 overflow-y-auto" bind:clientWidth={width} aria-busy={busy}>
				<div class="bg-background sticky top-0 z-20 border-b">
					<div class="flex gap-1 overflow-x-auto px-3 py-2">
						{#each months as month (month.month)}
							{@const current = month.month === visible[0]?.month}
							<button
								type="button"
								class={cn(
									"focus-visible:ring-ring h-10 shrink-0 rounded-md px-3 text-xs font-medium transition-colors focus-visible:ring-2 focus-visible:outline-none",
									current
										? "bg-primary text-primary-foreground"
										: "bg-muted/60 text-muted-foreground hover:bg-muted"
								)}
								aria-pressed={current}
								aria-label={monthLabel(month.month)}
								title={monthLabel(month.month)}
								onclick={() => pick(month.month)}
							>
								{shortMonthLabel(month.month)}
							</button>
						{/each}
					</div>
					{#if visible[0]}
						<div class="border-t">
							<BudgetMonthCard
								month={visible[0].month}
								toBudget={visible[0].to_budget}
								{currency}
								totals={{
									budgeted: visible[0].budgeted,
									spent: visible[0].spent,
									balance: visible[0].balance,
								}}
							/>
						</div>
					{/if}
				</div>

				{#if rows.length === 0}
					<p class="text-muted-foreground px-3 py-4 text-xs">
						Nothing is budgeted in {windowLabel} — the envelope plugin wrote no envelopes
						under <code class="font-mono">Equity:Envelope:</code> for it.
					</p>
				{:else}
					{#each shownRows as row (row.key)}
						<BudgetStackedRow
							{row}
							cell={row.cells[0]}
							month={visible[0]?.month ?? ""}
							{currency}
							collapsed={collapsed.includes(row.group)}
							ontoggle={() => toggleGroup(row.group)}
						/>
					{/each}
				{/if}
			</div>
		{:else}
			<!--
				Wider screens: the three bands of the reference design. The two header
				bands stick to the top of the scroll region; everything below them is
				rows. All of it shares one column template, so a month's three figures
				line up from the summary card to the last envelope.
			-->
			<div
				class="min-h-0 flex-1 overflow-y-auto"
				role="table"
				aria-label="Envelope budget by month"
				aria-busy={busy}
				bind:clientWidth={width}
			>
				<div
					role="row"
					class="bg-card sticky top-0 z-30 grid h-14 items-stretch border-b"
					style:grid-template-columns={template}
				>
					<div role="cell" class="flex flex-col justify-center px-3">
						<span class="text-muted-foreground text-[10px] tracking-wide uppercase">To budget</span>
					</div>
					{#each visible as month (month.month)}
						<div role="cell" class="min-w-0 border-s">
							<BudgetMonthCard
								month={month.month}
								toBudget={month.to_budget}
								{currency}
								selected={month.month === visible[0]?.month}
								onpick={() => pick(month.month)}
							/>
						</div>
					{/each}
				</div>

				<div
					role="row"
					class="bg-background sticky top-14 z-20 grid h-11 items-stretch border-b"
					style:grid-template-columns={template}
				>
					<div role="cell" class="flex items-center px-3">
						<span class="text-muted-foreground text-[10px] tracking-wide uppercase">Category</span>
					</div>
					{#each visible as month (month.month)}
						<div class="grid grid-cols-3 border-s">
							{#each [{ label: "Budgeted", units: month.budgeted }, { label: "Spent", units: month.spent }, { label: "Balance", units: month.balance }] as figure (figure.label)}
								<div role="columnheader" class="flex flex-col justify-center px-1 text-right">
									<span class="text-muted-foreground text-[10px] leading-tight">
										{figure.label}
									</span>
									<BudgetAmount units={figure.units} {currency} class="text-[11px] leading-tight font-medium" />
								</div>
							{/each}
						</div>
					{/each}
				</div>

				{#if rows.length === 0}
					<div role="row" class="px-3 py-4">
						<div role="cell" class="text-muted-foreground max-w-md text-xs">
							Nothing is budgeted in {windowLabel} — the envelope plugin wrote no envelopes
							under <code class="font-mono">Equity:Envelope:</code> for it.
						</div>
					</div>
				{:else}
					{#each shownRows as row (row.key)}
						<BudgetGridRow
							{row}
							{template}
							{currency}
							months={visible}
							collapsed={collapsed.includes(row.group)}
							ontoggle={() => toggleGroup(row.group)}
						/>
					{/each}
				{/if}
			</div>
		{/if}
	{:else}
		<!--
			Nothing to lay out, and the backend's own sentence below says whether
			that is "a ledger not yet budgeted" or a read that failed. The empty
			state stays calm rather than destructive: a ledger without an envelope
			budget is the normal state of one, not a failure.
		-->
		<div class="min-h-0 flex-1 overflow-y-auto p-4" bind:clientWidth={width}>
			<div class="flex max-w-md flex-col gap-2">
				{#if range === null}
					<h2 class="text-sm font-medium">The budget could not be read</h2>
					<p class="text-muted-foreground text-xs">
						The backend did not return a month range. Open a ledger, or fix what the message
						below reports, and the grid will fill in.
					</p>
				{:else}
					<h2 class="text-sm font-medium">No envelope budget yet</h2>
					<p class="text-muted-foreground text-xs">
						The envelope plugin writes the budget: once the ledger configures it, its funding,
						spending and rollovers land under <code class="font-mono">Equity:Envelope:</code>
						and the months appear here.
					</p>
				{/if}
				{#if error}
					<Alert variant="destructive">{error}</Alert>
				{/if}
			</div>
		</div>
	{/if}
</div>
