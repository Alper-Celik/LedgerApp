<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands, type JournalFile, type PriceQuote } from "../../../bindings";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import { pairKey } from "$lib/components/prices/prices.js";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import { messageOf } from "$lib/errors.js";

	/**
	 * The way a fetched quote becomes part of the ledger: pick which answers to
	 * keep and which file writes them, and the backend does the writing through
	 * the same reload-and-rollback pipeline as every other edit — this sheet
	 * never touches a file itself. Its answer is shown where the choice was
	 * made, so a refusal from the loader reads as a refusal and not as success.
	 */
	let {
		open,
		quotes,
		files,
		defaultFile,
		onclose,
		onwritten,
	}: {
		open: boolean;
		/** Quotes fetched this visit and not yet written, all included until unticked. */
		quotes: PriceQuote[];
		files: JournalFile[];
		/** The file to preselect: the root journal, or the journal's first file. */
		defaultFile: string;
		onclose: () => void;
		/** What was written, so the page can forget exactly those and refetch. */
		onwritten: (written: PriceQuote[]) => void;
	} = $props();

	/** The file picked here, or `""` while the default still stands. */
	let picked = $state("");
	/** Quotes unticked by their pair key; unticked rather than ticked so new ones start included. */
	let excluded = $state<Record<string, boolean>>({});
	let writing = $state(false);
	let answer = $state<{ ok: boolean; text: string } | null>(null);

	const selectedFile = $derived(
		picked !== "" && files.some((file) => file.path === picked) ? picked : defaultFile
	);
	const selectedLabel = $derived(
		files.find((file) => file.path === selectedFile)?.label ?? selectedFile
	);
	const included = $derived(
		quotes.filter((quote) => excluded[pairKey(quote.commodity, quote.currency)] !== true)
	);
	/** The backend's answer is about the choice that produced it; a new choice clears it. */
	function changed() {
		answer = null;
	}

	function toggle(quote: PriceQuote) {
		const key = pairKey(quote.commodity, quote.currency);
		excluded[key] = excluded[key] !== true;
		changed();
	}

	async function write() {
		if (selectedFile === "" || included.length === 0) return;
		const chosen = included;
		writing = true;
		answer = null;
		try {
			const result = await commands.writePrices(selectedFile, chosen);
			if (result.status === "error") {
				answer = { ok: false, text: result.error };
				return;
			}
			answer = {
				ok: true,
				text: `Wrote ${chosen.length} ${chosen.length === 1 ? "price" : "prices"} to ${selectedLabel}.`,
			};
			onwritten(chosen);
			// The written quotes are gone from the list; their unticked marks would
			// otherwise outlive them and hide a later fetch of the same pair.
			for (const quote of chosen) delete excluded[pairKey(quote.commodity, quote.currency)];
		} catch (cause) {
			answer = { ok: false, text: messageOf(cause) };
		} finally {
			writing = false;
		}
	}
</script>

<Sheet.Root open={open} onOpenChange={(next) => !next && onclose()}>
	<Sheet.Content side="bottom" class="p-0">
		<div class="mx-auto flex max-h-[85dvh] w-full max-w-xl flex-col">
			<Sheet.Header class="p-4 pb-2">
				<Sheet.Title>Add prices to a file</Sheet.Title>
				<Sheet.Description>
					The quotes fetched here, written as price directives into one journal file. The
					ledger is reloaded once they are in, and refused quotes roll the file back.
				</Sheet.Description>
			</Sheet.Header>

			<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 pb-4">
				<div class="flex flex-col gap-0.5">
					<span class="text-muted-foreground text-xs">Write to</span>
					<Select.Root
						type="single"
						value={selectedFile}
						onValueChange={(next) => {
							if (!next) return;
							picked = next;
							changed();
						}}
					>
						<Select.Trigger
							type="button"
							class="data-[size=default]:h-8 w-full sm:data-[size=default]:h-7"
							aria-label="Target journal file"
							disabled={files.length === 0}
						>
							<span data-slot="select-value" class="min-w-0 truncate">
								{files.length === 0 ? "No journal files" : selectedLabel || "Choose a file"}
							</span>
						</Select.Trigger>
						<Select.Content>
							{#each files as file (file.path)}
								<Select.Item value={file.path} label={file.label} title={file.path} />
							{/each}
						</Select.Content>
					</Select.Root>
				</div>

				<div class="flex flex-col gap-1">
					<div class="flex items-center justify-between gap-2">
						<span class="text-muted-foreground text-xs">Quotes</span>
						<span class="text-muted-foreground text-xs">
							{included.length} of {quotes.length} selected
						</span>
					</div>
					{#if quotes.length === 0}
						<p class="text-muted-foreground text-xs">
							No quote is waiting: fetch a row on the prices screen, then come back here.
						</p>
					{:else}
						<!--
							A whole line is the target, tall enough for a thumb, and the
							price is shown with the date and source it was answered with —
							the same figures the row printed when it was fetched.
						-->
						<ul class="divide-border divide-y rounded-md border">
							{#each quotes as quote (pairKey(quote.commodity, quote.currency))}
								<li>
									<label
										class="hover:bg-muted flex min-h-8 cursor-pointer items-center gap-2 px-2 py-1.5"
									>
										<input
											type="checkbox"
											class="accent-primary size-4 shrink-0"
											checked={excluded[pairKey(quote.commodity, quote.currency)] !== true}
											onchange={() => toggle(quote)}
										/>
										<span class="min-w-0 flex-1 truncate text-xs" title={quote.source}>
											{quote.commodity} → {quote.currency}
										</span>
										<MoneyAmount
											units={quote.price}
											currency={quote.currency}
											class="text-foreground shrink-0"
										/>
										<span class="text-muted-foreground shrink-0 text-xs">{quote.date}</span>
									</label>
								</li>
							{/each}
						</ul>
					{/if}
				</div>

				{#if answer}
					<Alert variant={answer.ok ? "default" : "destructive"}>{answer.text}</Alert>
				{/if}
			</div>

			<Sheet.Footer class="flex-row flex-wrap items-center gap-2 border-t px-4 py-3">
				<Button
					class="h-8"
					variant="secondary"
					disabled={writing || included.length === 0 || selectedFile === ""}
					onclick={write}
				>
					{writing ? "Writing…" : "Write"}
				</Button>
				<Button class="h-8" variant="ghost" onclick={onclose}>Cancel</Button>
				{#if selectedFile !== ""}
					<!--
						The whole file, opened in the view that already owns editing it:
						this sheet writes directives, it does not become an editor.
					-->
					<Button
						class="ms-auto h-8"
						variant="ghost"
						href={`/files/${encodeURIComponent(selectedFile)}`}
						onclick={onclose}
					>
						Edit this file
					</Button>
				{/if}
			</Sheet.Footer>
		</div>
	</Sheet.Content>
</Sheet.Root>
