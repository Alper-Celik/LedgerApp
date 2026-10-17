<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { invalidateAll } from "$app/navigation";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { Add01Icon, Delete02Icon } from "@hugeicons/core-free-icons";
	import { commands, type JournalFile } from "../../bindings";
	import { Alert, AlertDescription } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";

	/**
	 * The new-transaction form: the register's way to write an entry that is not
	 * in the ledger yet.
	 *
	 * An entry belongs to a file — a journal can be a root of `open` directives
	 * that includes a file per year — so the form asks which one, and asks the
	 * backend for the answer first: the default is deduced from the accounts
	 * typed (the file of their newest entry), and a file picked by hand ends the
	 * guessing. Whichever file it is, the entry is appended to it through the
	 * same pipeline as every other edit: the loader decides whether it is legal,
	 * a refusal leaves the file byte-for-byte as it was, and a success stages it
	 * for the next commit.
	 */
	let {
		account = "",
	}: {
		/** The account the register is showing, where there is one: the first posting's account. */
		account?: string;
	} = $props();

	/** One posting row. `id` is what the row is keyed by, so removing one cannot shift another's fields. */
	type Posting = { id: number; account: string; units: string; currency: string };

	/** The flags an entry can carry: cleared, or flagged for review. */
	const FLAGS = [
		{ flag: "*", label: "Cleared" },
		{ flag: "!", label: "Flagged" },
	] as const;

	let open = $state(false);
	let saving = $state(false);
	let error = $state("");

	/** The journal's files, listed afresh on every open. */
	let files = $state<JournalFile[]>([]);
	/** Every account the ledger has, offered as completions in the account fields. */
	let known = $state<string[]>([]);

	/** Hands out the row ids; only ever grows, so a removed row's id is never reused. */
	let rows = 0;
	const posting = (name: string): Posting => ({ id: rows++, account: name, units: "", currency: "" });

	/** Today, in the `YYYY-MM-DD` the ledger writes dates in. */
	const today = () => {
		const now = new Date();
		const pad = (value: number) => String(value).padStart(2, "0");
		return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
	};

	let draft = $state({
		date: today(),
		flag: "*",
		payee: "",
		narration: "",
		postings: [] as Posting[],
	});

	/** A file picked by hand; the deduction keeps its hands off while this is set. */
	let chosen = $state<string | null>(null);
	/** What the backend deduced, and the accounts it deduced it from. */
	let suggestion = $state<{ path: string; from: string[] } | null>(null);

	/** The file the entry will be written into. */
	let selected = $derived(chosen ?? suggestion?.path ?? files[0]?.path ?? "");

	/** The accounts the form mentions, which is what the deduction reads. */
	let mentioned = $derived(
		draft.postings.map((row) => row.account.trim()).filter((name) => name !== "")
	);

	/**
	 * Ask the backend which file this entry belongs in, and ask again whenever the
	 * accounts change: they are what say which neighbours the entry has. One ask
	 * per pause in the typing, and the cleanup drops the ask it replaced, so a
	 * slower earlier answer cannot land over a newer one.
	 */
	$effect(() => {
		if (!open) return;
		const accounts = mentioned;
		let live = true;
		const timer = setTimeout(async () => {
			const path = await commands.suggestTransactionFile(accounts);
			if (live) suggestion = path ? { path, from: accounts } : null;
		}, 150);
		return () => {
			live = false;
			clearTimeout(timer);
		};
	});

	/**
	 * Open the form on a clean entry: today, cleared, and — on an account page —
	 * that account on the first posting. The files and the account names are
	 * fetched per open, since an edit elsewhere may have added either.
	 */
	async function start() {
		draft.date = today();
		draft.flag = "*";
		draft.payee = "";
		draft.narration = "";
		draft.postings = [posting(account), posting("")];
		chosen = null;
		suggestion = null;
		error = "";
		open = true;
		try {
			files = await commands.journalFiles();
			known = (await commands.accounts()).map((balance) => balance.name);
		} catch (cause) {
			// A backend that cannot answer — no ledger loaded, an IPC rejection —
			// leaves the form open with nothing to write to, and says why.
			error = cause instanceof Error ? cause.message : String(cause);
		}
	}

	/**
	 * Write the entry. The rows the user left untouched are dropped rather than
	 * sent as empty postings, and a blank commodity is sent as nothing at all:
	 * the backend writes the ledger's own operating currency for it.
	 */
	async function save() {
		if (saving) return;
		saving = true;
		error = "";
		try {
			const result = await commands.addTransaction({
				file: selected,
				date: draft.date.trim(),
				flag: draft.flag,
				payee: draft.payee.trim() === "" ? null : draft.payee.trim(),
				narration: draft.narration.trim(),
				postings: draft.postings
					.filter((row) => row.account.trim() !== "")
					.map((row) => ({
						account: row.account.trim(),
						units: row.units.trim() === "" ? null : row.units.trim(),
						currency: row.currency.trim() === "" ? null : row.currency.trim(),
					})),
			});
			if (result.status === "error") {
				error = result.error;
				return;
			}
		} catch (cause) {
			// A rejection that is not the command's own error string — a missing
			// command, a payload the webview's IPC refused — arrives as a thrown
			// Error, and has to be shown rather than swallowed.
			error = cause instanceof Error ? cause.message : String(cause);
			return;
		} finally {
			saving = false;
		}
		bumpLedgerRevision();
		await invalidateAll();
		open = false;
	}
</script>

<Button size="sm" variant="outline" onclick={start}>
	<HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
	New transaction
</Button>

<Sheet.Root bind:open>
	<Sheet.Content>
		<Sheet.Header>
			<Sheet.Title>New transaction</Sheet.Title>
			<Sheet.Description>
				Appended to the end of the file you pick, then staged for the next commit.
			</Sheet.Description>
		</Sheet.Header>

		<div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-6 pb-2">
			<div class="flex flex-wrap items-start gap-4">
				<div class="flex flex-col gap-1">
					<span class="text-muted-foreground" id="new-transaction-date">Date</span>
					<Input
						class="w-36"
						type="date"
						bind:value={draft.date}
						aria-labelledby="new-transaction-date"
					/>
				</div>
				<div class="flex flex-col gap-1">
					<span class="text-muted-foreground" id="new-transaction-flag">Flag</span>
					<ToggleGroup.Root
						type="single"
						value={draft.flag}
						onValueChange={(next) => next && (draft.flag = next)}
						aria-labelledby="new-transaction-flag"
					>
						{#each FLAGS as option (option.flag)}
							<ToggleGroup.Item value={option.flag} title={option.label}>
								<!-- The character is what the file will carry; the word is what it means. -->
								<span aria-hidden="true">{option.flag}</span>
								<span class="sr-only">{option.label}</span>
							</ToggleGroup.Item>
						{/each}
					</ToggleGroup.Root>
				</div>
			</div>

			<div class="flex flex-col gap-1">
				<span class="text-muted-foreground" id="new-transaction-payee">Payee</span>
				<Input
					bind:value={draft.payee}
					placeholder="Optional"
					aria-labelledby="new-transaction-payee"
				/>
			</div>

			<div class="flex flex-col gap-1">
				<span class="text-muted-foreground" id="new-transaction-narration">Narration</span>
				<Input
					bind:value={draft.narration}
					placeholder="What the entry is for"
					aria-labelledby="new-transaction-narration"
				/>
			</div>

			<div class="flex flex-col gap-2">
				<div class="flex items-center justify-between gap-2">
					<span class="text-muted-foreground" id="new-transaction-postings">Postings</span>
					<Button size="sm" variant="ghost" onclick={() => draft.postings.push(posting(""))}>
						<HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
						Add posting
					</Button>
				</div>
				<div class="flex flex-col gap-2" aria-labelledby="new-transaction-postings">
					{#each draft.postings as row, index (row.id)}
						<div class="border-border flex flex-col gap-1.5 rounded-md border p-2">
							<div class="flex items-center gap-1.5">
								<Input
									class="min-w-0 flex-1"
									list="ledger-accounts"
									placeholder="Expenses:Food"
									bind:value={row.account}
									aria-label={`Posting ${index + 1} account`}
								/>
								<Button
									size="icon-sm"
									variant="ghost"
									aria-label={`Remove posting ${index + 1}`}
									disabled={draft.postings.length <= 1}
									onclick={() => draft.postings.splice(index, 1)}
								>
									<HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
								</Button>
							</div>
							<div class="flex items-center gap-1.5">
								<Input
									class="min-w-0 flex-1 text-right tabular-nums"
									placeholder="12.34"
									bind:value={row.units}
									aria-label={`Posting ${index + 1} amount`}
								/>
								<Input
									class="w-20"
									placeholder="currency"
									bind:value={row.currency}
									aria-label={`Posting ${index + 1} commodity`}
								/>
							</div>
						</div>
					{/each}
				</div>
				<p class="text-muted-foreground text-xs">
					An amount left empty is the leg beancount works out itself, and an empty commodity is
					the ledger's own operating currency.
				</p>
				<!--
					Completions for the account fields, from the ledger itself: what is
					open is what can be posted to.
				-->
				<datalist id="ledger-accounts">
					{#each known as name (name)}
						<option value={name}></option>
					{/each}
				</datalist>
			</div>

			<div class="flex flex-col gap-2">
				<span class="text-muted-foreground" id="new-transaction-file">File</span>
				<ToggleGroup.Root
					type="single"
					class="flex-wrap"
					value={selected}
					onValueChange={(next) => next && (chosen = next)}
					aria-labelledby="new-transaction-file"
				>
					{#each files as file (file.path)}
						<ToggleGroup.Item value={file.path} title={file.path}>{file.label}</ToggleGroup.Item>
					{/each}
				</ToggleGroup.Root>
				<p class="text-muted-foreground text-xs">
					{#if files.length === 0}
						No journal file is loaded, so there is nowhere to write an entry yet.
					{:else if !chosen && suggestion}
						Suggested from the latest entry on {suggestion.from.join(", ")}.
					{:else}
						The entry goes to the end of this file.
					{/if}
				</p>
			</div>

			{#if error}
				<Alert variant="destructive">
					<AlertDescription>{error}</AlertDescription>
				</Alert>
			{/if}
		</div>

		<Sheet.Footer>
			<div class="flex items-center gap-2">
				<Button size="sm" onclick={save} disabled={saving || selected === ""}>
					Add transaction
				</Button>
				<Button size="sm" variant="ghost" onclick={() => (open = false)} disabled={saving}>
					Cancel
				</Button>
			</div>
		</Sheet.Footer>
	</Sheet.Content>
</Sheet.Root>
