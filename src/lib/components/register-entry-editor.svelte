<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script module lang="ts">
	/**
	 * What an in-place edit holds while it is open: the entry's own fields and
	 * one amount string per posting, in the file's order. Strings, not amounts —
	 * an edit is sent back exactly as typed, and the loader is what parses it.
	 */
	export type EntryDraft = {
		date: string;
		payee: string;
		narration: string;
		units: string[];
	};
</script>

<script lang="ts">
	import { commands, type TransactionView } from "../../bindings";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Table from "$lib/components/ui/table/index.js";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { cn } from "$lib/utils.js";

	/**
	 * The register's in-place editor: the entry's own values, which replace a
	 * row's cells while that entry is rewritten.
	 *
	 * It is not a row of its own. The register's table is a grid whose value
	 * cells are the row's, so in the table this renders the two cells a closed
	 * row would have shown — Date and Description — and the register keeps the
	 * rest of the row around them; on a phone it renders the block that replaces
	 * the card's summary button. `variant` says which of the two shapes the same
	 * fields are laid into; the labels and the commands are the same in both.
	 *
	 * `draft` is a `$bindable` prop rather than this component's own state
	 * because the fields are not the only surface bound to it: the register's
	 * leg rows — the nested table under an open entry — bind their amount inputs
	 * to the same draft, and a component cannot hand its state to cells it does
	 * not render. The register keeps the draft's lifetime, one copy per edit;
	 * everything else about it is here: its shape (`EntryDraft`), the write, the
	 * keys, and what a refusal does.
	 */
	let {
		variant,
		tx,
		draft = $bindable(null),
		onError,
		onSaved,
		onCancel,
	}: {
		/** Which register shape the fields are laid into. */
		variant: "table" | "card";
		/**
		 * The entry being rewritten: its id names the transaction, its flag is
		 * kept as the file spells it, and its modified state tints the card.
		 */
		tx: TransactionView;
		/**
		 * The values as the fields show them. Null only before the register opens
		 * one (it gates this component on its own draft).
		 */
		draft: EntryDraft | null;
		/** The backend's refusal, to be shown where the register shows a row's error. */
		onError: (message: string) => void;
		/** What the register does after a successful write, in its own order. */
		onSaved: () => void | Promise<void>;
		/** Drop the draft: Escape, or the card's Cancel button. */
		onCancel: () => void;
	} = $props();

	/**
	 * Write the draft through the command every other edit goes through. The
	 * payee is trimmed only to decide whether it is empty — a payee that is
	 * there is sent as typed — and the amounts go back as the strings the fields
	 * hold, one per posting, addressed by the posting's index in the file.
	 *
	 * A refusal leaves the draft open with the backend's words (the register
	 * shows them under the row); a success hands the register its own sequence:
	 * close the draft with its row, stage the revision, reload.
	 */
	export async function save() {
		if (!draft) return;
		const result = await commands.updateTransaction({
			id: tx.id,
			date: draft.date,
			flag: tx.flag,
			payee: draft.payee.trim() === "" ? null : draft.payee,
			narration: draft.narration,
			postings: draft.units.map((units, index) => ({ index, units })),
		});
		if (result.status === "error") {
			onError(result.error);
			return;
		}
		await onSaved();
	}

	/**
	 * The keys an open entry answers: Enter writes it, Escape drops the draft.
	 *
	 * The register's rows catch them for the table — they have to work whichever
	 * cell or leg field holds focus, not only the two cells rendered here — and
	 * hand them over; the phone's card has no row to catch them (a list item is
	 * not an interactive element), so its fields carry them themselves.
	 */
	export function keydown(event: KeyboardEvent) {
		if (event.key === "Enter") {
			event.preventDefault();
			void save();
		} else if (event.key === "Escape") {
			event.preventDefault();
			onCancel();
		}
	}
</script>

{#if draft}
	{#if variant === "card"}
		<!--
			Fields cannot sit inside the summary button, so the card's head
			*becomes* the fields while the entry is edited — stacked, because a
			phone has no room for a row of them.
		-->
		<div
			class={cn(
				"flex flex-col gap-1.5 border-s-2 p-3",
				tx.modified ? "border-s-added" : "border-s-primary"
			)}
		>
			<Input
				class="h-8 tabular-nums"
				placeholder="YYYY-MM-DD"
				bind:value={draft.date}
				onkeydown={(event) => keydown(event)}
				aria-label="Date"
			/>
			<Input
				class="h-8"
				placeholder="Payee"
				bind:value={draft.payee}
				onkeydown={(event) => keydown(event)}
				aria-label="Payee"
			/>
			<Input
				class="h-8"
				placeholder="Narration"
				bind:value={draft.narration}
				onkeydown={(event) => keydown(event)}
				aria-label="Narration"
			/>
			<!--
				Thumb-sized instead of the keyboard's Enter: a phone's done key
				does not always blur the field, and these buttons stay put —
				swapping the footer's own buttons out would blur the one that was
				just tapped and so commit before the first keystroke.
			-->
			<span class="flex items-center gap-2 pt-0.5">
				<Button size="sm" class="h-8 flex-1" onclick={() => void save()}>Save</Button>
				<Button size="sm" class="h-8 flex-1" variant="ghost" onclick={onCancel}>
					Cancel
				</Button>
			</span>
		</div>
	{:else}
		<!-- The row's two value cells, as fields: the grid's remaining cells are the register's. -->
		<Table.Cell class="border-s border-border py-1.5 px-1 tabular-nums text-muted-foreground">
			<Input
				class="h-6 px-1 tabular-nums"
				placeholder="YYYY-MM-DD"
				bind:value={draft.date}
				aria-label="Date"
			/>
		</Table.Cell>
		<Table.Cell class="border-s border-border truncate py-1.5 pr-3 pl-2 font-medium">
			<!-- The uncommitted marker keeps riding in the description cell, now beside the fields. -->
			<UncommittedMark modified={tx.modified} class="me-1.5 align-middle" />
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
		</Table.Cell>
	{/if}
{/if}
