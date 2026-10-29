<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { browser } from "$app/environment";
	import { invalidateAll } from "$app/navigation";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { Add01Icon, Cancel01Icon, Delete01Icon } from "@hugeicons/core-free-icons";
	import { commands } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import { messageOf } from "$lib/errors.js";
	import { ledger } from "$lib/ledger.svelte.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import {
		amountOf,
		appendKey,
		suggestions,
		todayIso,
		type QuickAddSign,
		type QuickAddSuggestions,
	} from "$lib/quick-add.js";
	import { cn } from "$lib/utils.js";

	/**
	 * The phone's way in: a floating button, and a sheet that adds an entry in
	 * two taps — the amount, then Add. Everything else already has a default:
	 * today's date, the last pair of accounts this sheet used, the most used
	 * category when there is no pair yet. The register's own composer is the
	 * desktop's version of this and stays the fuller one; on phones there is no
	 * room for it, hence this.
	 *
	 * Mounted once in the root layout, so navigating between routes never
	 * remounts it — the draft and the remembered pair survive.
	 */

	/** The remembered pair: what this sheet posted last, so tap three repeats it. */
	const LAST_PAIR_KEY = "quick-add:last-pair";

	/** The zeros and ones the keypad is laid out as. */
	const KEYS = ["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", "backspace"];

	let open = $state(false);
	/** Whether any sheet or dialog is showing — this one included. */
	let modalOpen = $state(false);

	/** The amount as typed, never as a number: `appendKey`/`amountOf` own it. */
	let digits = $state("");
	let sign = $state<QuickAddSign>("expense");
	/** Today's date once the sheet opens; empty at rest, so SSR and the client agree. */
	let date = $state("");
	/** The entry's one free-text field; it lands in the narration. */
	let text = $state("");

	let from = $state("");
	let to = $state("");
	let accounts = $state<string[]>([]);
	let guesses = $state<QuickAddSuggestions | null>(null);
	let loading = $state(false);
	let saving = $state(false);
	let error = $state("");
	/** Which account picker is open, so Enter there picks rather than posts. */
	let fromOpen = $state(false);
	let toOpen = $state(false);

	const amount = $derived(amountOf(digits, sign));
	const hidden = $derived(open || modalOpen);

	/** The To picker's order: the categories this ledger uses most, then the rest. */
	const toOptions = $derived.by(() => {
		const first = (guesses?.categories ?? []).filter((name) => accounts.includes(name));
		const picked = new Set(first);
		return [...first, ...accounts.filter((name) => !picked.has(name))];
	});

	/** The pair this sheet posted last, when it is still usable. */
	function rememberedPair(): { from: string; to: string } {
		if (!browser) return { from: "", to: "" };
		try {
			const raw = localStorage.getItem(LAST_PAIR_KEY);
			if (raw === null) return { from: "", to: "" };
			const parsed: unknown = JSON.parse(raw);
			if (typeof parsed !== "object" || parsed === null) return { from: "", to: "" };
			const pair = parsed as { from?: unknown; to?: unknown };
			return {
				from: typeof pair.from === "string" ? pair.from : "",
				to: typeof pair.to === "string" ? pair.to : "",
			};
		} catch {
			// No storage (or junk in it): the ledger's own most recent pair answers.
			return { from: "", to: "" };
		}
	}

	function rememberPair() {
		if (!browser) return;
		try {
			localStorage.setItem(LAST_PAIR_KEY, JSON.stringify({ from, to }));
		} catch {
			// A full or forbidden store costs the shortcut, never the write.
		}
	}

	/**
	 * Open onto a fresh draft and fill it in: the account names for the pickers,
	 * and the presets — the pair remembered here, else the ledger's own most
	 * recent pair, else the fallbacks in `suggestions`.
	 */
	async function openSheet() {
		digits = "";
		sign = "expense";
		date = todayIso();
		text = "";
		from = "";
		to = "";
		error = "";
		saving = false;
		fromOpen = false;
		toOpen = false;
		open = true;
		await load();
	}

	async function load() {
		loading = true;
		error = "";
		try {
			const [balances, transactions] = await Promise.all([
				commands.accounts(),
				commands.accountTransactions(""),
			]);
			accounts = balances.map((balance) => balance.name);
			const derived = suggestions(transactions, balances);
			guesses = derived;
			const known = new Set(accounts);
			const remembered = rememberedPair();
			// A choice made while this was in flight stands: only an empty or
			// vanished account is replaced.
			if (!known.has(from)) {
				from = known.has(remembered.from) ? remembered.from : derived.from;
			}
			if (!known.has(to)) {
				to = known.has(remembered.to) ? remembered.to : derived.to;
			}
			if (accounts.length === 0) {
				// An open journal with no `open` directives yet is not a missing
				// ledger: say which one it is, because the fix is different.
				error = ledger.summary
					? "This journal has no accounts yet — open one in the ledger first."
					: "No ledger is open, so there is nothing to add to.";
			}
		} catch (cause) {
			// No ledger loaded, or an IPC refusal: the sheet says why rather than
			// opening onto two pickers with nothing in them.
			error = messageOf(cause);
		} finally {
			loading = false;
		}
	}

	function close() {
		open = false;
		fromOpen = false;
		toOpen = false;
	}

	function press(key: string) {
		digits = appendKey(digits, key);
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		void add();
	}

	/**
	 * Write the draft through the backend's pipeline: the file is the one it
	 * deduces for the pair, and the entry is the category leg carrying the figure
	 * plus the funding leg left for the loader to interpolate. A refusal keeps
	 * the sheet and the draft, and is shown where the choice was made.
	 */
	async function add() {
		if (saving || fromOpen || toOpen) return;
		if (amount === "") {
			error = "Type the amount first.";
			return;
		}
		if (from === "" || to === "") {
			error = "Pick where the money came from and what it was for.";
			return;
		}
		if (from === to) {
			error = "The two accounts have to differ.";
			return;
		}
		if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) {
			error = "The entry needs a date.";
			return;
		}
		saving = true;
		error = "";
		try {
			const file = await commands.suggestTransactionFile([from, to]);
			if (file === null) {
				error = "No journal file is known for these accounts, so there is nowhere to write.";
				return;
			}
			const result = await commands.addTransaction({
				file,
				date,
				flag: "*",
				payee: null,
				narration: text.trim(),
				postings: [
					{ account: to, units: amount, currency: null },
					{ account: from, units: null, currency: null },
				],
			});
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
		rememberPair();
		close();
		bumpLedgerRevision();
		await invalidateAll();
	}

	$effect(() => {
		if (!browser) return;
		// A sheet or dialog is open when its portal (overlay included) sits on the
		// body: shadcn's sheets and dialogs all portal there, so watching the body
		// itself is enough and costs nothing while a page mutates its own DOM.
		const marks = '[data-slot$="overlay"], [role="dialog"], [role="alertdialog"]';
		const update = () => {
			modalOpen = document.querySelector(marks) !== null;
		};
		update();
		const observer = new MutationObserver(update);
		observer.observe(document.body, { childList: true });
		return () => observer.disconnect();
	});
</script>

{#if !hidden}
	<!--
		Phone only: the desktop register has its own composer and its own button.
		56px, sitting above the home indicator rather than on it.
	-->
	<div class="fixed right-4 bottom-0 z-40 pb-[max(0.75rem,env(safe-area-inset-bottom))] md:hidden">
		<Button
			size="icon"
			class="size-14 rounded-full shadow-lg"
			aria-label="Add transaction"
			onclick={openSheet}
		>
			<HugeiconsIcon icon={Add01Icon} strokeWidth={2} class="size-6" />
		</Button>
	</div>
{/if}

<Sheet.Root
	open={open}
	onOpenChange={(next) => {
		if (next) open = true;
		else close();
	}}
>
	<Sheet.Content side="bottom" class="max-h-[92dvh] p-0" showCloseButton={false}>
		<form
			class="mx-auto flex min-h-0 w-full max-w-md flex-col gap-2 overflow-y-auto p-3 pb-[max(0.75rem,env(safe-area-inset-bottom))]"
			onsubmit={submit}
			onkeydowncapture={(event) => {
				// Enter adds from anywhere in the sheet — including on a keypad key,
				// where the browser would otherwise press that key again. The open
				// account pickers take Enter for themselves.
				if (event.key !== "Enter" || event.shiftKey) return;
				if (fromOpen || toOpen) return;
				event.preventDefault();
				void add();
			}}
		>
			<div class="flex items-center gap-2">
				<Sheet.Title class="flex-1">Add entry</Sheet.Title>
				<Sheet.Description class="sr-only">
					Type the amount, check where it came from and what it was for, then add.
				</Sheet.Description>
				<Button size="icon" variant="ghost" class="size-9" aria-label="Close" onclick={close}>
					<HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
				</Button>
			</div>

			<!-- The amount, and which way it went. -->
			<div class="flex items-center gap-2">
				<span
					class={cn(
						"min-w-0 flex-1 truncate text-3xl font-semibold tabular-nums",
						digits === "" && "text-muted-foreground"
					)}
				>
					{amount === "" ? "0" : amount}
				</span>
				<div class="bg-muted flex shrink-0 gap-0.5 rounded-md p-0.5">
					<Button
						type="button"
						size="sm"
						variant={sign === "expense" ? "secondary" : "ghost"}
						class="h-8 px-3"
						aria-pressed={sign === "expense"}
						onclick={() => (sign = "expense")}
					>
						Expense
					</Button>
					<Button
						type="button"
						size="sm"
						variant={sign === "income" ? "secondary" : "ghost"}
						class="h-8 px-3"
						aria-pressed={sign === "income"}
						onclick={() => (sign = "income")}
					>
						Income
					</Button>
				</div>
			</div>

			<div class="grid grid-cols-3 gap-1">
				{#each KEYS as key (key)}
					<Button
						type="button"
						variant="outline"
						class="h-12 text-lg font-medium"
						aria-label={key === "backspace" ? "Backspace" : key}
						onclick={() => press(key)}
					>
						{#if key === "backspace"}
							<HugeiconsIcon icon={Delete01Icon} strokeWidth={2} class="size-5" />
						{:else}
							{key}
						{/if}
					</Button>
				{/each}
			</div>

			<!-- The two accounts: the same full-list picker the register uses. -->
			<div class="grid grid-cols-2 gap-1">
				<Select.Root
					type="single"
					value={from}
					onValueChange={(next) => next && (from = next)}
					bind:open={fromOpen}
				>
					<Select.Trigger
						type="button"
						class="data-[size=default]:h-10 w-full min-w-0 text-sm"
						aria-label="Account the money came from"
						disabled={loading || accounts.length === 0}
					>
						<span data-slot="select-value" class="min-w-0 truncate">
							<span class="text-muted-foreground mr-1">From</span>{from || "Pick"}
						</span>
					</Select.Trigger>
					<Select.Content class="z-60">
						{#each accounts as account (account)}
							<Select.Item value={account} label={account} class="min-h-9 text-sm" />
						{/each}
					</Select.Content>
				</Select.Root>

				<Select.Root
					type="single"
					value={to}
					onValueChange={(next) => next && (to = next)}
					bind:open={toOpen}
				>
					<Select.Trigger
						type="button"
						class="data-[size=default]:h-10 w-full min-w-0 text-sm"
						aria-label="Account the money was for"
						disabled={loading || accounts.length === 0}
					>
						<span data-slot="select-value" class="min-w-0 truncate">
							<span class="text-muted-foreground mr-1">To</span>{to || "Pick"}
						</span>
					</Select.Trigger>
					<Select.Content class="z-60">
						{#each toOptions as account (account)}
							<Select.Item value={account} label={account} class="min-h-9 text-sm" />
						{/each}
					</Select.Content>
				</Select.Root>
			</div>

			<Input class="h-10" placeholder="Payee or note (optional)" bind:value={text} />

			{#if guesses?.payees.length}
				<div class="-mt-1 flex gap-1 overflow-x-auto pb-1">
					{#each guesses.payees.slice(0, 6) as payee (payee)}
						<button
							type="button"
							class="bg-muted hover:bg-accent h-8 shrink-0 rounded-full px-3 text-xs"
							onclick={() => (text = payee)}
						>
							{payee}
						</button>
					{/each}
				</div>
			{/if}

			<Input type="date" class="h-10" aria-label="Date" bind:value={date} />

			{#if error}
				<Alert variant="destructive" class="py-1 text-xs">{error}</Alert>
			{/if}

			<Button type="submit" class="h-11 w-full" disabled={saving || amount === ""}>
				{saving ? "Adding…" : "Add"}
			</Button>
		</form>
	</Sheet.Content>
</Sheet.Root>
