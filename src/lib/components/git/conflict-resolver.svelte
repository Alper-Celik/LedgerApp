<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { onMount, tick } from "svelte";
	import { commands, type ConflictTexts } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";
	import { Textarea } from "$lib/components/ui/textarea/index.js";
	import { messageOf } from "$lib/errors.js";

	/**
	 * One conflicted path, resolved over the three versions git kept in the index.
	 *
	 * The base is context — collapsed, because the decision is between ours and
	 * theirs — while ours and theirs are the panes that stay in view. Taking a
	 * side fills the editor with that side's text, and the editor is the only
	 * thing `git_resolve` ever writes: a merge is a text edit here, exactly as it
	 * would be in an editor, and the ledger's loader still has the last word on
	 * whether the text is a legal journal.
	 *
	 * A side can be missing (`null`): the file did not exist in that index stage,
	 * which is what a delete on one branch of the merge looks like. `git_resolve`
	 * writes text and stages it — it has no way to stage a deletion — so the
	 * missing side's button is disabled rather than silently writing an empty
	 * file. A path outside the journal's own files cannot be written at all, and
	 * its refusal is reported verbatim.
	 *
	 * At 390px the two panes stack; each is a scrolling box, so a long journal
	 * file never pushes the buttons off the screen.
	 */
	let {
		path,
		applyPath,
		oursLabel = "this branch",
		theirsLabel = "the merged branch",
		onapplied,
		onclose,
	}: {
		/** The repository-relative path, as the merge named it — what the row shows. */
		path: string;
		/** The path handed to the commands: the journal file's absolute path when it is one. */
		applyPath: string;
		/** The branch being merged into, for the pane labels. */
		oursLabel?: string;
		/** The branch merged in, for the pane labels. */
		theirsLabel?: string;
		/** Called with the sentence to show once the text is staged; awaited before the row settles. */
		onapplied: (message: string) => Promise<void>;
		/** Collapse the panel without writing anything. */
		onclose: () => void;
	} = $props();

	let texts = $state<ConflictTexts | null>(null);
	let loading = $state(true);
	let loadError = $state("");
	let baseOpen = $state(false);
	/** The text `Apply & stage` would write. */
	let draft = $state("");
	/** Whether a text has been picked at all — an emptied editor is still a choice. */
	let chosen = $state(false);
	/** What `draft` is, for the label above the editor. */
	let draftLabel = $state("not chosen yet");
	let editing = $state(false);
	let applying = $state(false);
	let applyError = $state("");
	let editor = $state<HTMLElement | null>(null);

	const bothMissing = $derived(texts !== null && texts.ours === null && texts.theirs === null);
	const oneSideMissing = $derived(
		texts !== null && (texts.ours === null) !== (texts.theirs === null)
	);
	const applyDisabled = $derived(applying || bothMissing || (!chosen && !editing));

	async function load() {
		loading = true;
		loadError = "";
		try {
			const result = await commands.gitConflictTexts(applyPath);
			if (result.status === "error") {
				loadError = result.error;
				return;
			}
			texts = result.data;
		} catch (cause) {
			console.error("gitConflictTexts failed", cause);
			loadError = messageOf(cause);
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		void load();
	});

	/** Fill the editor with one side's text; that side is a complete resolution. */
	function take(side: "ours" | "theirs") {
		if (!texts) return;
		const text = texts[side];
		if (text === null) return;
		draft = text;
		draftLabel = side === "ours" ? `our version (${oursLabel})` : `their version (${theirsLabel})`;
		chosen = true;
		editing = true;
		applyError = "";
		void tick().then(() => editor?.focus());
	}

	/**
	 * Open the editor for a hand merge. A fresh editor starts from our side, as
	 * the per-file resolution a person would do in an editor does; when our side
	 * has no file, the merged branch's text is the only content there is, and
	 * starting from an empty box would quietly propose an empty file.
	 */
	function edit() {
		if (!chosen && texts) {
			if (texts.ours !== null) {
				draft = texts.ours;
				draftLabel = `our version (${oursLabel}), to merge by hand`;
			} else if (texts.theirs !== null) {
				draft = texts.theirs;
				draftLabel = `their version (${theirsLabel}), to merge by hand`;
			} else {
				draft = "";
				draftLabel = "empty";
			}
			chosen = true;
		}
		editing = true;
		applyError = "";
		void tick().then(() => editor?.focus());
	}

	async function apply() {
		applying = true;
		applyError = "";
		try {
			const result = await commands.gitResolve(applyPath, draft);
			if (result.status === "error") {
				// A loader refusal (the merged text is not a legal journal) or a
				// file that is outside the journal arrives here, word for word.
				applyError = result.error;
				return;
			}
			await onapplied(`Staged the resolution for ${path}`);
		} catch (cause) {
			console.error("gitResolve failed", cause);
			applyError = messageOf(cause);
		} finally {
			applying = false;
		}
	}
</script>

<div class="border-t px-3 py-2.5">
	{#if loading}
		<p class="text-muted-foreground text-xs">Reading the three versions…</p>
	{:else if loadError}
		<div class="flex flex-col gap-2">
			<Alert variant="destructive">{loadError}</Alert>
			<div class="flex flex-wrap items-center gap-1">
				<Button size="sm" variant="outline" onclick={() => void load()}>Try again</Button>
				<Button size="sm" variant="ghost" onclick={onclose}>Close</Button>
			</div>
		</div>
	{:else if texts}
		<div class="flex flex-col gap-2">
			<p class="text-muted-foreground font-mono text-[0.625rem] break-all" title={applyPath}>
				{path}
			</p>

			{#if texts.base === null}
				<p class="text-muted-foreground text-[0.625rem]">
					No merge base — the file did not exist in the common ancestor, so both sides are
					additions.
				</p>
			{:else}
				<Collapsible.Root bind:open={baseOpen}>
					<Collapsible.Trigger
						class="text-muted-foreground hover:text-foreground flex items-center gap-1 text-[0.625rem] font-medium tracking-wide uppercase"
					>
						Base · {baseOpen ? "hide" : "show"}
					</Collapsible.Trigger>
					<Collapsible.Content>
						<pre
							class="bg-muted/40 mt-1 max-h-48 overflow-auto rounded-sm border p-2 font-mono text-[0.6875rem] leading-relaxed whitespace-pre">{texts.base}</pre>
					</Collapsible.Content>
				</Collapsible.Root>
			{/if}

			<!-- Stacked below md: at 390px two columns would be two unreadable slivers. -->
			<div class="grid gap-2 md:grid-cols-2">
				<div
					class="border-primary/40 bg-card ring-primary/20 flex min-w-0 flex-col gap-1 rounded-md border p-2 ring-1"
				>
					<span class="text-[0.625rem] font-medium tracking-wide uppercase">
						ours · {oursLabel}
					</span>
					{#if texts.ours === null}
						<p class="text-muted-foreground text-xs italic">deleted on this branch</p>
					{:else}
						<pre
							class="bg-muted/20 max-h-64 overflow-auto rounded-sm border p-2 font-mono text-[0.6875rem] leading-relaxed whitespace-pre">{texts.ours}</pre>
					{/if}
				</div>
				<div
					class="border-primary/40 bg-card ring-primary/20 flex min-w-0 flex-col gap-1 rounded-md border p-2 ring-1"
				>
					<span class="text-[0.625rem] font-medium tracking-wide uppercase">
						theirs · {theirsLabel}
					</span>
					{#if texts.theirs === null}
						<p class="text-muted-foreground text-xs italic">deleted on the merged branch</p>
					{:else}
						<pre
							class="bg-muted/20 max-h-64 overflow-auto rounded-sm border p-2 font-mono text-[0.6875rem] leading-relaxed whitespace-pre">{texts.theirs}</pre>
					{/if}
				</div>
			</div>

			{#if bothMissing}
				<p class="text-muted-foreground text-xs">
					Both sides deleted this file. The resolver stages text and cannot stage a deletion,
					so there is nothing it can write here — Abort merge keeps the deletion, or resolve
					this path outside the app.
				</p>
			{:else if oneSideMissing}
				<p class="text-muted-foreground text-xs">
					One side has no file here (a delete on that branch). Taking a missing side would
					have to stage a deletion, which this resolver cannot do — that button stays
					disabled. Keeping the file means taking the side that has it; keeping the deletion
					means Abort merge.
				</p>
			{/if}

			<div class="flex flex-wrap items-center gap-2">
				<Button
					size="sm"
					variant="outline"
					disabled={texts.ours === null}
					title={texts.ours === null
						? "This branch deleted the file; the resolver stages text, not a deletion"
						: "Use this branch's whole version"}
					onclick={() => take("ours")}
				>
					Take ours
				</Button>
				<Button
					size="sm"
					variant="outline"
					disabled={texts.theirs === null}
					title={texts.theirs === null
						? "The merged branch deleted the file; the resolver stages text, not a deletion"
						: "Use the merged branch's whole version"}
					onclick={() => take("theirs")}
				>
					Take theirs
				</Button>
				<Button size="sm" variant="ghost" onclick={edit}>Edit merge…</Button>
				<span class="text-muted-foreground ms-auto text-[0.625rem]">{draftLabel}</span>
			</div>

			{#if editing}
				<label class="flex min-w-0 flex-col gap-1">
					<span
						class="text-muted-foreground text-[0.625rem] font-medium tracking-wide uppercase"
					>
						Text to stage · {draftLabel}
					</span>
					<Textarea
						bind:ref={editor}
						bind:value={draft}
						class="max-h-80 min-h-40 resize-y overflow-auto font-mono text-[0.6875rem]"
						spellcheck={false}
						autocapitalize="off"
						autocomplete="off"
						rows={10}
						aria-label={`Merged text for ${path}`}
						oninput={() => (draftLabel = "edited by hand")}
					/>
				</label>
				<div class="flex flex-wrap items-center gap-2">
					<Button size="sm" disabled={applyDisabled} onclick={() => void apply()}>
						{applying ? "Staging…" : "Apply & stage"}
					</Button>
					<Button size="sm" variant="ghost" disabled={applying} onclick={onclose}>Close</Button>
					<span class="text-muted-foreground text-[0.625rem]">
						{draft === "" ? "empty file" : `${draft.split("\n").length} lines`}
					</span>
				</div>
			{:else}
				<div class="flex items-center gap-1">
					<Button size="sm" variant="ghost" onclick={onclose}>Close</Button>
				</div>
			{/if}

			{#if applyError}
				<Alert variant="destructive">{applyError}</Alert>
			{/if}
		</div>
	{/if}
</div>
