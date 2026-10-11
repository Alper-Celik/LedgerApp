<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { invalidateAll } from "$app/navigation";
	import ChevronRight from "@lucide/svelte/icons/chevron-right";
	import Pencil from "@lucide/svelte/icons/pencil";
	import { commands, type PostingView, type TransactionView } from "../../bindings";
	import { Button } from "$lib/components/ui/button/index.js";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import { isSubAccountOf } from "$lib/accounts.js";
	import { signOf, sumDecimals } from "$lib/money.js";
	import { smoothHeight } from "$lib/smooth-height.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";


	/**
	 * The three register styles, GnuCash's: `effect` is the basic ledger (one
	 * line per entry, the Transfer column naming the other side), `split` is the
	 * auto-split ledger (the legs that hit this account, the rest on demand),
	 * `full` is the transaction journal (every leg of every entry).
	 *
	 * Each style is one row of data — headings and which postings show — so the
	 * markup renders one shape and never branches on the selected style.
	 */
	const VIEWS = [
		{
			id: "effect",
			label: "Effect",
			transferColumn: "Transfer",
			/** Names the other side of the entry (the basic ledger's behaviour). */
			namesOtherSide: true,
			amountColumns: ["Deposit", "Withdrawal"],
			/** Postings appear only for the current entry, and any entry can open. */
			legsWhenCurrentOnly: true,
			showAllPostings: true,
			expandWhen: "any-entry",
		},
		{
			id: "split",
			label: "Splits",
			transferColumn: "Account",
			namesOtherSide: false,
			amountColumns: ["Tot Deposit", "Tot Withdrawal"],
			legsWhenCurrentOnly: false,
			showAllPostings: false,
			expandWhen: "hidden-legs",
		},
		{
			id: "full",
			label: "Full",
			transferColumn: "Account",
			namesOtherSide: false,
			amountColumns: ["Tot Deposit", "Tot Withdrawal"],
			legsWhenCurrentOnly: false,
			showAllPostings: true,
			expandWhen: "none",
		},
	] as const;
	type ViewId = (typeof VIEWS)[number]["id"];
	const VIEW_KEY = "account_view";

	/**
	 * Column widths, defined once: the register's header/entry table and the
	 * nested table inside each entry's leg block both use them, so the legs line
	 * up with the columns they belong to.
	 */
	const COLUMNS = ["3rem", "6rem", "minmax(0, 1fr)", "14rem", "7rem", "7rem"];

	/** The style choice sticks across accounts and reloads. */
	let view = $state<ViewId>(
		(VIEWS.find((candidate) => candidate.id === localStorage.getItem(VIEW_KEY))?.id ?? "effect") as ViewId
	);
	let mode = $derived(VIEWS.find((candidate) => candidate.id === view) ?? VIEWS[0]);

	/** The register's *current transaction*, as GnuCash highlights it. */
	let current = $state<number | null>(null);
	/**
	 * In-place editing, the way GnuCash's register works: the current entry's
	 * cells *become* fields. `draft` holds the values while editing; Enter (or
	 * leaving the row) saves, Escape cancels.
	 */
	let editing = $state<number | null>(null);
	let draft = $state<{ date: string; payee: string; narration: string; units: string[] } | null>(null);
	let editError = $state("");

	function startEdit(tx: TransactionView) {
		editing = tx.id;
		editError = "";
		draft = {
			date: tx.date,
			payee: tx.payee ?? "",
			narration: tx.narration,
			units: tx.postings.map((posting) => posting.units),
		};
	}

	function cancelEdit() {
		editing = null;
		draft = null;
		editError = "";
	}

	/**
	 * GnuCash commits when the cursor leaves the transaction. The entry row and
	 * its leg block are separate table rows, so both carry `data-entry-id` and
	 * the check follows that, not containment.
	 */
	function commitOnLeave(event: FocusEvent, tx: TransactionView) {
		if (editing !== tx.id) return;
		const next = event.relatedTarget;
		const owner = next instanceof Element ? next.closest("[data-entry-id]") : null;
		if (owner?.getAttribute("data-entry-id") !== String(tx.id)) saveEdit(tx);
	}

	async function saveEdit(tx: TransactionView) {
		if (!draft || editing !== tx.id) return;
		const result = await commands.updateTransaction({
			id: tx.id,
			date: draft.date,
			flag: tx.flag,
			payee: draft.payee.trim() === "" ? null : draft.payee,
			narration: draft.narration,
			postings: draft.units.map((units, index) => ({ index, units })),
		});
		if (result.status === "error") {
			editError = result.error;
			return;
		}
		cancelEdit();
		bumpLedgerRevision();
		await invalidateAll();
	}
	let select = (tx: TransactionView) => (current = current === tx.id ? null : tx.id);

	$effect(() => {
		localStorage.setItem(VIEW_KEY, view);
	});

	let { account, transactions }: { account: string; transactions: TransactionView[] } = $props();

	let hiddenCount = (tx: TransactionView) =>
		tx.postings.filter((posting) => !isSubAccountOf(posting.account, account)).length;

	/**
	 * The account's total for this transaction, per commodity — GnuCash's
	 * transaction-line total: the legs that touch the account summed, and put in
	 * the column its sign calls for. Amounts keep their exact decimals
	 * (`sumDecimals` adds integers, not floats) and every commodity the account
	 * was touched in is reported, not just its main one.
	 */
	let flows = (tx: TransactionView) => {
		const legs: Record<string, string[]> = {};
		for (const posting of tx.postings) {
			if (!isSubAccountOf(posting.account, account)) continue;
			(legs[posting.currency] ??= []).push(posting.units);
		}
		const deposits: { units: string; currency: string }[] = [];
		const withdrawals: { units: string; currency: string }[] = [];
		for (const [currency, units] of Object.entries(legs)) {
			const total = sumDecimals(units);
			const sign = signOf(total);
			if (sign > 0) deposits.push({ units: total, currency });
			else if (sign < 0) withdrawals.push({ units: total, currency });
		}
		return { deposits, withdrawals };
	};

	/**
	 * The other side of the entry, as the basic ledger's Transfer column shows
	 * it: the counter-account for a simple entry, "-- Split Transaction --" when
	 * several accounts are involved (GnuCash's wording), and nothing when every
	 * leg sits inside this account already.
	 */
	let transferLabel = (tx: TransactionView) => {
		const others = [
			...new Set(
				tx.postings
					.filter((posting) => !isSubAccountOf(posting.account, account))
					.map((posting) => posting.account)
			),
		];
		if (others.length === 0) return "—";
		return others.length === 1 ? others[0] : "-- Split Transaction --";
	};
</script>

{#snippet amounts(values: { units: string; currency: string }[], tone: "in" | "out")}
	<span class="inline-flex flex-wrap justify-end gap-x-2">
		{#each values as value, index (index)}
			<MoneyAmount
				units={value.units}
				currency={value.currency}
				signed={false}
				class={tone === "out" ? "text-red-600 dark:text-red-400" : ""}
			/>
		{/each}
	</span>
{/snippet}

{#snippet postingRow(
	posting: PostingView,
	affecting: boolean,
	position: number,
	index: number,
	legDraft: { units: string[] } | null
)}
	{@const outward = posting.units.startsWith("-")}
	<!-- Legs: hairline rule, alternating tint so consecutive splits stay countable. -->
	<tr class="border-border/30 border-t {position % 2 === 0 ? 'bg-foreground/[0.035]' : ''}">
		<td></td>
		<td class="border-border/40 border-l"></td>
		<td class="border-border/40 border-l"></td>
		<!--
			Weight, not opacity, separates the legs that hit this account: an alpha
			would drop small text under the 4.5:1 contrast floor.
		-->
		<td
			class="border-s-2 border-s-muted-foreground/25 truncate py-1 pr-3 pl-3 text-foreground/75
				{affecting ? 'font-medium' : ''}"
		>
			{posting.account}
		</td>
		<td class="border-border/40 border-l overflow-hidden px-2 py-1 text-right">
			{#if legDraft && !outward}
				<input
					class="bg-background/70 focus:ring-ring w-full rounded-sm px-1 py-0.5 text-right text-xs tabular-nums outline-none focus:ring-1"
					bind:value={legDraft.units[index]}
					aria-label={`Amount on ${posting.account}`}
				/>
			{:else if !outward}
				{@render amounts([{ units: posting.units, currency: posting.currency }], "in")}
			{/if}
		</td>
		<td class="border-border/40 border-l overflow-hidden px-2 py-1 pr-3 text-right">
			{#if legDraft && outward}
				<input
					class="bg-background/70 focus:ring-ring w-full rounded-sm px-1 py-0.5 text-right text-xs tabular-nums outline-none focus:ring-1"
					bind:value={legDraft.units[index]}
					aria-label={`Amount on ${posting.account}`}
				/>
			{:else if outward}
				{@render amounts([{ units: posting.units, currency: posting.currency }], "out")}
			{/if}
		</td>
	</tr>
{/snippet}

<div class="flex flex-col gap-3">
	<div class="flex items-center justify-end gap-1">
		{#each VIEWS as option (option.id)}
			<Button
				size="sm"
				variant={view === option.id ? "secondary" : "ghost"}
				onclick={() => (view = option.id)}
			>
				{option.label}
			</Button>
		{/each}
	</div>

	<!--
		Fixed layout: widths come from the colgroup, never from content, so a long
		description or an opened entry cannot shift the amount columns.
	-->
	<div class="overflow-x-auto">
		{#key view}
			<table
				class="w-full min-w-[44rem] animate-in fade-in table-fixed text-xs duration-[var(--motion-duration)]
					ease-[var(--motion-ease-entering)] motion-reduce:animate-none"
			>
				<colgroup>
					{#each COLUMNS as width, index (index)}
						<col style="width: {width}" />
					{/each}
				</colgroup>
				<thead class="text-muted-foreground">
					<tr>
						<th></th>
						<th class="border-border/40 border-l pl-2 text-left font-normal">Date</th>
						<th class="border-border/40 border-l pl-2 text-left font-normal">Description</th>
						<th class="border-border/40 border-l pl-2 text-left font-normal">
							{mode.transferColumn}
						</th>
						<th class="border-border/40 border-l px-2 text-right font-normal">
							{mode.amountColumns[0]}
						</th>
						<th class="border-border/40 border-l px-2 pr-3 text-right font-normal">
							{mode.amountColumns[1]}
						</th>
					</tr>
				</thead>
				<tbody>
					{#each transactions as tx, index (tx.id)}
						{@const flow = flows(tx)}
						{@const legs =
							mode.legsWhenCurrentOnly && current !== tx.id
								? []
								: tx.postings
										.map((posting, index) => ({ posting, index }))
										.filter(
											(leg) =>
												mode.showAllPostings ||
												current === tx.id ||
												isSubAccountOf(leg.posting.account, account)
										)}
						<!--
							Zebra steps are alphas of the foreground, not of `--muted`: the
							former inverts with the theme (darker on light, lighter on dark),
							while `--muted` here is lighter than white in light mode, where a
							tint of it would vanish. Entries carry a slightly stronger tint
							than their own legs, so the ladder stays entry > its legs in both
							themes, with the current entry above all of it.
						-->
						<!-- Uncommitted entries read green, the way a diff marks additions. -->
						<tr
							class="border-border border-t transition-colors duration-[var(--motion-duration-fast)]
								ease-[var(--motion-ease)] {tx.modified
								? 'bg-emerald-500/10'
								: current === tx.id
									? 'bg-primary/15'
									: index % 2 === 0
										? 'bg-foreground/8 hover:bg-foreground/12'
										: 'bg-foreground/4 hover:bg-foreground/8'}
								{editing === tx.id ? 'ring-primary/40 ring-1 ring-inset' : ''}"
							onclick={() => select(tx)}
							ondblclick={() => startEdit(tx)}
							data-entry-id={tx.id}
							onkeydown={(event) => {
								if (event.key === "Enter") {
									event.preventDefault();
									saveEdit(tx);
								} else if (event.key === "Escape") {
									event.preventDefault();
									cancelEdit();
								}
							}}
							onfocusout={(event) => commitOnLeave(event, tx)}
						>
							<td
								class="py-1.5 {tx.modified
									? 'border-s-2 border-s-emerald-500/70'
									: current === tx.id
										? 'border-s-2 border-s-primary/70'
										: ''}"
							>
								<!--
									GnuCash's expander, pointing down once the splits
									show. In the basic ledger nothing of the entry is
									listed yet, so any entry with more than one posting
									opens; in the auto-split ledger only entries with
									legs outside this account can.
								-->
								<span class="inline-flex items-center gap-0.5">
									{#if mode.expandWhen !== "none" &&
										(mode.expandWhen === "any-entry" ? tx.postings.length > 1 : hiddenCount(tx) > 0)}
										<Button
											size="icon-xs"
											variant="ghost"
											aria-label={current === tx.id ? "Hide splits" : "Show splits"}
											aria-expanded={current === tx.id}
											onclick={(event) => {
												event.stopPropagation();
												select(tx);
											}}
										>
											<ChevronRight
												class="transition-transform duration-[var(--motion-duration-fast)] ease-[var(--motion-ease)]
													{current === tx.id ? 'rotate-90' : ''}"
											/>
										</Button>
									{/if}
									<Button
										size="icon-xs"
										variant="ghost"
										aria-label={`Edit ${tx.narration}`}
										aria-expanded={editing === tx.id}
										onclick={(event) => {
											event.stopPropagation();
											if (editing === tx.id) cancelEdit();
											else startEdit(tx);
										}}
									>
										<Pencil />
									</Button>
								</span>
							</td>
							<td
								class="border-border/40 border-l py-1.5 whitespace-nowrap tabular-nums text-muted-foreground {editing ===
								tx.id
									? 'px-1'
									: 'pr-3 pl-2'}"
							>
								{#if editing === tx.id && draft}
									<input
										class="bg-background/70 focus:ring-ring w-full rounded-sm px-1 py-0.5 text-xs tabular-nums outline-none focus:ring-1"
										placeholder="YYYY-MM-DD"
										bind:value={draft.date}
										aria-label="Date"
									/>
								{:else}
									{tx.date}
								{/if}
							</td>
							<td class="border-border/40 border-l truncate py-1.5 pr-3 pl-2 font-medium">
								{#if editing === tx.id && draft}
									<span class="flex min-w-0 items-center gap-1">
										<input
											class="bg-background/70 focus:ring-ring min-w-0 flex-1 rounded-sm px-1 py-0.5 text-xs font-medium outline-none focus:ring-1"
											placeholder="Payee"
											bind:value={draft.payee}
											aria-label="Payee"
										/>
										<span class="text-muted-foreground">—</span>
										<input
											class="bg-background/70 focus:ring-ring min-w-0 flex-[2] rounded-sm px-1 py-0.5 text-xs font-medium outline-none focus:ring-1"
											placeholder="Narration"
											bind:value={draft.narration}
											aria-label="Narration"
										/>
									</span>
								{:else if tx.payee && tx.payee !== tx.narration}
									<span class="font-medium">{tx.payee}</span>
									<span class="text-muted-foreground">— {tx.narration}</span>
								{:else}
									{tx.narration}
								{/if}
							</td>
							<!--
								The basic ledger names the other side; the split styles
								instead say how many legs are still hidden, so an entry that
								can be opened is obvious without hunting for the chevron.
							-->
							<td class="border-border/40 border-l truncate py-1.5 pr-3 pl-2 text-muted-foreground">
								{#if mode.namesOtherSide}
									{transferLabel(tx)}
								{:else if !mode.showAllPostings && hiddenCount(tx) > 0 && current !== tx.id}
									<span
										class="rounded bg-foreground/10 px-1.5 py-0.5 text-[0.7rem] font-medium text-foreground/75"
									>
										{hiddenCount(tx)} more{hiddenCount(tx) === 1 ? "" : "s"}
									</span>
								{/if}
							</td>
							<td class="border-border/40 border-l overflow-hidden px-2 py-1.5 text-right font-medium">
								{@render amounts(flow.deposits, "in")}
							</td>
							<td class="border-border/40 border-l overflow-hidden px-2 py-1.5 pr-3 text-right font-medium">
								{@render amounts(flow.withdrawals, "out")}
							</td>
						</tr>
						{#if editing === tx.id && editError}
							<tr>
								<td
									colspan={COLUMNS.length}
									class="py-1 pr-3 pl-2 text-xs text-red-600 dark:text-red-400"
								>
									{editError}
								</td>
							</tr>
						{/if}
						<!--
							One holder row per entry, always mounted: its block grows
							from zero to the legs' own height (and back), so opening
							an entry expands those legs in place instead of the whole
							register being clipped and revealed.
						-->
						<tr
							data-entry-id={tx.id}
							onkeydown={(event) => {
								if (event.key === "Enter") {
									event.preventDefault();
									saveEdit(tx);
								} else if (event.key === "Escape") {
									event.preventDefault();
									cancelEdit();
								}
							}}
							onfocusout={(event) => commitOnLeave(event, tx)}
						>
							<td colspan={COLUMNS.length} class="p-0">
								<div use:smoothHeight class="overflow-hidden">
									<table class="w-full table-fixed">
										<colgroup>
											{#each COLUMNS as width, index (index)}
												<col style="width: {width}" />
											{/each}
										</colgroup>
										<tbody>
											{#each legs as leg, position (leg.index)}
												{@render postingRow(
													leg.posting,
													isSubAccountOf(leg.posting.account, account),
													position,
													leg.index,
													editing === tx.id ? draft : null
												)}
											{/each}
										</tbody>
									</table>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/key}
	</div>
</div>
