<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { PythonIcon, SparklesIcon } from "@hugeicons/core-free-icons";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { pythonRunOf, type PythonRun } from "$lib/assistant/artifacts/types.js";
	import { isRunPython } from "$lib/assistant/tools.js";
	import ChartArtifact from "./chart-artifact.svelte";
	import ToolCard from "./tool-card.svelte";
	import type { Assistant } from "$lib/assistant/store.svelte.js";

	let { assistant }: { assistant: Assistant } = $props();

	/** What a run's one-line header says: tables, a chart, or an error. */
	function pythonSummary(run: PythonRun): string {
		const parts: string[] = [];
		if (run.error) parts.push("error");
		if (run.tables.length === 1) parts.push("1 table");
		else if (run.tables.length > 1) parts.push(`${run.tables.length} tables`);
		if (run.chart) parts.push("1 chart");
		if (parts.length === 0) parts.push(run.stdout.trim() ? "text output" : "no output");
		return parts.join(", ");
	}

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
		// Text that arrives a fragment at a time does not change the row count,
		// so the store bumps a tick for each one and the transcript follows it.
		const streamed = assistant.streamTick;
		void count;
		void busy;
		void failure;
		void saveFailure;
		void note;
		void streamed;
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
			{:else if isRunPython(entry.name)}
				{@const run = pythonRunOf(entry.result)}
				<div class="bg-card/60 w-full min-w-0 rounded-lg border">
					<div class="flex items-center gap-2 px-2 py-1.5 text-xs">
						<span
							class="bg-muted text-muted-foreground flex size-5 shrink-0 items-center justify-center rounded-md"
						>
							<HugeiconsIcon icon={PythonIcon} strokeWidth={2} class="size-3" />
						</span>
						<span class="min-w-0 flex-1 truncate">
							ran <code class="font-mono text-[0.6875rem]">Python</code>
							{#if run}
								<span class="text-muted-foreground">→ {pythonSummary(run)}</span>
							{/if}
						</span>
					</div>

					<div class="space-y-2 border-t p-2">
						{#if run === null}
							<pre
								class="text-destructive max-h-64 overflow-auto font-mono text-[0.625rem] leading-relaxed break-words whitespace-pre-wrap">{entry.text}</pre>
						{:else}
							{#if run.error}
								<pre
									class="text-destructive max-h-64 overflow-auto font-mono text-[0.625rem] leading-relaxed break-words whitespace-pre-wrap">{run.error}</pre>
							{/if}
							{#if run.stdout.trim()}
								<pre
									class="bg-muted/40 max-h-64 overflow-auto rounded-md border p-1.5 font-mono text-[0.625rem] leading-relaxed break-words whitespace-pre-wrap">{run.stdout}</pre>
							{/if}

							{#each run.tables as table, index (index)}
								<div class="min-w-0">
									<div
										class="text-muted-foreground mb-1 flex items-center justify-between gap-2 text-[0.6875rem]"
									>
										<span class="truncate">{table.name}</span>
										{#if table.truncated}
											<span class="shrink-0">truncated</span>
										{/if}
									</div>
									<div class="max-h-56 overflow-auto rounded-md border">
										<table class="w-full border-collapse text-left font-mono text-[0.6875rem]">
											<thead class="bg-muted sticky top-0">
												<tr>
													{#each table.columns as column}
														<th scope="col" class="px-1.5 py-1 font-medium whitespace-nowrap">
															{column}
														</th>
													{/each}
												</tr>
											</thead>
											<tbody>
												{#each table.rows as row, rowIndex (rowIndex)}
													<tr class="border-t">
														{#each row as cell}
															<td class="px-1.5 py-0.5 whitespace-nowrap">{cell}</td>
														{/each}
													</tr>
												{/each}
											</tbody>
										</table>
									</div>
								</div>
							{/each}

							{#if run.chart}
								<ChartArtifact spec={run.chart} />
							{/if}

							{#if !run.error && !run.stdout.trim() && run.tables.length === 0 && !run.chart}
								<p class="text-muted-foreground text-xs">Ran with no output.</p>
							{/if}
						{/if}
					</div>
				</div>
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
