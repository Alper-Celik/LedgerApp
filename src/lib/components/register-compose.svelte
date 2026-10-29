<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands } from "../../bindings";
	import { Alert, AlertDescription } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import { ledgerEpoch, ledgerRevision } from "$lib/ledger-revision.svelte.js";

	/**
	 * Composing an entry the ledger does not hold yet. The register's in-place
	 * editor rewrites an entry that is in the file; this one has no row to
	 * become, so it is a form above the rows — one form for both shapes, since a
	 * phone's cards have no columns to put a draft in.
	 *
	 * Two legs and one amount field: the picked account carries the amount, and
	 * the other side — the account the register shows — is left to the loader
	 * to balance. A register over the whole ledger (`account === ""`, as the file
	 * view opens it) has no account of its own to imply, so it picks both.
	 *
	 * `compose` spells the draft out as fields, the way the in-place editor's
	 * `draft` does, so neither editor depends on a directive's shape.
	 */
	let {
		composing = $bindable(false),
		account,
		defaultFile = null,
		onOpen,
		onSaved,
	}: {
		/**
		 * Whether the form is showing. The register's own button owns the click
		 * that flips it, so it holds a copy for `aria-expanded` while the form
		 * holds the state.
		 */
		composing?: boolean;
		/**
		 * The account the register shows: the leg it implies as the other side,
		 * and — when it has none — why the form asks for a second account.
		 */
		account: string;
		/**
		 * The file the register already knows the entry belongs in — a file's own
		 * view. It overrides what the backend deduces from the accounts the draft
		 * touches, since a file's register is already looking at the right file.
		 */
		defaultFile?: string | null;
		/** Closes whatever the register was editing: one entry is written at a time. */
		onOpen?: () => void;
		/**
		 * What the register does after a successful write: stage the revision and
		 * bring itself back in step with the file.
		 */
		onSaved: () => void | Promise<void>;
	} = $props();

	/**
	 * Anything with an id-bound draft is dropped when the ledger reloads.
	 *
	 * A reload renumbers transactions: ids are positions in the booked stream,
	 * so inserting one entry shifts every later id and opening another journal
	 * replaces them wholesale. A draft kept across either would save onto an
	 * unrelated transaction — silently, since the id still exists. The two
	 * signals differ only in who caused the reload: `ledgerEpoch` for a different
	 * journal, `ledgerRevision` for any write (an assistant's proposal, a
	 * commit, an undo). Both close the draft; the user's own save already closed
	 * it before bumping.
	 */
	$effect(() => {
		void ledgerEpoch.value;
		void ledgerRevision.value;
		cancelCompose();
	});

	let saving = $state(false);
	/** The backend's or the loader's refusal, shown under the form. */
	let composeError = $state("");
	let compose = $state({ date: "", payee: "", narration: "", account: "", other: "", units: "" });

	/** The accounts the picker offers — every account the ledger holds. */
	let known = $state<string[]>([]);
	/** The file the backend deduced from the draft's accounts, when it found one. */
	let suggested = $state<string | null>(null);

	/** The leg left to the loader: this register's account, or the second pick. */
	let counter = $derived(account !== "" ? account : compose.other.trim());

	/** The accounts the entry touches, which is what the deduction reads. */
	let touched = $derived([
		...new Set([compose.account.trim(), counter].filter((name) => name !== "")),
	]);

	/**
	 * The file the entry is written into: the view's own (a file's register already
	 * knows it), else the deduced one.
	 */
	let entryFile = $derived(defaultFile || suggested || "");

	/** Its name alone — a path's last segment — since the path itself is the picker's. */
	let entryFileName = $derived(entryFile.split(/[\\/]/).pop() || entryFile);

	/** The narration field, focused as the form opens: it is the one with no default. */
	let narrationInput = $state<HTMLElement | null>(null);

	/** Today, the form the register's own editor spells a date: local, not UTC. */
	function today() {
		const now = new Date();
		const pad = (value: number) => String(value).padStart(2, "0");
		return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
	}

	/** An IPC rejection arrives as a string, a thrown `Error` as itself. */
	const messageOf = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

	/**
	 * Ask the backend which file the entry belongs in, and ask again whenever the
	 * accounts change: they are what say which neighbours the entry has. One ask
	 * per pause in the typing, and the cleanup drops the ask it replaced, so a
	 * slower earlier answer cannot land over a newer one.
	 */
	$effect(() => {
		if (!composing) return;
		const accounts = touched;
		let live = true;
		const timer = setTimeout(async () => {
			try {
				const path = await commands.suggestTransactionFile(accounts);
				if (live) suggested = path;
			} catch {
				// A backend that cannot answer leaves the file to the view's own
				// choice; submitting reports the refusal if there is nowhere to write.
				if (live) suggested = null;
			}
		}, 150);
		return () => {
			live = false;
			clearTimeout(timer);
		};
	});

	/** Focus the narration as the form appears, so typing can start straight away. */
	$effect(() => {
		if (composing) narrationInput?.focus();
	});

	/** The register's button: open the form, or close it again. */
	export function toggle() {
		if (composing) cancelCompose();
		else startCompose();
	}

	/**
	 * Open the form: today, cleared, and with no account picked — the picker is
	 * the *other* side of an entry this register is one leg of, so defaulting it
	 * to this account would post an entry to itself. The account names are
	 * fetched per draft, since an edit elsewhere may have opened one. The
	 * register closes its own two editors first, through `onOpen`: one entry is
	 * written at a time.
	 */
	function startCompose() {
		onOpen?.();
		compose.date = today();
		compose.payee = "";
		compose.narration = "";
		compose.account = "";
		compose.other = "";
		compose.units = "";
		suggested = null;
		composeError = "";
		saving = false;
		composing = true;
		void loadAccounts();
	}

	async function loadAccounts() {
		try {
			known = (await commands.accounts()).map((balance) => balance.name);
		} catch (cause) {
			// No ledger loaded, or an IPC refusal: the form stands and says why,
			// rather than opening onto a picker with nothing in it.
			composeError = messageOf(cause);
		}
	}

	function cancelCompose() {
		composing = false;
		saving = false;
		composeError = "";
	}

	/**
	 * Write the draft through the same pipeline as every other edit: the backend
	 * refuses an entry the loader will not take, puts the file back byte-for-byte
	 * and its complaint is shown under the form; a success stages the file and
	 * brings the register back in step.
	 *
	 * A blank amount is sent as no units at all — the loader interpolates it,
	 * which is what the branch's composer did with an untouched row — and no
	 * commodity at all, so the backend writes the ledger's operating currency.
	 */
	async function saveCompose() {
		if (saving || !composing) return;
		const date = compose.date.trim();
		const posted = compose.account.trim();
		const balancing = counter;
		if (date === "") {
			composeError = "The entry needs a date.";
			return;
		}
		if (posted === "") {
			composeError = "Pick the account the amount is posted to.";
			return;
		}
		if (balancing === "") {
			composeError = "Pick the account that carries the other side.";
			return;
		}
		if (entryFile === "") {
			composeError = "No journal file is known for these accounts, so there is nowhere to write.";
			return;
		}
		const units = compose.units.trim();
		// The extra `currency` is what the branch sent; a backend that takes no
		// commodity ignores it, and one that does writes the operating currency.
		const postings = [
			{ account: posted, units: units === "" ? null : units, currency: null as string | null },
			{ account: balancing, units: null as string | null, currency: null as string | null },
		];
		saving = true;
		composeError = "";
		try {
			const result = await commands.addTransaction({
				file: entryFile,
				date,
				flag: "*",
				payee: compose.payee.trim() === "" ? null : compose.payee.trim(),
				narration: compose.narration.trim(),
				postings,
			});
			if (result.status === "error") {
				composeError = result.error;
				return;
			}
		} catch (cause) {
			composeError = messageOf(cause);
			return;
		} finally {
			saving = false;
		}
		composing = false;
		await onSaved();
	}
</script>

{#if composing}
	<!--
		The draft. It is not in a file yet, so it has no row of the register to
		be edited in place: the form above the rows *is* the entry, the way the
		in-place editor's fields are the entry they rewrite.
	-->
	<!--
		Escape is handled on the form's own fields rather than the form: a
		`<form>` is not an interactive element, and Enter already submits it
		natively, so the only key that needs catching is the one that cancels.
	-->
	<form
		class="bg-accent/30 flex animate-enter flex-col gap-2 rounded-md border border-border p-2"
		aria-label="New transaction"
		onsubmit={(event) => {
			event.preventDefault();
			void saveCompose();
		}}
		onkeydowncapture={(event) => {
			if (event.key !== "Escape") return;
			event.preventDefault();
			cancelCompose();
		}}
	>
		<div class="flex flex-wrap items-end gap-2">
			<label class="flex w-32 shrink-0 flex-col gap-0.5">
				<span class="text-muted-foreground text-xs">Date</span>
				<Input
					class="h-8 px-1 tabular-nums sm:h-7"
					placeholder="YYYY-MM-DD"
					bind:value={compose.date}
				/>
			</label>
			<label class="flex min-w-0 grow basis-36 flex-col gap-0.5">
				<span class="text-muted-foreground text-xs">Payee</span>
				<Input class="h-8 px-1 sm:h-7" placeholder="Optional" bind:value={compose.payee} />
			</label>
			<label class="flex min-w-0 grow basis-44 flex-col gap-0.5">
				<span class="text-muted-foreground text-xs">Narration</span>
				<Input
					class="h-8 px-1 sm:h-7"
					placeholder="What was it for?"
					bind:value={compose.narration}
					bind:ref={narrationInput}
				/>
			</label>
		</div>
		<div class="flex flex-wrap items-end gap-2">
			<div class="flex min-w-0 grow basis-44 flex-col gap-0.5">
				<span class="text-muted-foreground text-xs">Account</span>
				<Select.Root
					type="single"
					value={compose.account}
					onValueChange={(next) => next && (compose.account = next)}
				>
					<Select.Trigger
						type="button"
						class="data-[size=default]:h-8 w-full sm:data-[size=default]:h-7"
						aria-label="Account"
						title={account === "" ? undefined : `${account} takes the other side`}
					>
						<span
							data-slot="select-value"
							class={compose.account === "" ? "text-muted-foreground" : ""}
						>
							{compose.account || "Choose an account"}
						</span>
					</Select.Trigger>
					<Select.Content>
						{#each known as name (name)}
							<Select.Item value={name} label={name} />
						{/each}
					</Select.Content>
				</Select.Root>
			</div>
			{#if account === ""}
				<!--
					A register over the whole ledger has no account of its own to
					imply as the other side, so the file view picks both.
				-->
				<div class="flex min-w-0 grow basis-44 flex-col gap-0.5">
					<span class="text-muted-foreground text-xs">Balancing account</span>
					<Select.Root
						type="single"
						value={compose.other}
						onValueChange={(next) => next && (compose.other = next)}
					>
						<Select.Trigger
							type="button"
							class="data-[size=default]:h-8 w-full sm:data-[size=default]:h-7"
							aria-label="Balancing account"
						>
							<span
								data-slot="select-value"
								class={compose.other === "" ? "text-muted-foreground" : ""}
							>
								{compose.other || "Choose an account"}
							</span>
						</Select.Trigger>
						<Select.Content>
							{#each known as name (name)}
								<Select.Item value={name} label={name} />
							{/each}
						</Select.Content>
					</Select.Root>
				</div>
			{/if}
			<label class="flex w-32 shrink-0 flex-col gap-0.5">
				<span class="text-muted-foreground text-xs">Amount</span>
				<Input
					class="h-8 px-1 text-right tabular-nums sm:h-7"
					inputmode="decimal"
					placeholder="0.00"
					title="Posted to the account picked here; the other leg is left for the ledger to balance"
					bind:value={compose.units}
				/>
			</label>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<span class="text-muted-foreground min-w-0 grow basis-48 text-xs">
				{#if entryFile === ""}
					{#if touched.length === 0}
						The entry's file is deduced from the accounts it touches.
					{:else}
						No journal file is known for these accounts.
					{/if}
				{:else}
					Writes to
					<span class="text-foreground font-medium" title={entryFile}>{entryFileName}</span>
					{#if !defaultFile && suggested !== null}
						— the file suggested for {touched.join(" and ")}.
					{:else}
						— the file this view shows.
					{/if}
				{/if}
			</span>
			<div class="flex items-center gap-2">
				<Button type="submit" class="h-8 sm:h-7" disabled={saving}>Add transaction</Button>
				<Button variant="ghost" class="h-8 sm:h-7" disabled={saving} onclick={cancelCompose}>
					Cancel
				</Button>
			</div>
		</div>
		{#if composeError}
			<Alert variant="destructive">
				<AlertDescription>{composeError}</AlertDescription>
			</Alert>
		{/if}
	</form>
{/if}
