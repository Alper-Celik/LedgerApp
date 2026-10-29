<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { FileEditIcon, PencilEdit02Icon } from "@hugeicons/core-free-icons";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import type { Proposal } from "$lib/assistant/tools.js";

	let {
		proposal,
		status,
		error,
		disabled = false,
		onapply,
		onreject,
	}: {
		proposal: Proposal;
		status: "pending" | "applied" | "failed" | "expired";
		error: string;
		disabled?: boolean;
		onapply: () => void;
		onreject: () => void;
	} = $props();

	/** One line of the file diff, or the `…` that stands for skipped context. */
	type DiffLine = {
		kind: "context" | "added" | "removed" | "gap";
		text: string;
		number: number | null;
	};

	/** How many unchanged lines are kept around each change. */
	const CONTEXT = 3;
	/** The most diff lines the card draws; the rest is a one-line note. */
	const DIFF_LIMIT = 60;
	/** Past this many line pairs, a file is not diffed: the card says its size. */
	const DIFF_MAX_CELLS = 1_000_000;

	/** The transaction's own header line, as beancount reads it. */
	const header = $derived.by(() => {
		const entry = proposal.entry;
		if (!entry) return "";
		const parts = [entry.date, entry.flag];
		if (entry.payee) parts.push(JSON.stringify(entry.payee));
		parts.push(JSON.stringify(entry.narration));
		return parts.join(" ");
	});

	/** A whole-file write, diffed line by line; `null` when too large to diff. */
	const diff = $derived.by(() => {
		if (proposal.kind !== "file" || proposal.before === null || proposal.after === null) {
			return null;
		}
		return diffLines(proposal.before, proposal.after);
	});

	/** The one-line header summary: what kind of change, and its size. */
	const summary = $derived(
		proposal.kind === "transaction"
			? `${proposal.entry?.postings.length ?? 0} postings`
			: `${(proposal.after ?? "").split("\n").length} lines`
	);

	/**
	 * A line diff of two texts, shown as hunks with context.
	 *
	 * A longest-common-subsequence walk, not a line-by-line comparison: two
	 * edits far apart must not read as one huge rewrite. The walk is bounded —
	 * a file large enough to make the table expensive is reported by size
	 * instead of guessed at — and only the hunks are drawn.
	 */
	function diffLines(before: string, after: string): { lines: DiffLine[]; truncated: boolean } | null {
		const old = before.split("\n");
		const current = after.split("\n");
		if (old.length * current.length > DIFF_MAX_CELLS) return null;

		const width = current.length + 1;
		const lcs = new Uint32Array((old.length + 1) * width);
		for (let i = old.length - 1; i >= 0; i--) {
			for (let j = current.length - 1; j >= 0; j--) {
				lcs[i * width + j] =
					old[i] === current[j]
						? lcs[(i + 1) * width + j + 1] + 1
						: Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
			}
		}

		const ops: DiffLine[] = [];
		let i = 0;
		let j = 0;
		while (i < old.length && j < current.length) {
			if (old[i] === current[j]) {
				ops.push({ kind: "context", text: old[i], number: i + 1 });
				i += 1;
				j += 1;
			} else if (lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
				ops.push({ kind: "removed", text: old[i], number: i + 1 });
				i += 1;
			} else {
				ops.push({ kind: "added", text: current[j], number: j + 1 });
				j += 1;
			}
		}
		while (i < old.length) {
			ops.push({ kind: "removed", text: old[i], number: i + 1 });
			i += 1;
		}
		while (j < current.length) {
			ops.push({ kind: "added", text: current[j], number: j + 1 });
			j += 1;
		}

		// The indices worth drawing: every change, plus the context around it.
		const keep = new Array<boolean>(ops.length).fill(false);
		ops.forEach((op, index) => {
			if (op.kind === "context") return;
			const from = Math.max(0, index - CONTEXT);
			const to = Math.min(ops.length - 1, index + CONTEXT);
			for (let at = from; at <= to; at += 1) keep[at] = true;
		});
		const wanted: number[] = [];
		keep.forEach((kept, index) => {
			if (kept) wanted.push(index);
		});
		const drawn = wanted.slice(0, DIFF_LIMIT);

		const lines: DiffLine[] = [];
		let previous = -1;
		for (const index of drawn) {
			if (previous >= 0 && index - previous > 1) {
				lines.push({ kind: "gap", text: "…", number: null });
			}
			lines.push(ops[index]);
			previous = index;
		}
		return { lines, truncated: wanted.length > drawn.length };
	}

	/** The class a diff line draws in. */
	function diffClass(kind: DiffLine["kind"]): string {
		if (kind === "added") return "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400";
		if (kind === "removed") return "bg-destructive/10 text-destructive";
		if (kind === "gap") return "text-muted-foreground/60";
		return "text-muted-foreground";
	}

	/** The one-character sign a diff line starts with. */
	function diffSign(kind: DiffLine["kind"]): string {
		if (kind === "added") return "+";
		if (kind === "removed") return "-";
		return " ";
	}
</script>

<section class="bg-card/60 w-full min-w-0 rounded-lg border">
	<div class="flex items-center gap-2 px-2 py-1.5 text-xs">
		<span
			class="bg-muted text-muted-foreground flex size-5 shrink-0 items-center justify-center rounded-md"
		>
			<HugeiconsIcon
				icon={proposal.kind === "transaction" ? PencilEdit02Icon : FileEditIcon}
				strokeWidth={2}
				class="size-3"
			/>
		</span>
		<span class="min-w-0 flex-1 truncate">
			proposed <code class="font-mono text-[0.6875rem]">{proposal.tool}</code>
			<span class="text-muted-foreground">→ {summary}</span>
		</span>
		{#if status === "applied"}
			<span class="shrink-0 text-emerald-600 dark:text-emerald-400">written</span>
		{:else if status === "expired"}
			<span class="text-muted-foreground shrink-0">earlier conversation</span>
		{:else}
			<span class="text-muted-foreground shrink-0">not applied</span>
		{/if}
	</div>

	<div class="space-y-2 border-t p-2">
		<div class="text-muted-foreground flex flex-wrap items-center gap-1 text-[0.6875rem]">
			<span>target</span>
			<code class="text-foreground font-mono">{proposal.file}</code>
		</div>

		{#if proposal.kind === "transaction" && proposal.entry}
			{@const entry = proposal.entry}
			<div class="max-h-64 overflow-auto rounded-md border">
				<div class="bg-muted/40 border-b px-2 py-1 font-mono text-[0.6875rem] break-words">
					{header}
				</div>
				<table class="w-full border-collapse text-left font-mono text-[0.6875rem]">
					<thead class="bg-muted sticky top-0">
						<tr>
							<th scope="col" class="px-1.5 py-1 font-medium whitespace-nowrap">account</th>
							<th scope="col" class="px-1.5 py-1 text-right font-medium whitespace-nowrap">units</th>
							<th scope="col" class="px-1.5 py-1 font-medium whitespace-nowrap">currency</th>
						</tr>
					</thead>
					<tbody>
						{#each entry.postings as posting, index (index)}
							<tr class="border-t">
								<td class="px-1.5 py-0.5 break-all">{posting.account}</td>
								<td class="px-1.5 py-0.5 text-right whitespace-nowrap">
									{#if posting.units === null}
										<span class="text-muted-foreground">interpolated</span>
									{:else}
										{posting.units}
									{/if}
								</td>
								<td class="px-1.5 py-0.5 whitespace-nowrap">
									{#if posting.currency === null}
										<span class="text-muted-foreground">operating</span>
									{:else}
										{posting.currency}
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{:else if proposal.kind === "file"}
			{#if diff === null}
				<p class="text-muted-foreground text-xs">
					This file is too large to show its diff here; it has
					{(proposal.before ?? "").split("\n").length} lines now and would have
					{(proposal.after ?? "").split("\n").length}.
				</p>
			{:else}
				<div class="max-h-64 overflow-auto rounded-md border font-mono text-[0.625rem] leading-relaxed">
					{#each diff.lines as line, index (index)}
						<div class="{diffClass(line.kind)} flex gap-2 px-1.5 py-0.5 whitespace-pre-wrap">
							<span class="text-muted-foreground/60 w-6 shrink-0 text-right select-none">
								{line.number ?? ""}
							</span>
							<span class="w-2 shrink-0 select-none">{diffSign(line.kind)}</span>
							<span class="min-w-0 break-words">{line.text}</span>
						</div>
					{/each}
				</div>
				{#if diff.truncated}
					<p class="text-muted-foreground text-[0.6875rem]">
						The diff was cut short; only the first {DIFF_LIMIT} changed lines are shown.
					</p>
				{/if}
			{/if}
		{/if}

		{#if status === "failed" && error}
			<Alert variant="destructive">{error}</Alert>
		{:else if status === "expired"}
			<p class="text-muted-foreground text-xs">
				From an earlier conversation, so it is not applied; ask again to stage it fresh.
			</p>
		{:else if status === "applied"}
			<p class="text-xs">Applied. The ledger has been reloaded.</p>
		{/if}

		{#if status === "pending" || status === "failed"}
			<div class="flex items-center gap-2">
				<Button size="sm" onclick={onapply} disabled={disabled}>
					{status === "failed" ? "Try again" : "Apply"}
				</Button>
				<Button size="sm" variant="ghost" onclick={onreject} disabled={disabled}>Reject</Button>
				<span class="text-muted-foreground text-[0.6875rem]">
					Nothing is written until you apply this.
				</span>
			</div>
		{/if}
	</div>
</section>
