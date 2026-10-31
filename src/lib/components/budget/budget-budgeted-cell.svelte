<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { tick } from "svelte";
	import { invalidateAll } from "$app/navigation";
	import { commands } from "../../../bindings";
	import BudgetAmount from "$lib/components/budget/budget-amount.svelte";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import { monthLabel } from "$lib/budget.js";
	import { messageOf } from "$lib/errors.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";

	/**
	 * One envelope's Budgeted figure, editable in place.
	 *
	 * The displayed number is the plugin's output and is never changed here. A
	 * save writes a dated `fund` directive into the journal — the configuration
	 * the plugin reads on its next run — and then asks the ledger to reload, so
	 * the number that appears afterwards is the plugin's arithmetic, not an
	 * optimistic guess. The directive applies from its month on, which is why
	 * the label names the month rather than implying the figure is only for it.
	 */
	let {
		units,
		currency,
		month,
		account,
		file,
		label,
		disabled = false,
	}: {
		/** The plugin's figure for `month`, as the grid renders it. */
		units: string;
		currency: string;
		/** `YYYY-MM` — the cell's month, and the date the directive carries. */
		month: string;
		/** The ledger account the directive names (`Expenses:Food:Groceries`). */
		account: string;
		/** The journal file the directive is written into. */
		file: string;
		/** The envelope's account, for the control's accessible name. */
		label: string;
		/** Nothing can be written: no file, or the row is not an envelope. */
		disabled?: boolean;
	} = $props();

	let editing = $state(false);
	let draft = $state("");
	let saving = $state(false);
	/** The backend's refusal or the IPC failure, shown in the editor. */
	let error = $state("");
	let panel = $state<HTMLElement | null>(null);
	let field = $state<HTMLInputElement | null>(null);

	const writable = $derived(!disabled && account !== "" && file !== "");

	function open() {
		if (!writable || saving || editing) return;
		draft = units;
		error = "";
		editing = true;
		void tick().then(() => field?.focus());
	}

	function close() {
		if (saving) return;
		editing = false;
		error = "";
	}

	/**
	 * Write the draft as the funding in force from `month` on. A blank draft
	 * never reaches the backend; `0.00` is legal and is how funding stops, so it
	 * is deliberately not treated as empty. An unchanged value closes without a
	 * write, while a refusal keeps the editor and the draft on screen.
	 */
	async function save() {
		if (saving || !editing) return;
		const value = draft.trim();
		if (value === "") {
			error = "Enter an amount — 0.00 stops the funding.";
			return;
		}
		if (value === units) {
			close();
			return;
		}
		saving = true;
		error = "";
		try {
			const result = await commands.budgetFundSet(file, account, value, currency, month);
			if (result.status === "error") {
				error = result.error;
				return;
			}
		} catch (cause) {
			error = messageOf(cause);
			return;
		} finally {
			saving = false;
		}
		editing = false;
		// The plugin's output is re-derived on load; the revision is what makes
		// every surface reading the ledger — this grid included — fetch it again.
		bumpLedgerRevision();
		await invalidateAll();
	}

	/**
	 * Leaving the editor saves only when the value changed; moving focus to a
	 * button inside the panel (Save, Cancel) is not leaving it, so the click's
	 * own action decides instead.
	 */
	function onblur(event: FocusEvent) {
		if (!editing || saving) return;
		const next = event.relatedTarget;
		if (next instanceof Node && panel?.contains(next)) return;
		void save();
	}
</script>

<div class="relative min-w-0 text-right">
	<button
		type="button"
		class="focus-visible:ring-ring w-full rounded-sm text-right underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:outline-none disabled:cursor-default disabled:no-underline disabled:opacity-100"
		aria-label={`Edit budgeted amount for ${label} from ${month}`}
		aria-haspopup="dialog"
		title={writable
			? `Write the funding in force from ${month} on`
			: "This envelope has no journal file to write its funding to"}
		disabled={!writable || saving}
		onclick={open}
	>
		<BudgetAmount {units} {currency} />
	</button>

	{#if editing}
		<!--
			The editor is a panel rather than a cell-shaped field: the hint and a
			possible refusal need room the 32px row does not have, and the cell
			stays a figure while it is open behind it.
		-->
		<div
			bind:this={panel}
			class="bg-popover text-popover-foreground absolute end-0 top-full z-50 mt-1 w-60 rounded-md border p-2 text-left shadow-md"
			role="dialog"
			tabindex="-1"
			aria-label={`Funding from ${month} for ${label}`}
			onkeydown={(event) => {
				if (event.key !== "Escape") return;
				event.preventDefault();
				close();
			}}
		>
			<form
				onsubmit={(event) => {
					event.preventDefault();
					void save();
				}}
			>
				<span class="text-muted-foreground block text-[10px] leading-tight">
					Funding from {month} on ({monthLabel(month)}) for {label}
				</span>
				<div class="mt-1 flex items-center gap-1">
					<Input
						bind:ref={field}
						bind:value={draft}
						class="h-7 min-w-0 flex-1 text-right tabular-nums"
						inputmode="decimal"
						aria-label={`Budgeted from ${month} for ${label}`}
						disabled={saving}
						onblur={onblur}
					/>
					<span class="text-muted-foreground shrink-0 text-[10px]">{currency}</span>
				</div>
				<p class="text-muted-foreground mt-1 text-[10px] leading-tight">
					Applies from {month} on, and later months inherit it until another fund replaces it.
					<code class="font-mono">0.00</code> stops the funding.
				</p>
				{#if error}
					<p class="text-destructive mt-1 text-[11px] leading-tight" role="alert">{error}</p>
				{/if}
				<div class="mt-1 flex justify-end gap-1">
					<Button type="submit" size="sm" class="h-6 px-2" disabled={saving}>
						{saving ? "Saving…" : "Save"}
					</Button>
					<Button
						type="button"
						size="sm"
						variant="ghost"
						class="h-6 px-2"
						disabled={saving}
						onclick={close}
					>
						Cancel
					</Button>
				</div>
			</form>
		</div>
	{/if}
</div>
