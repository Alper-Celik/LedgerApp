<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { goto } from "$app/navigation";
	import { resolve } from "$app/paths";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import {
		definitionFor,
		widgetDefinitions,
		type WidgetKind,
		type WidgetSpan,
	} from "$lib/dashboard/index.js";
	import { messageOf } from "$lib/errors.js";
	import { commands, type Dashboard } from "../../../bindings";
	import { isWidgetKind, prepareWidgets, type DraftWidget } from "./data.js";
	import EditorRow from "./editor-row.svelte";

	/**
	 * A dashboard's layout, edited.
	 *
	 * The editor works on a draft — name, widget order, spans, membership — and
	 * writes nothing until Save. Every widget is checked through the registry as
	 * it is loaded and again as it is saved: a config read off disk is normalised
	 * against its kind's defaults, and a span that is not 1, 2 or 4 falls back to
	 * the kind's width. An id that is not saved yet is a new dashboard; Save is
	 * the create.
	 *
	 * The route mounts this under `{#key id}`: the draft belongs to one id, and
	 * navigating to another dashboard's editor must not carry it across.
	 */
	let {
		id,
		dashboard,
		loadError,
	}: {
		id: string;
		/** The saved dashboard, or `null` when this id has not been saved yet. */
		dashboard: Dashboard | null;
		/** Why the store could not be read, or `""`. */
		loadError: string;
	} = $props();

	let name = $state(untrack(() => dashboard?.name ?? "New dashboard"));
	let widgets = $state<DraftWidget[]>(
		untrack(() => dashboard)
			? prepareWidgets(untrack(() => dashboard!)).map(({ id: widgetId, kind, config, span }) => ({
					id: widgetId,
					kind,
					config,
					span,
				}))
			: []
	);

	const defaultKind: WidgetKind = widgetDefinitions[0]?.kind ?? "net-worth";
	let pickedKind = $state<string>(defaultKind);
	const pickedDefinition = $derived(
		isWidgetKind(pickedKind) ? definitionFor(pickedKind) : null
	);

	let busy = $state(false);
	let error = $state("");
	let confirmingDelete = $state(false);
	let counter = 0;

	const canDelete = $derived(dashboard !== null);
	const cancelHref = $derived(
		dashboard ? resolve("/dashboards/[id]", { id }) : resolve("/dashboards")
	);

	/** A widget id that is unique within this draft; only its uniqueness matters. */
	function freshWidgetId(): string {
		counter += 1;
		return `widget-${Date.now().toString(36)}-${counter.toString(36)}`;
	}

	function addWidget() {
		if (!pickedDefinition) return;
		widgets = [
			...widgets,
			{
				id: freshWidgetId(),
				kind: pickedDefinition.kind,
				config: pickedDefinition.normalise({}),
				span: pickedDefinition.defaultSpan,
			},
		];
	}

	function moveWidget(index: number, delta: -1 | 1) {
		const target = index + delta;
		if (target < 0 || target >= widgets.length) return;
		const next = [...widgets];
		const [moved] = next.splice(index, 1);
		if (!moved) return;
		next.splice(target, 0, moved);
		widgets = next;
	}

	function removeWidget(index: number) {
		widgets = widgets.filter((_, at) => at !== index);
	}

	function changeSpan(index: number, span: WidgetSpan) {
		widgets = widgets.map((widget, at) => (at === index ? { ...widget, span } : widget));
	}

	/**
	 * Save the draft and open the rendered view.
	 *
	 * The order the list is in becomes the saved `order`, so a move is persisted
	 * by position rather than by rewriting every widget's number by hand. A
	 * known kind's config is normalised one last time on the way out.
	 */
	async function save() {
		if (busy) return;
		busy = true;
		error = "";
		try {
			const result = await commands.saveDashboard({
				id,
				name: name.trim() || "Untitled dashboard",
				widgets: widgets.map((widget, order) => ({
					id: widget.id,
					kind: widget.kind,
					config: isWidgetKind(widget.kind)
						? definitionFor(widget.kind).normalise(widget.config)
						: widget.config,
					span: widget.span,
					order,
				})),
			});
			if (result.status === "error") {
				error = result.error;
				return;
			}
			await goto(resolve("/dashboards/[id]", { id }));
		} catch (cause) {
			error = messageOf(cause);
		} finally {
			busy = false;
		}
	}

	async function deleteDashboard() {
		if (busy) return;
		busy = true;
		error = "";
		try {
			const result = await commands.deleteDashboard(id);
			if (result.status === "error") {
				error = result.error;
				return;
			}
			await goto(resolve("/dashboards"));
		} catch (cause) {
			error = messageOf(cause);
		} finally {
			busy = false;
		}
	}
</script>

<div class="mx-auto flex w-full max-w-3xl flex-col gap-4 p-4">
	<header class="flex flex-wrap items-center gap-2">
		<div class="min-w-0 flex-1">
			<h1 class="text-lg font-semibold tracking-tight">
				{dashboard ? "Edit dashboard" : "New dashboard"}
			</h1>
			<p class="text-muted-foreground truncate text-xs">
				{dashboard
					? "Changes are written only when you save."
					: "Nothing is written until you save."}
			</p>
		</div>
		<Button variant="outline" size="lg" class="h-8" href={resolve("/dashboards")}>
			All dashboards
		</Button>
	</header>

	{#if loadError}
		<Alert variant="destructive">{loadError}</Alert>
	{/if}
	{#if error}
		<Alert variant="destructive">{error}</Alert>
	{/if}

	<label class="flex flex-col gap-1.5" for="dashboard-name">
		<span class="text-xs font-medium">Name</span>
		<Input
			id="dashboard-name"
			bind:value={name}
			class="h-8"
			placeholder="Dashboard name"
			autocomplete="off"
		/>
	</label>

	<section class="flex flex-col gap-2">
		<div class="flex flex-wrap items-baseline justify-between gap-2">
			<h2 class="text-sm font-semibold tracking-tight">Widgets</h2>
			<span class="text-muted-foreground text-xs">
				{widgets.length}
				{widgets.length === 1 ? "widget" : "widgets"}
			</span>
		</div>

		{#if widgets.length === 0}
			<p
				class="text-muted-foreground flex items-center justify-center rounded-lg border border-dashed border-border px-4 py-6 text-center text-xs"
			>
				No widgets yet. Pick a kind below and add the first one.
			</p>
		{:else}
			<div class="flex flex-col gap-2">
				{#each widgets as widget, index (`${widget.id}:${index}`)}
					<EditorRow
						{widget}
						{index}
						count={widgets.length}
						onmove={moveWidget}
						onremove={removeWidget}
						onspan={changeSpan}
					/>
				{/each}
			</div>
		{/if}
	</section>

	<section class="flex flex-col gap-2 rounded-xl border border-dashed border-border p-3">
		<div class="flex flex-col gap-2 sm:flex-row sm:items-end">
			<label class="flex min-w-0 flex-1 flex-col gap-1.5" for="widget-kind">
				<span class="text-xs font-medium">Add a widget</span>
				<select
					id="widget-kind"
					bind:value={pickedKind}
					class="border-input bg-input/20 dark:bg-input/30 focus-visible:border-ring focus-visible:ring-ring/30 h-8 w-full rounded-md border px-2 text-sm outline-none focus-visible:ring-2 md:text-xs/relaxed"
				>
					{#each widgetDefinitions as definition (definition.kind)}
						<option value={definition.kind}>{definition.title}</option>
					{/each}
				</select>
			</label>
			<Button size="lg" class="h-8" onclick={addWidget} disabled={!pickedDefinition}>
				Add widget
			</Button>
		</div>
		{#if pickedDefinition}
			<p class="text-muted-foreground text-xs">{pickedDefinition.description}</p>
		{/if}
	</section>

	<div class="flex flex-wrap items-center gap-2 border-t border-border pt-3">
		<Button size="lg" class="h-8" onclick={save} disabled={busy}>
			{busy ? "Saving…" : "Save"}
		</Button>
		<Button variant="outline" size="lg" class="h-8" href={cancelHref} disabled={busy}>
			Cancel
		</Button>

		{#if canDelete}
			{#if confirmingDelete}
				<span class="text-destructive text-xs">
					Delete “{name.trim() || "this dashboard"}”?
				</span>
				<Button
					variant="destructive"
					size="lg"
					class="h-8"
					onclick={deleteDashboard}
					disabled={busy}
				>
					Delete
				</Button>
				<Button
					variant="ghost"
					size="lg"
					class="h-8"
					onclick={() => (confirmingDelete = false)}
					disabled={busy}
				>
					Keep
				</Button>
			{:else}
				<Button
					variant="destructive"
					size="lg"
					class="ml-auto h-8"
					onclick={() => (confirmingDelete = true)}
					disabled={busy}
				>
					Delete
				</Button>
			{/if}
		{/if}
	</div>
</div>
