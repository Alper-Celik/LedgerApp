<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { invalidateAll } from "$app/navigation";
	import { BracesIcon, ChevronRightIcon, PencilEdit01Icon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { commands, type PostingView, type TransactionView } from "../../bindings";
	import { Alert, AlertDescription } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Table from "$lib/components/ui/table/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import SourceEditor from "$lib/components/source-editor.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { isSubAccountOf } from "$lib/accounts.js";
	import { EDITOR_SCOPES, editorScope, setEditorScope, type EditorScope } from "$lib/editor-scope.svelte.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import { signOf, sumDecimals } from "$lib/money.js";
	import { cn } from "$lib/utils.js";

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
	 * Column widths, named in `app.css` and applied to both the register's own
	 * table and the nested table inside each entry's leg block, so the legs line
	 * up with the columns they belong to. The first fits the entry's actions
	 * side by side — the expander, the pencil, the source editor's braces — so
	 * they cannot spill into the Date cell; the description column is the one
	 * that absorbs the remaining width and truncates.
	 */
	const COLUMNS = [
		"col-actions",
		"col-date",
		"col-description",
		"col-transfer",
		"col-amount",
		"col-amount",
	];

	/** The style choice sticks across accounts and reloads. */
	let view = $state<ViewId>(
		(VIEWS.find((candidate) => candidate.id === localStorage.getItem(VIEW_KEY))?.id ?? "effect") as ViewId
	);
	/**
	 * Both toggles are `single` groups whose pressed segment can be cleared by
	 * clicking it again. Neither may be empty — a register always has a style
	 * and the editor always a scope — so an empty value is ignored and the
	 * stray click is a no-op.
	 */
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

	/**
	 * The other editor: `editing` rewrites an entry's fields in place, this one
	 * shows its whole source text in a Monaco editor under the row. They are
	 * mutually exclusive — opening either closes the other — because both end
	 * by writing the same transaction.
	 */
	let sourceEditing = $state<number | null>(null);

	function startEdit(tx: TransactionView) {
		sourceEditing = null;
		editing = tx.id;
		editError = "";
		draft = {
			date: tx.date,
			payee: tx.payee ?? "",
			narration: tx.narration,
			units: tx.postings.map((posting) => posting.units),
		};
	}

	function toggleSourceEdit(tx: TransactionView) {
		cancelEdit();
		sourceEditing = sourceEditing === tx.id ? null : tx.id;
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

{#snippet amounts(values: { units: string; currency: string }[])}
	<span class="inline-flex flex-wrap justify-end gap-x-2">
		{#each values as value, index (index)}
			<MoneyAmount units={value.units} currency={value.currency} signed={false} />
		{/each}
	</span>
{/snippet}

{#snippet postingRow(
	posting: PostingView,
	affecting: boolean,
	index: number,
	legDraft: { units: string[] } | null
)}
	{@const outward = posting.units.startsWith("-")}
	<Table.Row>
		<Table.Cell></Table.Cell>
		<Table.Cell class="border-s border-border"></Table.Cell>
		<Table.Cell class="border-s border-border"></Table.Cell>
		<!--
			Colour, not opacity, separates the legs that hit this account from the
			ones that do not: an alpha would drop small text under the 4.5:1
			contrast floor.
		-->
		<Table.Cell
			class={cn(
				"border-s-2 border-s-muted-foreground truncate py-1 pr-3 pl-3",
				affecting ? "font-medium text-foreground" : "text-muted-foreground"
			)}
		>
			{posting.account}
		</Table.Cell>
		<Table.Cell class="border-s border-border overflow-hidden px-2 py-1 text-right">
			{#if legDraft && !outward}
				<Input
					class="h-6 px-1 text-right tabular-nums"
					bind:value={legDraft.units[index]}
					aria-label={`Amount on ${posting.account}`}
				/>
			{:else if !outward}
				{@render amounts([{ units: posting.units, currency: posting.currency }])}
			{/if}
		</Table.Cell>
		<Table.Cell class="border-s border-border overflow-hidden px-2 py-1 pr-3 text-right">
			{#if legDraft && outward}
				<Input
					class="h-6 px-1 text-right tabular-nums"
					bind:value={legDraft.units[index]}
					aria-label={`Amount on ${posting.account}`}
				/>
			{:else if outward}
				{@render amounts([{ units: posting.units, currency: posting.currency }])}
			{/if}
		</Table.Cell>
	</Table.Row>
{/snippet}

<div class="flex flex-col gap-3">
	<div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
		<!--
			What the braces button opens: the entry's own lines, or the whole
			`.bean` it lives in, opened on the entry. A setting rather than a
			per-entry choice, so it is chosen once here and followed by every
			entry after it.
		-->
		<div class="flex items-center gap-2">
			<span class="text-muted-foreground text-xs" title="What the source editor opens"> Source editor </span>
			<ToggleGroup.Root
				type="single"
				value={editorScope.scope}
				onValueChange={(next) => next && setEditorScope(next as EditorScope)}
				aria-label="Source editor scope"
			>
				{#each EDITOR_SCOPES as option (option.id)}
					<ToggleGroup.Item value={option.id} title={option.hint}>{option.label}</ToggleGroup.Item>
				{/each}
			</ToggleGroup.Root>
		</div>
		<ToggleGroup.Root
			type="single"
			value={view}
			onValueChange={(next) => next && (view = next as ViewId)}
			aria-label="Register style"
		>
			{#each VIEWS as option (option.id)}
				<ToggleGroup.Item value={option.id}>{option.label}</ToggleGroup.Item>
			{/each}
		</ToggleGroup.Root>
	</div>

	{#key view}
		<!--
			Fixed layout: widths come from the colgroup, never from content, so a
			long description or an opened entry cannot shift the amount columns.
		-->
		<Table.Root class="table-fixed min-w-176 animate-enter">
			<colgroup>
				{#each COLUMNS as column, index (index)}
					<col class={column} />
				{/each}
			</colgroup>
			<Table.Header>
				<Table.Row>
					<Table.Head class="h-7"></Table.Head>
					<Table.Head class="h-7 border-s border-border pl-2">Date</Table.Head>
					<Table.Head class="h-7 border-s border-border pl-2">Description</Table.Head>
					<Table.Head class="h-7 border-s border-border pl-2">{mode.transferColumn}</Table.Head>
					<Table.Head class="h-7 border-s border-border px-2 text-right">
						{mode.amountColumns[0]}
					</Table.Head>
					<Table.Head class="h-7 border-s border-border px-2 pr-3 text-right">
						{mode.amountColumns[1]}
					</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each transactions as tx (tx.id)}
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
						Three states, three semantic tints: this entry is the current
						one (`accent`), or it holds changes not yet committed (the
						diff's green), or it is a plain entry of the register. The
						first cell carries the matching left marker, so an entry's
						state never has to be read from a tint alone.
					-->
					<Table.Row
						class={cn(
							"transition-motion",
							tx.modified ? "bg-added/10" : current === tx.id ? "bg-accent" : "",
							editing === tx.id && "ring-1 ring-inset ring-ring"
						)}
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
						<Table.Cell
							class={cn(
								"px-0.5 py-1",
								tx.modified
									? "border-s-2 border-s-added"
									: current === tx.id && "border-s-2 border-s-primary"
							)}
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
										<HugeiconsIcon
											icon={ChevronRightIcon}
											strokeWidth={2}
											class={cn("transition-motion-fast", current === tx.id && "rotate-90")}
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
									<HugeiconsIcon icon={PencilEdit01Icon} strokeWidth={2} />
								</Button>
								<!--
									The other way into the same transaction: its whole
									source text, edited as text.
								-->
								<Button
									size="icon-xs"
									variant="ghost"
									aria-label={`Edit the source of ${tx.narration}`}
									aria-expanded={sourceEditing === tx.id}
									onclick={(event) => {
										event.stopPropagation();
										toggleSourceEdit(tx);
									}}
								>
									<HugeiconsIcon icon={BracesIcon} strokeWidth={2} />
								</Button>
							</span>
						</Table.Cell>
						<Table.Cell
							class={cn(
								"border-s border-border py-1.5 tabular-nums text-muted-foreground",
								editing === tx.id ? "px-1" : "pr-3 pl-2"
							)}
						>
							{#if editing === tx.id && draft}
								<Input
									class="h-6 px-1 tabular-nums"
									placeholder="YYYY-MM-DD"
									bind:value={draft.date}
									aria-label="Date"
								/>
							{:else}
								{tx.date}
							{/if}
						</Table.Cell>
						<Table.Cell class="border-s border-border truncate py-1.5 pr-3 pl-2 font-medium">
							<!--
								The uncommitted state, said in the sidebar's own
								words: one dot in the diff's green, named for the
								entry so it is not silent. It rides in the
								description cell — the column that absorbs the
								slack — because the actions column is already
								full: an expander, a pencil and the braces fill
								it to the pixel, and a fourth child would spill
								over its border.
							-->
							<UncommittedMark modified={tx.modified} class="me-1.5 align-middle" />
							{#if editing === tx.id && draft}
								<span class="flex min-w-0 items-center gap-1">
									<Input
										class="h-6 min-w-0 flex-1 px-1"
										placeholder="Payee"
										bind:value={draft.payee}
										aria-label="Payee"
									/>
									<span class="text-muted-foreground">—</span>
									<Input
										class="h-6 min-w-0 flex-2 px-1"
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
						</Table.Cell>
						<!--
							The basic ledger names the other side; the split styles
							instead say how many legs are still hidden, so an entry that
							can be opened is obvious without hunting for the chevron.
						-->
						<Table.Cell class="border-s border-border truncate py-1.5 pr-3 pl-2 text-muted-foreground">
							{#if mode.namesOtherSide}
								{transferLabel(tx)}
							{:else if !mode.showAllPostings && hiddenCount(tx) > 0 && current !== tx.id}
								<Badge variant="secondary">
									{hiddenCount(tx)} more{hiddenCount(tx) === 1 ? "" : "s"}
								</Badge>
							{/if}
						</Table.Cell>
						<Table.Cell
							class="border-s border-border overflow-hidden px-2 py-1.5 text-right font-medium"
						>
							{@render amounts(flow.deposits)}
						</Table.Cell>
						<Table.Cell
							class="border-s border-border overflow-hidden px-2 py-1.5 pr-3 text-right font-medium"
						>
							{@render amounts(flow.withdrawals)}
						</Table.Cell>
					</Table.Row>
					{#if editing === tx.id && editError}
						<Table.Row>
							<Table.Cell colspan={COLUMNS.length} class="py-1 pr-3 pl-2">
								<Alert variant="destructive">
									<AlertDescription>{editError}</AlertDescription>
								</Alert>
							</Table.Cell>
						</Table.Row>
					{/if}
					<!--
						The source editor spans every column, the way GnuCash's
						register opens a fold: the text is the entry's own lines,
						not a cell's value.
					-->
					{#if sourceEditing === tx.id}
						<Table.Row>
							<Table.Cell colspan={COLUMNS.length} class="p-0">
								{#key editorScope.scope}
									<SourceEditor
										id={tx.id}
										scope={editorScope.scope}
										onSaved={() => {
											sourceEditing = null;
											invalidateAll();
										}}
										onCancel={() => (sourceEditing = null)}
									/>
								{/key}
							</Table.Cell>
						</Table.Row>
					{/if}
					<!--
						One holder row per entry, always mounted: its block folds
						from zero height to the legs' own height (and back), so
						opening an entry expands those legs in place instead of the
						whole register being clipped and revealed.
					-->
					<Table.Row
						class="border-b-0"
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
						<Table.Cell colspan={COLUMNS.length} class="p-0">
							<div
								class={cn("fold", current === tx.id && "fold-open")}
							>
								<div class="overflow-hidden">
									<Table.Root class="table-fixed">
										<colgroup>
											{#each COLUMNS as column, index (index)}
												<col class={column} />
											{/each}
										</colgroup>
										<Table.Body>
											{#each legs as leg (leg.index)}
												{@render postingRow(
													leg.posting,
													isSubAccountOf(leg.posting.account, account),
													leg.index,
													editing === tx.id ? draft : null
												)}
											{/each}
										</Table.Body>
									</Table.Root>
								</div>
							</div>
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	{/key}
</div>
