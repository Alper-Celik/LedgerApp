<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { Add01Icon, MinusSignIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { commands, type JournalFile } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import { messageOf } from "$lib/errors.js";

	/**
	 * The way into an envelope budget: write the plugin's `custom "envelope"`
	 * configuration block into a journal file.
	 *
	 * This sheet writes configuration and nothing else — the plugin computes the
	 * months from it on the next load. When the journal does not declare the
	 * plugin, the sheet says so and still writes: installing a WASM plugin is not
	 * something the app can do to a ledger, so the reader adds that line.
	 */
	let {
		open,
		pluginDeclared,
		onclose,
		onsaved,
	}: {
		open: boolean;
		/** Whether the journal already loads an envelope plugin. */
		pluginDeclared: boolean;
		/** Closes the sheet; the page's own flag owns it. */
		onclose: () => void;
		/**
		 * After a successful write: stage the revision and reload the ledger, so
		 * the plugin's output — not an optimistic guess — is what the grid shows.
		 */
		onsaved: () => void | Promise<void>;
	} = $props();

	type Pattern = { id: number; value: string };

	let files = $state<JournalFile[]>([]);
	let loadingFiles = $state(false);
	let filesError = $state("");
	let picked = $state("");
	let start = $state(currentMonth());
	/** Which of the two limits to write; the plugin accepts either, and one is enough. */
	let limit = $state<"months" | "end">("months");
	/** A number field binds as a number, a month field as a string; the command takes either spelling. */
	let monthsAhead = $state<string | number>("12");
	let end = $state("");
	let patterns = $state<Pattern[]>([{ id: 1, value: "^Assets:" }]);
	let nextPattern = 2;
	let assertOn = $state(true);
	let strictOn = $state(false);
	let saving = $state(false);
	/** The backend's refusal, or the IPC's, kept inside the sheet with the draft. */
	let error = $state("");
	/** The one month spelling every budget directive and command uses. */
	const MONTH_LABEL = /^\d{4}-(0[1-9]|1[0-2])$/;
	/** Opening resets the draft once; reopening starts from the defaults again. */
	let wasOpen = false;

	/** The current month in the device's own calendar, spelled `YYYY-MM`. */
	function currentMonth(): string {
		const now = new Date();
		return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;
	}

	const selectedFile = $derived(
		picked !== "" && files.some((file) => file.path === picked)
			? picked
			: (files[0]?.path ?? "")
	);

	async function loadFiles() {
		loadingFiles = true;
		filesError = "";
		try {
			files = await commands.journalFiles();
		} catch (cause) {
			files = [];
			filesError = messageOf(cause);
		} finally {
			loadingFiles = false;
		}
	}

	function reset() {
		start = currentMonth();
		limit = "months";
		monthsAhead = "12";
		end = "";
		patterns = [{ id: nextPattern++, value: "^Assets:" }];
		assertOn = true;
		strictOn = false;
		picked = "";
		error = "";
		saving = false;
	}

	$effect(() => {
		if (!open) {
			wasOpen = false;
			return;
		}
		if (wasOpen) return;
		wasOpen = true;
		reset();
		void loadFiles();
	});

	function addPattern() {
		patterns.push({ id: nextPattern++, value: "" });
	}

	function removePattern(id: number) {
		patterns = patterns.filter((pattern) => pattern.id !== id);
	}

	/**
	 * Validate what the backend would refuse to write and then write it. A
	 * refusal stays in the sheet, with every field as it was, so the sentence can
	 * be read next to the choice that caused it.
	 */
	async function submit() {
		if (saving) return;
		error = "";
		const file = selectedFile;
		const startMonth = start.trim();
		if (file === "") {
			error = "Pick the journal file to write the budget into.";
			return;
		}
		if (!MONTH_LABEL.test(startMonth)) {
			error = "Start month must be a calendar month, e.g. 2026-01.";
			return;
		}
		let ahead: number | null = null;
		let endMonth: string | null = null;
		if (limit === "months") {
			const raw = String(monthsAhead).trim();
			if (!/^\d+$/.test(raw)) {
				error = "Months ahead must be a whole number, 0 or more.";
				return;
			}
			ahead = Number(raw);
		} else {
			const raw = end.trim();
			if (!MONTH_LABEL.test(raw)) {
				error = "End month must be a calendar month, e.g. 2026-12.";
				return;
			}
			endMonth = raw;
		}
		const accounts = patterns.map((pattern) => pattern.value.trim()).filter((value) => value !== "");
		if (accounts.length === 0) {
			error = "Add at least one on-budget account pattern.";
			return;
		}
		saving = true;
		try {
			const result = await commands.budgetInit(
				file,
				startMonth,
				ahead,
				endMonth,
				accounts,
				assertOn,
				strictOn
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
		onclose();
		await onsaved();
	}
</script>

<Sheet.Root open={open} onOpenChange={(next) => !next && onclose()}>
	<Sheet.Content side="bottom" class="p-0">
		<div class="mx-auto flex max-h-[88dvh] w-full max-w-xl flex-col">
			<Sheet.Header class="p-4 pb-2">
				<Sheet.Title>Set up the envelope budget</Sheet.Title>
				<Sheet.Description>
					Write the ledger's <code class="font-mono">custom "envelope"</code> configuration into
					a journal file. This writes the configuration only — the envelope plugin computes the
					months from it on the next load.
				</Sheet.Description>
			</Sheet.Header>

			<form
				class="flex min-h-0 flex-1 flex-col"
				onsubmit={(event) => {
					event.preventDefault();
					void submit();
				}}
			>
				<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 pb-4">
					{#if !pluginDeclared}
						<Alert>
							This journal does not declare
							<code class="font-mono">plugin "rustledger_envelope.wasm"</code>. The app cannot
							install a plugin: add that line to the journal yourself. Setup writes the
							configuration only, so the grid stays empty until the plugin is declared and the
							ledger reloads.
						</Alert>
					{/if}

					<div class="flex flex-col gap-0.5">
						<span class="text-muted-foreground text-xs">Write to</span>
						<Select.Root
							type="single"
							value={selectedFile}
							onValueChange={(next) => {
								if (next) picked = next;
							}}
						>
							<Select.Trigger
								type="button"
								class="data-[size=default]:h-8 w-full sm:data-[size=default]:h-7"
								aria-label="Target journal file"
								disabled={files.length === 0 || loadingFiles}
							>
								<span data-slot="select-value" class="min-w-0 truncate">
									{loadingFiles
										? "Reading the journal's files…"
										: files.length === 0
											? "No journal files"
											: (files.find((file) => file.path === selectedFile)?.label ??
												"Choose a file")}
								</span>
							</Select.Trigger>
							<Select.Content>
								{#each files as file (file.path)}
									<Select.Item value={file.path} label={file.label} title={file.path} />
								{/each}
							</Select.Content>
						</Select.Root>
						{#if filesError}
							<Alert variant="destructive">{filesError}</Alert>
						{/if}
					</div>

					<div class="flex flex-wrap items-end gap-2">
						<label class="flex w-36 shrink-0 flex-col gap-0.5">
							<span class="text-muted-foreground text-xs">Start month</span>
							<Input
								type="month"
								class="h-8 tabular-nums sm:h-7"
								aria-label="Start month"
								bind:value={start}
							/>
						</label>
						<div class="flex min-w-0 grow basis-64 flex-col gap-0.5">
							<span class="text-muted-foreground text-xs">How far it runs</span>
							<div class="flex flex-wrap items-end gap-2">
								<ToggleGroup.Root
									type="single"
									value={limit}
									onValueChange={(next) => {
										if (next === "months" || next === "end") limit = next;
									}}
									aria-label="Budget end"
								>
									<ToggleGroup.Item value="months">Months ahead</ToggleGroup.Item>
									<ToggleGroup.Item value="end">End month</ToggleGroup.Item>
								</ToggleGroup.Root>
								{#if limit === "months"}
									<label class="flex w-24 flex-col gap-0.5">
										<span class="sr-only">Months ahead</span>
										<Input
											type="number"
											min="0"
											step="1"
											class="h-8 text-right tabular-nums sm:h-7"
											aria-label="Months ahead"
											bind:value={monthsAhead}
										/>
									</label>
								{:else}
									<label class="flex w-36 flex-col gap-0.5">
										<span class="sr-only">End month</span>
										<Input
											type="month"
											class="h-8 tabular-nums sm:h-7"
											aria-label="End month"
											bind:value={end}
										/>
									</label>
								{/if}
							</div>
						</div>
					</div>
					<p class="text-muted-foreground -mt-1 text-[11px] leading-tight">
						One or the other is written: months ahead extends the budget past the newest directive
						by a rolling count, while an end month stops it at a fixed month.
					</p>

					<div class="flex flex-col gap-1">
						<div class="flex items-center justify-between gap-2">
							<span class="text-muted-foreground text-xs">On-budget account patterns</span>
							<span class="text-muted-foreground text-[10px]">regular expressions</span>
						</div>
						{#each patterns as pattern, index (pattern.id)}
							<div class="flex items-center gap-1">
								<Input
									class="h-8 min-w-0 flex-1 font-mono sm:h-7"
									aria-label={`Budget account pattern ${index + 1}`}
									bind:value={pattern.value}
								/>
								<Button
									type="button"
									variant="ghost"
									size="icon-sm"
									aria-label={`Remove budget account pattern ${index + 1}`}
									disabled={patterns.length === 1}
									onclick={() => removePattern(pattern.id)}
								>
									<HugeiconsIcon icon={MinusSignIcon} strokeWidth={2} />
								</Button>
							</div>
						{/each}
						<div>
							<Button type="button" variant="outline" size="sm" class="h-8 sm:h-7" onclick={addPattern}>
								<HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
								Add pattern
							</Button>
						</div>
						<p class="text-muted-foreground text-[11px] leading-tight">
							The on-budget accounts the plugin funds from; <code class="font-mono">^Assets:</code>
							is the usual start.
						</p>
					</div>

					<div class="flex flex-col gap-1.5">
						<label class="flex items-start gap-2">
							<input
								type="checkbox"
								class="accent-primary mt-0.5 size-4 shrink-0"
								bind:checked={assertOn}
							/>
							<span class="flex min-w-0 flex-col gap-0.5">
								<span class="text-xs">Emit month-end assertions</span>
								<span class="text-muted-foreground text-[11px] leading-tight">
									The plugin checks its own arithmetic at each month's end. On by default.
								</span>
							</span>
						</label>
						<label class="flex items-start gap-2">
							<input
								type="checkbox"
								class="accent-primary mt-0.5 size-4 shrink-0"
								bind:checked={strictOn}
							/>
							<span class="flex min-w-0 flex-col gap-0.5">
								<span class="text-xs">Warn about never-funded spending</span>
								<span class="text-muted-foreground text-[11px] leading-tight">
									The plugin warns when a category that was never funded is spent from. Off by
									default.
								</span>
							</span>
						</label>
					</div>

					{#if error}
						<Alert variant="destructive">{error}</Alert>
					{/if}
				</div>

				<Sheet.Footer class="flex-row flex-wrap items-center gap-2 border-t px-4 py-3">
					<Button
						type="submit"
						class="h-8"
						disabled={saving || selectedFile === "" || loadingFiles}
					>
						{saving ? "Writing…" : "Write configuration"}
					</Button>
					<Button type="button" variant="ghost" class="h-8" onclick={onclose} disabled={saving}>
						Cancel
					</Button>
					<span class="text-muted-foreground ms-auto text-[10px]">
						the plugin computes the months; this writes its configuration
					</span>
				</Sheet.Footer>
			</form>
		</div>
	</Sheet.Content>
</Sheet.Root>
