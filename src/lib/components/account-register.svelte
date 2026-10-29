<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { invalidateAll } from "$app/navigation";
	import {
		Add01Icon,
		BracesIcon,
		ChevronRightIcon,
		PencilEdit01Icon,
	} from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { type PostingView, type TransactionView } from "../../bindings";
	import { Alert, AlertDescription } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Table from "$lib/components/ui/table/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import RegisterCompose from "$lib/components/register-compose.svelte";
	import RegisterEntryEditor, { type EntryDraft } from "$lib/components/register-entry-editor.svelte";
	import SourceEditor from "$lib/components/source-editor.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { IsMobile } from "$lib/hooks/is-mobile.svelte.js";
	import { isSubAccountOf } from "$lib/accounts.js";
	import { EDITOR_SCOPES, editorScope, setEditorScope, type EditorScope } from "$lib/editor-scope.svelte.js";
	import { bumpLedgerRevision, ledgerEpoch, ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import { signOf, sumDecimals, type Amount } from "$lib/money.js";
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

	/**
	 * Phones get a card list instead of the table (its tracks cannot fit a
	 * 360px viewport), so the component renders one shape or the other, never
	 * both: `isMobile.current` decides which.
	 */
	// The register's table needs ~44rem of columns beside the sidebar, so the
	// card list takes over below `lg`, not below `md`: at the default 800×600
	// window the sidebar leaves 544px, and a table there is a clipped table.
	const isMobile = new IsMobile(1024);

	/** The style choice sticks across accounts and reloads. */
	let view = $state<ViewId>(
		VIEWS.find((candidate) => candidate.id === localStorage.getItem(VIEW_KEY))?.id ?? "effect"
	);

	/**
	 * The selected style's row of `VIEWS`, which is what the markup reads — the
	 * reader's own choice, except where the view fixes it (a file's own
	 * register, whose amount columns would have no single account to total).
	 */
	let mode = $derived(
		VIEWS.find((candidate) => candidate.id === (fixedView ?? view)) ?? VIEWS[0]
	);

	/** The register's *current transaction*, as GnuCash highlights it. */
	let current = $state<number | null>(null);
	/**
	 * In-place editing, the way GnuCash's register works: the current entry's
	 * cells *become* fields, which `RegisterEntryEditor` renders and writes.
	 * `draft` is the values they hold — it lives here, not in the editor,
	 * because the leg rows' amount fields bind to it too — and it is opened with
	 * a copy of the entry and dropped by `cancelEdit`.
	 */
	let editing = $state<number | null>(null);
	let draft = $state<EntryDraft | null>(null);
	let editError = $state("");

	/**
	 * The open editor, for the rules the register keeps on its rows: the keys
	 * and the commit-on-leave check are caught by the entry's rows (and, in the
	 * table, by its leg block, which is a row of its own), not by the fields the
	 * editor renders. Null whenever no entry is open.
	 */
	let editor = $state<{
		save: () => Promise<void>;
		keydown: (event: KeyboardEvent) => void;
	} | null>(null);

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
		if (owner?.getAttribute("data-entry-id") !== String(tx.id)) void editor?.save();
	}

	/**
	 * The keys an entry's rows answer: Enter saves the entry, Escape drops the
	 * draft. The rows catch them as they bubble from whichever cell or leg field
	 * has focus, and hand them to the open editor; with no entry open a row
	 * still swallows both, since its cells are read-only and Enter would
	 * otherwise activate whichever action button held focus.
	 */
	function keys(event: KeyboardEvent, tx: TransactionView) {
		if (editing === tx.id && editor) {
			editor.keydown(event);
		} else if (event.key === "Enter" || event.key === "Escape") {
			event.preventDefault();
		}
	}

	/**
	 * What a successful in-place write does, in the order it has always done it:
	 * close the draft before the reload renumbers the entries it was opened on,
	 * stage the revision, and bring the register back in step with the file.
	 */
	async function afterEditSaved() {
		cancelEdit();
		bumpLedgerRevision();
		await invalidateAll();
	}
	/**
	 * The entry whose legs are still mounted while its fold closes. Collapsing
	 * removes the legs from `current` in the same tick the grid track starts
	 * going to 0fr, so without this the content is already gone and the row
	 * snaps shut instead of animating. One entry is enough — opening another
	 * closes only this one — and the fold's own `transitionend` releases it.
	 */
	let closing = $state<number | null>(null);

	function select(tx: TransactionView) {
		if (current !== null) closing = current;
		current = current === tx.id ? null : tx.id;
	}

	$effect(() => {
		localStorage.setItem(VIEW_KEY, view);
	});

	let {
		account,
		transactions,
		fixedView = null,
		newEntryFile = null,
		expandAll = false,
	}: {
		account: string;
		transactions: TransactionView[];
		/** The style to render when it is not the reader's to choose — a file's own view. */
		fixedView?: ViewId | null;
		/**
		 * The file a new entry goes into when the view is one file's. It overrides
		 * what the backend deduces from the accounts the draft touches, since a
		 * file's register is already looking at the file the entry belongs in.
		 */
		newEntryFile?: string | null;
		/**
		 * Whether every entry shows all of its legs without being opened. A
		 * register hides them behind the current entry because an account is
		 * asked about one entry at a time; a file is read, so its legs are its
		 * content.
		 */
		expandAll?: boolean;
	} = $props();

	/**
	 * The new-entry form is its own component (`register-compose.svelte`). The
	 * register keeps the flag the toolbar's button reflects, and a handle to ask
	 * the form to open or close — the button is here, the form's state is there.
	 */
	let composing = $state(false);
	let composer = $state<{ toggle: () => void } | null>(null);

	/**
	 * Anything with an id-bound draft is dropped when the ledger reloads.
	 *
	 * A reload renumbers transactions: ids are positions in the booked stream,
	 * so inserting one entry shifts every later id and opening another journal
	 * replaces them wholesale. A draft kept across either would save onto an
	 * unrelated transaction — silently, since the id still exists. The two
	 * signals differ only in who caused the reload: `ledgerEpoch` for a different
	 * journal, `ledgerRevision` for any write (an assistant's proposal, a
	 * commit, an undo). Both close what this component was holding; the user's
	 * own save already closed it before bumping.
	 */
	$effect(() => {
		void ledgerEpoch.value;
		void ledgerRevision.value;
		cancelEdit();
		sourceEditing = null;
		current = null;
		closing = null;
	});

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
		const deposits: Amount[] = [];
		const withdrawals: Amount[] = [];
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

	/**
	 * The postings an entry lists once it is open, in the order they appear in
	 * the file: the style decides whether the entry shows any legs while closed,
	 * and whether all of them show or only the ones hitting this account.
	 *
	 * Both shapes — the table and the phone's card list — read this, so opening
	 * an entry cannot mean different things to either. `expandAll` (a file's
	 * register) is every entry open at once: its legs are the file's content.
	 */
	function legsOf(tx: TransactionView, shown: boolean) {
		const open = shown || expandAll;
		if (mode.legsWhenCurrentOnly && !open) return [];
		return tx.postings
			.map((posting, index) => ({ posting, index }))
			.filter(
				(leg) =>
					mode.showAllPostings || open || isSubAccountOf(leg.posting.account, account)
			);
	}
</script>

{#snippet amounts(values: Amount[])}
	<span class="inline-flex flex-wrap justify-end gap-x-2">
		{#each values as value, index (index)}
			<MoneyAmount units={value.units} currency={value.currency} signed={false} />
		{/each}
	</span>
{/snippet}

{#snippet signedAmounts(values: Amount[])}
	<span class="inline-flex flex-wrap justify-end gap-x-2">
		{#each values as value, index (index)}
			<MoneyAmount units={value.units} currency={value.currency} />
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
				value={editorScope.value}
				onValueChange={(next) => next && setEditorScope(next as EditorScope)}
				aria-label="Source editor scope"
			>
				{#each EDITOR_SCOPES as option (option.id)}
					<ToggleGroup.Item value={option.id} title={option.hint}>{option.label}</ToggleGroup.Item>
				{/each}
			</ToggleGroup.Root>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<!--
				The one thing a register adds: an entry it does not hold yet. The
				form it opens sits above the rows (see below), not in a row of
				theirs, since the entry is in no file yet.
			-->
			<Button
				size="default"
				class="h-8 sm:h-7"
				variant="outline"
				aria-expanded={composing}
				onclick={() => composer?.toggle()}
			>
				<HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
				New transaction
			</Button>
			<!--
				The style is the reader's choice on a desktop register, except where
				the view fixes it (a file's own). A phone's cards have no columns to
				switch between — one signed amount and its legs is all a card shows —
				so the switch is not rendered there at all.
			-->
			{#if fixedView === null && !isMobile.current}
				<ToggleGroup.Root
					type="single"
					value={view}
					onValueChange={(next) => {
						if (!next) return;
						view = next as ViewId;
						// The table remounts with the style; nothing is left to close.
						closing = null;
					}}
					aria-label="Register style"
				>
					{#each VIEWS as option (option.id)}
						<ToggleGroup.Item value={option.id}>{option.label}</ToggleGroup.Item>
					{/each}
				</ToggleGroup.Root>
			{/if}
		</div>
	</div>

	<!--
		The new-entry form: an entry the ledger does not hold yet, so it has no row
		of the register to be edited in place. Always mounted — the toolbar's
		button opens it through the handle, and the form holds its own state.
	-->
	<RegisterCompose
		bind:this={composer}
		bind:composing
		{account}
		defaultFile={newEntryFile}
		onOpen={() => {
			cancelEdit();
			sourceEditing = null;
		}}
		onSaved={async () => {
			bumpLedgerRevision();
			await invalidateAll();
		}}
	/>

	{#key view}
		{#if isMobile.current}
			<!--
				Phones get a card per entry instead of the table: four tracks inside
				a 360px viewport leave the date and the description too narrow to
				read. The card keeps the register's three parts stacked — when,
				what, how much — with the account's effect on the entry as one
				*signed* amount, since a card has no deposit/withdrawal column to
				carry the direction. The legs open under the card the same way the
				table's rows do.
			-->
			<ul class="divide-border divide-y overflow-hidden rounded-md border animate-enter">
				{#each transactions as tx (tx.id)}
					{@const flow = flows(tx)}
					{@const shown = current === tx.id || closing === tx.id}
					{@const legs = legsOf(tx, shown)}
					{@const hidden = hiddenCount(tx)}
					{@const expandable =
						mode.expandWhen !== "none" &&
						(mode.expandWhen === "any-entry" ? tx.postings.length > 1 : hidden > 0)}
					<li
						class={cn(
							"flex flex-col transition-motion",
							tx.modified ? "bg-added/10" : current === tx.id && "bg-accent"
						)}
						data-entry-id={tx.id}
						onfocusout={(event) => commitOnLeave(event, tx)}
					>
						{#if editing === tx.id && draft}
							<!--
								The card's head *becomes* the fields while the entry is
								edited: the editor stacks them, because a phone has no
								room for a row of them.
							-->
							<RegisterEntryEditor
								bind:this={editor}
								variant="card"
								{tx}
								bind:draft
								onError={(message) => (editError = message)}
								onSaved={afterEditSaved}
								onCancel={cancelEdit}
							/>
						{:else}
							<button
								type="button"
								class={cn(
									"flex w-full flex-col gap-1 border-s-2 p-3 text-left",
									tx.modified
										? "border-s-added"
										: current === tx.id
											? "border-s-primary"
											: "border-s-transparent"
								)}
								aria-expanded={expandable ? current === tx.id : undefined}
								onclick={() => select(tx)}
							>
								<span class="flex items-baseline gap-2">
									<span class="text-muted-foreground shrink-0 text-xs tabular-nums">
										{tx.date}
									</span>
									<span class="ml-auto min-w-0 text-right font-medium">
										{@render signedAmounts([...flow.deposits, ...flow.withdrawals])}
									</span>
								</span>
								<span class="flex min-w-0 items-center gap-1.5">
									<UncommittedMark modified={tx.modified} class="shrink-0" />
									<span class="min-w-0 truncate font-medium">
										{#if tx.payee && tx.payee !== tx.narration}
											{tx.payee}
											<span class="text-muted-foreground font-normal">— {tx.narration}</span>
										{:else}
											{tx.narration}
										{/if}
									</span>
								</span>
							</button>
						{/if}
						<!--
							The basic ledger names the other side here, the split styles
							say how many legs are still hidden, and the entry's actions
							sit at the thumb end of the same row.
						-->
						<div class="flex items-center gap-1 border-s-2 border-s-transparent px-3 pb-2">
							<span class="text-muted-foreground min-w-0 flex-1 truncate text-xs">
								{#if mode.namesOtherSide}
									{transferLabel(tx)}
								{:else if !mode.showAllPostings && hidden > 0 && current !== tx.id}
									{hidden} more
								{/if}
							</span>
							{#if expandable}
								<Button
									size="icon"
									class="size-8"
									variant="ghost"
									aria-label={current === tx.id ? "Hide splits" : "Show splits"}
									aria-expanded={current === tx.id}
									onclick={() => select(tx)}
								>
									<HugeiconsIcon
										icon={ChevronRightIcon}
										strokeWidth={2}
										class={cn("transition-motion-fast", current === tx.id && "rotate-90")}
									/>
								</Button>
							{/if}
							<Button
								size="icon"
								class="size-8"
								variant="ghost"
								aria-label={editing === tx.id ? `Stop editing ${tx.narration}` : `Edit ${tx.narration}`}
								aria-expanded={editing === tx.id}
								onclick={() => (editing === tx.id ? cancelEdit() : startEdit(tx))}
							>
								<HugeiconsIcon icon={PencilEdit01Icon} strokeWidth={2} />
							</Button>
							<Button
								size="icon"
								class="size-8"
								variant="ghost"
								aria-label={`Edit the source of ${tx.narration}`}
								aria-expanded={sourceEditing === tx.id}
								onclick={() => toggleSourceEdit(tx)}
							>
								<HugeiconsIcon icon={BracesIcon} strokeWidth={2} />
							</Button>
						</div>
						{#if editing === tx.id && editError}
							<div class="px-3 pb-3">
								<Alert variant="destructive">
									<AlertDescription>{editError}</AlertDescription>
								</Alert>
							</div>
						{/if}
						{#if sourceEditing === tx.id}
							<div class="border-t border-border">
								{#key editorScope.value}
									<SourceEditor
										id={tx.id}
										scope={editorScope.value}
										onSaved={() => {
											sourceEditing = null;
											invalidateAll();
										}}
										onCancel={() => (sourceEditing = null)}
									/>
								{/key}
							</div>
						{/if}
						<!-- One holder per entry, always mounted, so the legs fold in place. -->
						<div
							class={cn("fold", (expandAll || current === tx.id) && "fold-open")}
							ontransitionend={() => {
								if (closing === tx.id) closing = null;
							}}
						>
							<div class="overflow-hidden">
								<div class="flex flex-col border-t border-border px-3 py-1">
									{#each legs as leg (leg.index)}
										{@const affecting = isSubAccountOf(leg.posting.account, account)}
										<div class="flex items-center gap-2 py-1">
											<span
												class={cn(
													"min-w-0 flex-1 truncate text-xs",
													affecting ? "font-medium text-foreground" : "text-muted-foreground"
												)}
											>
												{leg.posting.account}
											</span>
											{#if editing === tx.id && draft}
												<Input
													class="h-7 w-28 shrink-0 text-right tabular-nums"
													bind:value={draft.units[leg.index]}
													onkeydown={(event) => keys(event, tx)}
													aria-label={`Amount on ${leg.posting.account}`}
												/>
											{:else}
												<MoneyAmount
													class="shrink-0 text-xs"
													units={leg.posting.units}
													currency={leg.posting.currency}
												/>
											{/if}
										</div>
									{/each}
								</div>
							</div>
						</div>
					</li>
				{/each}
			</ul>
		{:else}
			<!--
				Fixed layout: widths come from the colgroup, never from content, so a
				long description or an opened entry cannot shift the amount columns.
			-->
			<Table.Root class="table-fixed animate-enter min-w-176">
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
						<Table.Head class="h-7 border-s border-border pl-2">
							{mode.transferColumn}
						</Table.Head>
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
						<!--
							`shown` is `current` plus the row that is closing: its legs have
							to survive one more transition for the fold to have anything
							to shrink. Everything that *looks* like the current entry
							below still reads `current` alone.
						-->
						{@const shown = current === tx.id || closing === tx.id}
						{@const legs = legsOf(tx, shown)}
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
							onkeydown={(event) => keys(event, tx)}
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
								<!--
									The row's three actions: 20px icons, the density this
									table's rows are built for. Phones have their own
									buttons on the card (see the list above).
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
							{#if editing === tx.id && draft}
								<!--
									The row's two value cells *become* fields: the editor
									renders them and the register keeps the rest of the
									row — the actions, the transfer and the amounts.
								-->
								<RegisterEntryEditor
									bind:this={editor}
									variant="table"
									{tx}
									bind:draft
									onError={(message) => (editError = message)}
									onSaved={afterEditSaved}
									onCancel={cancelEdit}
								/>
							{:else}
								<Table.Cell
									class="border-s border-border py-1.5 pr-3 pl-2 tabular-nums text-muted-foreground"
								>
									{tx.date}
								</Table.Cell>
								<Table.Cell class="border-s border-border truncate py-1.5 pr-3 pl-2 font-medium">
									<!--
										The uncommitted state, said in the sidebar's own
										words: one dot in the diff's green, named in the
										tooltip and for assistive tech so it is not
										silent. It rides in the description cell — the
										column that absorbs the slack — because the
										actions column is already full: an expander, a
										pencil and the braces fill it to the pixel, and a
										fourth child would spill over its border.
									-->
									<UncommittedMark modified={tx.modified} class="me-1.5 align-middle" />
									{#if tx.payee && tx.payee !== tx.narration}
										<span class="font-medium">{tx.payee}</span>
										<span class="text-muted-foreground">— {tx.narration}</span>
									{:else}
										{tx.narration}
									{/if}
								</Table.Cell>
							{/if}
							<!--
								The basic ledger names the other side; the split styles
								instead say how many legs are still hidden, so an entry that
								can be opened is obvious without hunting for the chevron.
							-->
							<Table.Cell
								class="border-s border-border truncate py-1.5 pr-3 pl-2 text-muted-foreground"
							>
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
									{#key editorScope.value}
										<SourceEditor
											id={tx.id}
											scope={editorScope.value}
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
							onkeydown={(event) => keys(event, tx)}
							onfocusout={(event) => commitOnLeave(event, tx)}
						>
							<Table.Cell colspan={COLUMNS.length} class="p-0">
								<div
									class={cn("fold", (expandAll || current === tx.id) && "fold-open")}
									ontransitionend={() => {
										if (closing === tx.id) closing = null;
									}}
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
		{/if}
	{/key}
</div>
