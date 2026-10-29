<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { resolve } from "$app/paths";
	import { onDestroy } from "svelte";
	import { commands, type Operation, type Origin } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import { messageOf } from "$lib/errors.js";
	import { bumpLedgerRevision } from "$lib/ledger-revision.svelte.js";
	import type { PageProps } from "./$types";

	/**
	 * The operation log: who wrote what, when, on which device.
	 *
	 * Everything the table shows is a line the backend recorded — the summary is
	 * the writer's own sentence, never parsed — and the two filters are the
	 * command's own: an origin, and a path matched exactly as the line recorded
	 * it. Load more grows `limit` rather than paging a cursor, so the list on
	 * screen is always the newest prefix of the log that matches.
	 *
	 * Undo writes the operation's pre-images back through the same loader-checked
	 * pipeline as any edit. It refuses, in the backend's own words, when a file
	 * moved on after the operation; that sentence is shown as it came, because it
	 * names the file that changed.
	 */
	let { data }: PageProps = $props();

	const ORIGINS = [
		{ value: "all", label: "All origins" },
		{ value: "user", label: "User" },
		{ value: "ai", label: "AI" },
		{ value: "importer", label: "Importer" },
	] as const;

	type OriginFilter = (typeof ORIGINS)[number]["value"];

	/** Each origin's own badge tone: the writer should read at a glance. */
	const ORIGIN_VARIANT: Record<Origin, "default" | "secondary" | "outline"> = {
		user: "default",
		ai: "secondary",
		importer: "outline",
	};

	/** What the last undo did, kept under the row it belongs to. */
	type Outcome = { id: string; restored: string[] } | { id: string; error: string };

	/** The route's own page of the log, taken once to seed the screen. */
	const first = untrack(() => data);
	/** The load the state below was last taken from (plain: see `branches`). */
	let adopted = first;

	let operations = $state<Operation[]>(first.operations);
	let error = $state(first.error);
	let loading = $state(false);
	let limit = $state(first.page);
	let exhausted = $state(first.operations.length < first.page);
	let origin = $state<OriginFilter>("all");
	let pathFilter = $state("");
	let busyId = $state<string | null>(null);
	let outcome = $state<Outcome | null>(null);
	let filterTimer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		// `invalidateAll` re-runs the loader — after another ledger is opened, or
		// after an undo — and the page above was seeded from an earlier read. The
		// log belongs to whichever ledger is open, so the page follows its own
		// load instead of offering the previous one's operations to Undo.
		if (data === adopted) return;
		adopted = data;
		operations = data.operations;
		limit = data.page;
		exhausted = data.operations.length < data.page;
		error = data.error;
		outcome = null;
	});

	const filtered = $derived(origin !== "all" || pathFilter.trim() !== "");

	async function load(next: number) {
		loading = true;
		error = "";
		try {
			const result = await commands.history(
				origin === "all" ? null : origin,
				pathFilter.trim() || null,
				next
			);
			if (result.status === "error") {
				error = result.error;
				return;
			}
			operations = result.data;
			limit = next;
			exhausted = result.data.length < next;
		} catch (cause) {
			console.error("history failed", cause);
			error = messageOf(cause);
		} finally {
			loading = false;
		}
	}

	function setOrigin(next: OriginFilter) {
		if (next === origin) return;
		origin = next;
		clearTimeout(filterTimer);
		void load(first.page);
	}

	/** Typing a path waits for a pause; every keystroke is not a scan. */
	function pathChanged() {
		clearTimeout(filterTimer);
		filterTimer = setTimeout(() => void load(limit), 300);
	}

	async function undo(operation: Operation) {
		busyId = operation.id;
		outcome = null;
		error = "";
		try {
			const result = await commands.historyUndo(operation.id);
			if (result.status === "error") {
				outcome = { id: operation.id, error: result.error };
				return;
			}
			outcome = { id: operation.id, restored: result.data };
			bumpLedgerRevision();
			// The undo is a write of its own, so the log on screen is read again.
			await load(limit);
		} catch (cause) {
			console.error("historyUndo failed", cause);
			outcome = { id: operation.id, error: messageOf(cause) };
		} finally {
			busyId = null;
		}
	}

	onDestroy(() => clearTimeout(filterTimer));

	/**
	 * The recorded time as local text. The log writes RFC 3339 with nanoseconds;
	 * `Date` reads milliseconds, so the fraction is cut to three digits first.
	 */
	function formatTime(at: string): string {
		const parsed = new Date(at.replace(/(\.\d{3})\d+/, "$1"));
		if (Number.isNaN(parsed.getTime())) return at;
		return parsed.toLocaleString(undefined, {
			year: "numeric",
			month: "short",
			day: "2-digit",
			hour: "2-digit",
			minute: "2-digit",
		});
	}

	function fileName(path: string): string {
		const parts = path.split(/[\\/]/);
		return parts[parts.length - 1] || path;
	}

	function pathsOf(operation: Operation): string {
		return operation.ops.map((file) => file.path).join("\n");
	}

	function namesOf(operation: Operation): string {
		if (operation.ops.length === 0) return "no files";
		return operation.ops.map((file) => fileName(file.path)).join(", ");
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">History</h1>
		<span class="text-muted-foreground text-xs">
			every write the app recorded, newest first
		</span>
		<div class="ml-auto flex items-center gap-1">
			<Button size="sm" variant="ghost" onclick={() => load(limit)} disabled={loading}>
				Refresh
			</Button>
			<Button size="sm" variant="outline" href={resolve("/conflicts")}>Conflicts</Button>
		</div>
	</div>

	<div class="flex flex-wrap items-end gap-2">
		<div class="flex min-w-32 flex-col gap-0.5">
			<span class="text-muted-foreground text-[0.625rem]">Origin</span>
			<Select.Root
				type="single"
				value={origin}
				onValueChange={(next) => next && setOrigin(next as OriginFilter)}
			>
				<Select.Trigger type="button" class="w-full sm:w-36" aria-label="Filter by origin">
					<span data-slot="select-value" class="min-w-0 truncate">
						{origin === "all" ? "All origins" : origin}
					</span>
				</Select.Trigger>
				<Select.Content>
					{#each ORIGINS as option (option.value)}
						<Select.Item value={option.value} label={option.label} />
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex min-w-0 flex-1 flex-col gap-0.5">
			<span class="text-muted-foreground text-[0.625rem]">
				Path, exactly as the log recorded it
			</span>
			<Input
				class="w-full font-mono"
				bind:value={pathFilter}
				oninput={pathChanged}
				placeholder="/home/…/main.bean"
				autocomplete="off"
				spellcheck={false}
				aria-label="Filter by exact path"
			/>
		</div>
	</div>

	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{/if}

	{#if operations.length === 0}
		{#if !error}
			<div class="flex flex-col gap-0.5">
				<h2 class="text-sm font-medium">
					{filtered ? "No writes match this filter" : "No writes recorded yet"}
				</h2>
				<p class="text-muted-foreground text-xs">
					{#if filtered}
						The log matches a path exactly as it was recorded and filters by who wrote it;
						clear one of the two to widen the list.
					{:else}
						The history journal fills as this app writes the ledger — each edit, import and
						assistant change adds a line with the files it touched.
					{/if}
				</p>
			</div>
		{/if}
	{:else}
		<div class="rounded-lg border">
			<!--
				The column names, on a wide screen only: on a phone each row stacks its
				pieces instead, so a header row would sit above nothing it lines up with.
			-->
			<div
				class="text-muted-foreground hidden grid-cols-[8.5rem_5.5rem_minmax(0,1fr)_4rem_7rem_4.5rem] items-center gap-2 border-b px-3 py-1.5 text-xs sm:grid"
			>
				<span>Time</span>
				<span>Origin</span>
				<span>Summary</span>
				<span class="text-right">Files</span>
				<span>Device</span>
				<span></span>
			</div>
			<ul class="divide-border divide-y">
				{#each operations as operation (operation.id)}
					<li
						class="px-3 py-2 sm:grid sm:grid-cols-[8.5rem_5.5rem_minmax(0,1fr)_4rem_7rem_4.5rem] sm:items-center sm:gap-2"
						aria-busy={busyId === operation.id}
					>
						<div class="flex flex-wrap items-center gap-x-2 gap-y-1 sm:contents">
							<span
								class="text-muted-foreground font-mono text-[0.6875rem] whitespace-nowrap"
								title={operation.at}
							>
								{formatTime(operation.at)}
							</span>
							<Badge
								class="sm:justify-self-start"
								variant={ORIGIN_VARIANT[operation.origin]}
							>
								{operation.origin}
							</Badge>
							<span class="w-full min-w-0 sm:col-start-3 sm:w-auto">
								<span class="block truncate text-sm" title={operation.summary}>
									{operation.summary || "—"}
								</span>
								<span
									class="text-muted-foreground block truncate font-mono text-[0.625rem]"
									title={pathsOf(operation)}
								>
									{namesOf(operation)}
								</span>
							</span>
							<span class="text-muted-foreground text-xs whitespace-nowrap sm:text-right">
								{operation.ops.length}
								{operation.ops.length === 1 ? " file" : " files"}
							</span>
							<span
								class="text-muted-foreground max-w-32 min-w-0 truncate text-xs"
								title={operation.device}
							>
								{operation.device}
							</span>
							<Button
								class="ms-auto sm:ms-0 sm:justify-self-end"
								size="sm"
								variant="outline"
								disabled={busyId !== null}
								onclick={() => undo(operation)}
							>
								{busyId === operation.id ? "Undoing…" : "Undo"}
							</Button>
							{#if outcome && outcome.id === operation.id}
								<div class="mt-1.5 w-full sm:col-span-full sm:mt-1">
									{#if "error" in outcome}
										<Alert variant="destructive">
											<span class="font-mono break-all">{outcome.error}</span>
										</Alert>
									{:else}
										<Alert>
											{#if outcome.restored.length > 0}
												Restored {outcome.restored.length}
												{outcome.restored.length === 1 ? "file" : "files"}:
												<span class="font-mono break-all">
													{outcome.restored.join(", ")}
												</span>
											{:else}
												Undone.
											{/if}
										</Alert>
									{/if}
								</div>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		</div>

		<div class="flex flex-wrap items-center gap-2">
			<Button
				size="sm"
				variant="outline"
				onclick={() => load(limit + first.page)}
				disabled={loading || exhausted}
			>
				{loading ? "Loading…" : "Load more"}
			</Button>
			<span class="text-muted-foreground text-xs">
				{operations.length} {operations.length === 1 ? "write" : "writes"}
				{#if exhausted}&nbsp;— that is the whole log{/if}
			</span>
		</div>
	{/if}
</div>
