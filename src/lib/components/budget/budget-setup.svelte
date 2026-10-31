<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { Add01Icon, MinusSignIcon } from "@hugeicons/core-free-icons";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import {
		commands,
		type BudgetPluginStatus,
		type BudgetSettings,
		type JournalFile,
	} from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import { messageOf } from "$lib/errors.js";

	/**
	 * The envelope budget's configuration editor: create the block when the
	 * journal has none, edit it in place when it does.
	 *
	 * In edit mode every field is filled from the existing configuration and
	 * the write is `budget_config_set`, which rewrites the block's own lines
	 * wherever they live — including an included file — and leaves the
	 * `fund`/`rollover` directives around it untouched. In create mode the
	 * write is `budget_init`, which appends the block to the chosen file.
	 *
	 * Either write declares the plugin through the same loader-verified funnel
	 * when the journal has no `plugin` line yet. That needs
	 * `rustledger_envelope.wasm` beside the journal: when it is missing the
	 * backend refuses before writing, and this sheet shows the sentence with
	 * the exact path — plus a chooser that copies a downloaded copy there.
	 */
	let {
		open,
		settings,
		plugin,
		onclose,
		onsaved,
	}: {
		open: boolean;
		/** The configuration as the page read it; the sheet's edit mode. */
		settings: BudgetSettings | null;
		/** Whether the journal declares the plugin and where its module can sit. */
		plugin: BudgetPluginStatus | null;
		/** Closes the sheet; the page's own flag owns it. */
		onclose: () => void;
		/**
		 * After a successful write: stage the revision and reload the ledger, so
		 * the plugin's output — not an optimistic guess — is what the grid shows.
		 */
		onsaved: () => void | Promise<void>;
	} = $props();

	type Pattern = { id: number; value: string };

	/** The configuration being edited; `null` means the sheet creates one. */
	const config = $derived(settings?.config ?? null);
	/** The plugin status the sheet shows, kept current after an install here. */
	let pluginStatus = $state<BudgetPluginStatus | null>(null);

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
	/** A `.wasm` install in flight from this sheet. */
	let installing = $state(false);
	/** The backend's refusal, or the IPC's, kept inside the sheet with the draft. */
	let error = $state("");
	/** The one month spelling every budget directive and command uses. */
	const MONTH_LABEL = /^\d{4}-(0[1-9]|1[0-2])$/;
	/** Opening resets the draft once; reopening starts from the configuration again. */
	let wasOpen = false;

	/** The current month in the device's own calendar, spelled `YYYY-MM`. */
	function currentMonth(): string {
		const now = new Date();
		return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;
	}

	const selectedFile = $derived(
		config?.file ??
			(picked !== "" && files.some((file) => file.path === picked)
				? picked
				: (files[0]?.path ?? ""))
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

	/** Fill the draft from the existing configuration, or from the defaults. */
	function reset() {
		const current = settings?.config ?? null;
		start = current?.start ?? currentMonth();
		if (current?.end) {
			limit = "end";
			end = current.end;
			monthsAhead = "12";
		} else {
			limit = "months";
			// An existing block with no limit keeps the plugin's default (0
			// months past the newest directive); a new budget starts at 12.
			monthsAhead = current ? (current.months_ahead ?? "0") : "12";
			end = "";
		}
		const accounts =
			current && current.budget_accounts.length > 0 ? current.budget_accounts : ["^Assets:"];
		patterns = accounts.map((value) => ({ id: nextPattern++, value }));
		assertOn = current?.assert ?? true;
		strictOn = current?.strict ?? false;
		picked = current?.file ?? "";
		pluginStatus = plugin;
		error = "";
		saving = false;
		installing = false;
	}

	$effect(() => {
		if (!open) {
			wasOpen = false;
			return;
		}
		if (wasOpen) return;
		wasOpen = true;
		reset();
		// An edit rewrites the block where it already lives; only a create
		// chooses a file, so only a create reads the journal's files.
		if (!settings?.config) void loadFiles();
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
	 *
	 * The command is the mode: an existing configuration is replaced through
	 * `budget_config_set` (which rewrites its block where it lives), and only a
	 * missing one appends through `budget_init`.
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
			const result = config
				? await commands.budgetConfigSet(
						file,
						startMonth,
						ahead,
						endMonth,
						accounts,
						assertOn,
						strictOn
					)
				: await commands.budgetInit(
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

	/**
	 * The reader's downloaded `rustledger_envelope.wasm`: the chooser hands one
	 * back, the backend copies it beside the journal (and declares it when a
	 * configuration already exists). A missing configuration is not a failure
	 * here — the file is in place and the next submit declares it with the
	 * block. A refusal keeps its sentence with the draft.
	 */
	async function installPlugin() {
		if (installing) return;
		installing = true;
		error = "";
		try {
			const source = await commands.pickPluginFile();
			if (source === null) return;
			const result = await commands.budgetPluginInstall(source);
			if (result.status === "error") {
				error = result.error;
				return;
			}
			pluginStatus = result.data;
		} catch (cause) {
			error = messageOf(cause);
			return;
		} finally {
			installing = false;
		}
	}
</script>

<Sheet.Root open={open} onOpenChange={(next) => !next && onclose()}>
	<Sheet.Content side="bottom" class="p-0">
		<div class="mx-auto flex max-h-[88dvh] w-full max-w-xl flex-col">
			<Sheet.Header class="p-4 pb-2">
				<Sheet.Title>{config ? "Edit the envelope budget" : "Set up the envelope budget"}</Sheet.Title>
				<Sheet.Description>
					{#if config}
						Replace the ledger's <code class="font-mono">custom "envelope"</code> configuration in
						place. The block's own lines are rewritten where they live; the funding and rollover
						directives around it are left as they are.
					{:else}
						Write the ledger's <code class="font-mono">custom "envelope"</code> configuration into
						a journal file. When <code class="font-mono">rustledger_envelope.wasm</code> sits
						beside the journal the plugin is declared in the same write; the plugin computes the
						months on the next load.
					{/if}
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
					{#if pluginStatus && !pluginStatus.declared && pluginStatus.wasm.length > 0}
						{@const wasm = pluginStatus?.wasm[0] ?? null}
						<Alert class="flex flex-col gap-2">
							<div>
								<p class="text-xs font-medium">This journal does not declare the envelope plugin</p>
								<p class="text-muted-foreground text-[11px] leading-tight">
									Writing the configuration also writes
									<code class="font-mono">plugin "rustledger_envelope.wasm"</code>, which
									needs the module beside the journal. The write is refused — and nothing is
									changed — when it is missing.
								</p>
								{#if wasm}
									<p class="text-muted-foreground mt-1 flex flex-wrap items-center gap-1 text-[11px] leading-tight">
										<span>Module:</span>
										<code class="font-mono break-all">{wasm.path}</code>
										<span>{wasm.present ? "(found)" : "(not found yet)"}</span>
									</p>
								{/if}
							</div>
							<div>
								<Button
									type="button"
									size="sm"
									variant="outline"
									class="h-8"
									disabled={installing}
									onclick={() => void installPlugin()}
								>
									{installing ? "Installing…" : "Use a downloaded .wasm…"}
								</Button>
							</div>
						</Alert>
					{/if}

					<div class="flex flex-col gap-0.5">
						<span class="text-muted-foreground text-xs">Write to</span>
						{#if config}
							<!-- The block is rewritten where its `start date` lives; a
							     file picker here would suggest it can be moved. -->
							<p class="font-mono text-xs break-all">{config.file}</p>
							<p class="text-muted-foreground text-[11px] leading-tight">
								The configuration is rewritten in the file that holds it; the other journal
								files are not touched.
							</p>
						{:else}
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
						disabled={saving || installing || selectedFile === "" || loadingFiles}
					>
						{saving ? "Writing…" : config ? "Save configuration" : "Write configuration"}
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
