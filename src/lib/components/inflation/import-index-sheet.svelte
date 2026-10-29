<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands, type IndexSeries } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import { Textarea } from "$lib/components/ui/textarea/index.js";
	import { messageOf } from "$lib/errors.js";

	/**
	 * A publisher's monthly table, pasted in as `date,value` rows and stored as
	 * a series. The backend parses the rows — a header line, blank lines and `#`
	 * comments are skipped, `YYYY-MM` and `YYYY-MM-DD` both normalise to the
	 * month — so this sheet never reads the CSV itself, and its refusal is shown
	 * here, verbatim, next to the text that caused it.
	 */
	let {
		open,
		onclose,
		onimported,
	}: {
		open: boolean;
		onclose: () => void;
		/** The stored series, so the page can list it and compare against it. */
		onimported: (series: IndexSeries) => void;
	} = $props();

	/**
	 * Provider ids this build can also fetch, shown where the id is typed. They
	 * come from the backend so the hint cannot name a provider that is gone.
	 */
	let providers = $state<string[]>([]);
	$effect(() => {
		void commands.indexProviderList().then(
			(list) => (providers = list.map((provider) => provider.id)),
			() => (providers = [])
		);
	});

	let id = $state("");
	let label = $state("");
	let source = $state("");
	let currency = $state("");
	let csv = $state("");
	let busy = $state(false);
	let answer = $state("");

	const ready = $derived(
		id.trim() !== "" &&
			label.trim() !== "" &&
			source.trim() !== "" &&
			currency.trim() !== "" &&
			csv.trim() !== ""
	);

	async function submit() {
		if (!ready || busy) return;
		busy = true;
		answer = "";
		try {
			const result = await commands.indexImport(
				id.trim(),
				label.trim(),
				source.trim(),
				currency.trim().toUpperCase(),
				csv
			);
			if (result.status === "error") {
				answer = result.error;
				return;
			}
			onimported(result.data);
			id = label = source = currency = csv = "";
		} catch (cause) {
			answer = messageOf(cause);
		} finally {
			busy = false;
		}
	}
</script>

<Sheet.Root open={open} onOpenChange={(next) => !next && onclose()}>
	<Sheet.Content side="bottom" class="p-0">
		<div class="mx-auto flex max-h-[85dvh] w-full max-w-xl flex-col">
			<Sheet.Header class="p-4 pb-2">
				<Sheet.Title>Import an index</Sheet.Title>
				<Sheet.Description>
					One row per month, the date first and the index value second. The value is a price
					level, not a percentage: the price of one unit of the currency in that month.
				</Sheet.Description>
			</Sheet.Header>

			<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 pb-4">
				<div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
					<div class="flex flex-col gap-0.5">
						<span class="text-muted-foreground text-xs">Id</span>
						<Input
							class="h-8"
							aria-label="Series id"
							placeholder="tr-cpi"
							bind:value={id}
							autocapitalize="off"
							autocomplete="off"
							spellcheck="false"
						/>
						<span class="text-muted-foreground text-[0.625rem]">
							An id a built-in provider uses ({providers.join(", ") || "none in this build"}) can also be refreshed from its
							publisher; any other id is import-only.
						</span>
					</div>
					<div class="flex flex-col gap-0.5">
						<span class="text-muted-foreground text-xs">Label</span>
						<Input
							class="h-8"
							aria-label="Series label"
							placeholder="Türkiye CPI"
							bind:value={label}
						/>
					</div>
					<div class="flex flex-col gap-0.5">
						<span class="text-muted-foreground text-xs">Source</span>
						<Input
							class="h-8"
							aria-label="Publisher"
							placeholder="TÜİK"
							bind:value={source}
						/>
						<!-- A source naming ENAG is stored as unofficial: the store says so, this repeats it. -->
						<span class="text-muted-foreground text-[0.625rem]">
							The publisher's name. One naming ENAG is stored as unofficial.
						</span>
					</div>
					<div class="flex flex-col gap-0.5">
						<span class="text-muted-foreground text-xs">Currency</span>
						<Input
							class="h-8"
							aria-label="Currency the index measures"
							placeholder="TRY"
							bind:value={currency}
							autocapitalize="characters"
							autocomplete="off"
							spellcheck="false"
							maxlength={12}
						/>
						<span class="text-muted-foreground text-[0.625rem]">
							The money the index measures; a ledger in another currency is refused.
						</span>
					</div>
				</div>

				<div class="flex flex-col gap-0.5">
					<span class="text-muted-foreground text-xs">Rows</span>
					<Textarea
						class="min-h-40 font-mono text-xs"
						aria-label="Index rows"
						placeholder={"date,value\n2026-01,100.00\n2026-02,103.81"}
						bind:value={csv}
						spellcheck="false"
					/>
					<span class="text-muted-foreground text-[0.625rem]">
						Comma-separated, a dot for the decimal mark — the comma is the field separator. A
						header line is skipped.
					</span>
				</div>

				{#if answer}
					<Alert variant="destructive">{answer}</Alert>
				{/if}
			</div>

			<Sheet.Footer class="flex-row flex-wrap items-center gap-2 border-t px-4 py-3">
				<Button class="h-8" disabled={!ready || busy} onclick={submit}>
					{busy ? "Importing…" : "Import"}
				</Button>
				<Button class="h-8" variant="ghost" onclick={onclose}>Cancel</Button>
			</Sheet.Footer>
		</div>
	</Sheet.Content>
</Sheet.Root>
