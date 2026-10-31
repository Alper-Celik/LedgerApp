<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { tick } from "svelte";
	import { invalidateAll } from "$app/navigation";
	import { commands, type EnvelopeSetting } from "../../../bindings";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import { rolloverLabel } from "$lib/budget-settings.js";
	import { messageOf } from "$lib/errors.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";

	/**
	 * One envelope's rollover policy, as a menu on its row.
	 *
	 * Like the Budgeted cell, this edits the plugin's configuration and not its
	 * output: a choice writes a dated `rollover` directive, and the grid is
	 * reloaded afterwards. The policy applies from its month on, so the menu
	 * names the month it is being set from; the row control is given the newest
	 * visible month so a change governs the future rather than rewriting the
	 * window's history. A cap needs an amount, so choosing it opens a field
	 * instead of writing immediately.
	 */
	let {
		setting,
		account,
		file,
		month,
		label,
		currency,
		disabled = false,
	}: {
		/** The account's configured policy, or `null` when it has no setting yet. */
		setting: EnvelopeSetting | null;
		/** The ledger account the directive names. */
		account: string;
		/** The journal file the directive is written into. */
		file: string;
		/** `YYYY-MM` the policy is dated from. */
		month: string;
		/** The envelope's account, for the control's accessible name. */
		label: string;
		/** The grid's commodity, for a first cap on an account with no amount yet. */
		currency: string;
		disabled?: boolean;
	} = $props();

	let open = $state(false);
	let capping = $state(false);
	let cap = $state("");
	let saving = $state(false);
	let error = $state("");
	let container = $state<HTMLElement | null>(null);
	let trigger = $state<HTMLButtonElement | null>(null);
	let firstItem = $state<HTMLButtonElement | null>(null);
	let capField = $state<HTMLInputElement | null>(null);

	const current = $derived(rolloverLabel(setting, currency));
	const writable = $derived(!disabled && account !== "" && file !== "");
	const capAmount = $derived(
		setting?.rollover?.kind === "cap" ? (setting.rollover.amount ?? null) : null
	);
	const capCurrency = $derived(capAmount?.currency || currency);

	function show() {
		if (!writable || saving) return;
		error = "";
		capping = false;
		open = true;
		void tick().then(() => firstItem?.focus());
	}

	function hide() {
		if (saving) return;
		open = false;
		capping = false;
		error = "";
	}

	function startCap() {
		cap = capAmount?.units ?? "";
		error = "";
		capping = true;
		void tick().then(() => capField?.focus());
	}

	async function write(kind: "carry" | "reset" | "cap", units: string | null, unitCurrency: string | null) {
		if (saving) return;
		saving = true;
		error = "";
		try {
			const result = await commands.budgetRolloverSet(
				file,
				account,
				kind,
				units,
				unitCurrency,
				month
			);
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
		open = false;
		capping = false;
		bumpLedgerRevision();
		await invalidateAll();
	}

	function saveCap() {
		const value = cap.trim();
		if (value === "") {
			error = "Enter the amount to carry — 0.00 sweeps the whole balance.";
			return;
		}
		void write("cap", value, capCurrency);
	}

	/**
	 * Tabbing out closes the menu. A `relatedTarget` of `null` is not enough to
	 * act on: choosing the cap option removes the focused button, which fires
	 * `focusout` with no target just before the new field is focused. Clicks
	 * outside are caught by the document listener below instead.
	 */
	function onfocusout(event: FocusEvent) {
		if (!open || saving) return;
		const next = event.relatedTarget;
		if (!(next instanceof Node) || container?.contains(next)) return;
		open = false;
		capping = false;
	}

	$effect(() => {
		if (!open) return;
		const outside = (event: PointerEvent) => {
			if (event.target instanceof Node && container?.contains(event.target)) return;
			hide();
		};
		document.addEventListener("pointerdown", outside, true);
		return () => document.removeEventListener("pointerdown", outside, true);
	});

	function onkeydown(event: KeyboardEvent) {
		if (!open || event.key !== "Escape") return;
		event.preventDefault();
		event.stopPropagation();
		if (capping) {
			capping = false;
			error = "";
			void tick().then(() => firstItem?.focus());
		} else {
			hide();
			trigger?.focus();
		}
	}
</script>

<div
	bind:this={container}
	class="relative shrink-0"
	role="toolbar"
	tabindex="-1"
	aria-label={`Rollover policy for ${label}`}
	onfocusout={onfocusout}
	onkeydown={onkeydown}
>
	<button
		bind:this={trigger}
		type="button"
		class="focus-visible:ring-ring text-muted-foreground hover:text-foreground rounded-sm text-[10px] underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:outline-none disabled:opacity-60 disabled:no-underline"
		aria-haspopup="menu"
		aria-expanded={open}
		aria-label={`Rollover policy for ${label}: ${current}`}
		title={writable
			? `Set the rollover policy from ${month} on`
			: "This envelope has no journal file to write its rollover to"}
		disabled={!writable || saving}
		onclick={() => (open ? hide() : show())}
	>
		{current}
	</button>

	{#if open}
		<div
			class="bg-popover text-popover-foreground absolute end-0 top-full z-50 mt-1 w-60 rounded-md border p-2 text-left shadow-md"
			role="menu"
			aria-label={`Rollover policy for ${label}`}
		>
			<span class="text-muted-foreground block text-[10px] leading-tight">
				From {month} on for {label}
			</span>
			{#if capping}
				<form
					class="mt-1 flex flex-col gap-1"
					onsubmit={(event) => {
						event.preventDefault();
						saveCap();
					}}
				>
					<label class="flex flex-col gap-0.5">
						<span class="text-muted-foreground text-[10px]">Carry up to</span>
						<span class="flex items-center gap-1">
							<Input
								bind:ref={capField}
								bind:value={cap}
								class="h-7 min-w-0 flex-1 text-right tabular-nums"
								inputmode="decimal"
								aria-label={`Cap amount from ${month} for ${label}`}
								disabled={saving}
							/>
							<span class="text-muted-foreground shrink-0 text-[10px]">{capCurrency}</span>
						</span>
					</label>
					<p class="text-muted-foreground text-[10px] leading-tight">
						Anything above the cap is swept back to <code class="font-mono">To-Be-Budgeted</code>
						at the month boundary.
					</p>
					<div class="flex justify-end gap-1">
						<Button type="submit" size="sm" class="h-6 px-2" disabled={saving}>
							{saving ? "Saving…" : "Save cap"}
						</Button>
						<Button
							type="button"
							size="sm"
							variant="ghost"
							class="h-6 px-2"
							disabled={saving}
							onclick={() => {
								capping = false;
								error = "";
							}}
						>
							Back
						</Button>
					</div>
				</form>
			{:else}
				<div class="mt-1 flex flex-col">
					<button
						bind:this={firstItem}
						type="button"
						role="menuitem"
						class="hover:bg-accent focus-visible:bg-accent rounded-sm px-1.5 py-1 text-left text-[11px] focus-visible:outline-none"
						disabled={saving}
						onclick={() => void write("carry", null, null)}
					>
						Carry the whole balance
					</button>
					<button
						type="button"
						role="menuitem"
						class="hover:bg-accent focus-visible:bg-accent rounded-sm px-1.5 py-1 text-left text-[11px] focus-visible:outline-none"
						disabled={saving}
						onclick={() => void write("reset", null, null)}
					>
						Reset to zero
					</button>
					<button
						type="button"
						role="menuitem"
						class="hover:bg-accent focus-visible:bg-accent rounded-sm px-1.5 py-1 text-left text-[11px] focus-visible:outline-none"
						disabled={saving}
						onclick={startCap}
					>
						Carry up to…
					</button>
				</div>
			{/if}
			{#if error}
				<p class="text-destructive mt-1 text-[11px] leading-tight" role="alert">{error}</p>
			{/if}
		</div>
	{/if}
</div>
