<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { SparklesIcon } from "@hugeicons/core-free-icons";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import ToolCard from "./tool-card.svelte";
	import type { Assistant } from "$lib/assistant/store.svelte.js";

	let { assistant }: { assistant: Assistant } = $props();

	let scroller = $state<HTMLElement | null>(null);
	/** Whether the transcript is close enough to its end to follow new output. */
	let stick = true;

	function onScroll() {
		if (!scroller) return;
		stick = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
	}

	// Svelte effects run after the DOM update, so the new row is already there
	// when this measures. Following only when the reader was at the bottom keeps
	// scrolling up to read an earlier answer from being yanked back down.
	$effect(() => {
		const count = assistant.entries.length;
		const busy = assistant.busy;
		const failure = assistant.chatError;
		const saveFailure = assistant.saveError;
		const note = assistant.notice;
		void count;
		void busy;
		void failure;
		void saveFailure;
		void note;
		if (!stick || !scroller) return;
		const node = scroller;
		requestAnimationFrame(() => {
			if (node.isConnected) node.scrollTop = node.scrollHeight;
		});
	});
</script>

<div
	bind:this={scroller}
	onscroll={onScroll}
	class="min-h-0 flex-1 overflow-y-auto overscroll-contain scroll-smooth"
>
	<div class="mx-auto flex w-full max-w-3xl flex-col gap-2.5 px-3 py-3">
		{#if assistant.entries.length === 0 && !assistant.busy}
			<div class="text-muted-foreground flex flex-col items-center gap-2 py-10 text-center">
				<span class="bg-muted flex size-9 items-center justify-center rounded-full">
					<HugeiconsIcon icon={SparklesIcon} strokeWidth={2} class="size-4" />
				</span>
				<p class="text-xs">
					Ask about the ledger — balances, a month's spending, a file's text. Answers are read
					from the ledger through tools, never guessed.
				</p>
			</div>
		{/if}

		{#each assistant.entries as entry, index (`${assistant.activeId || "new"}:${index}`)}
			{#if entry.kind === "user"}
				<div class="flex justify-end">
					<div
						class="bg-primary text-primary-foreground max-w-[85%] rounded-2xl rounded-br-sm px-3 py-1.5 text-sm break-words whitespace-pre-wrap"
					>
						{entry.text}
					</div>
				</div>
			{:else if entry.kind === "assistant"}
				{#if entry.text.trim()}
					<div
						class="bg-muted max-w-[92%] self-start rounded-2xl rounded-bl-sm px-3 py-1.5 text-sm break-words whitespace-pre-wrap"
					>
						{entry.text}
					</div>
				{/if}
			{:else}
				<ToolCard name={entry.name} result={entry.result} text={entry.text} ok={entry.ok} />
			{/if}
		{/each}

		{#if assistant.busy}
			<div class="text-muted-foreground flex items-center gap-2 px-1 text-xs">
				<span class="size-3 animate-spin rounded-full border-2 border-current border-t-transparent"
				></span>
				{assistant.busyNote || "Thinking…"}
			</div>
		{/if}

		{#if assistant.chatError}
			<Alert variant="destructive">{assistant.chatError}</Alert>
		{/if}
		{#if assistant.saveError}
			<Alert variant="destructive">
				<span class="font-medium">This turn was not saved to the ledger:</span>
				{assistant.saveError}
			</Alert>
		{/if}
		{#if assistant.notice}
			<Alert>{assistant.notice}</Alert>
		{/if}
	</div>
</div>
