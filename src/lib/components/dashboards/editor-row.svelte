<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";
	import {
		definitionFor,
		WIDGET_SPANS,
		type WidgetSpan,
	} from "$lib/dashboard/index.js";
	import { isWidgetKind, type DraftWidget } from "./data.js";

	/**
	 * One widget's row in the editor: what it is, how wide it is, where it sits,
	 * and the way to remove it.
	 *
	 * The row is a stack of two bands on a phone and one line on a desk; every
	 * control keeps a 32px target either way. The callbacks carry the index
	 * rather than a reference, because the page owns the array and reorders it.
	 */
	let {
		widget,
		index,
		count,
		onmove,
		onremove,
		onspan,
	}: {
		widget: DraftWidget;
		index: number;
		count: number;
		onmove: (index: number, delta: -1 | 1) => void;
		onremove: (index: number) => void;
		onspan: (index: number, span: WidgetSpan) => void;
	} = $props();

	const definition = $derived(isWidgetKind(widget.kind) ? definitionFor(widget.kind) : null);
	const title = $derived(definition ? definition.title : widget.kind);
	const description = $derived(
		definition ? definition.description : "This build has no definition for this widget kind."
	);
</script>

<div
	class="flex flex-col gap-2 rounded-lg border border-border bg-card p-2 sm:flex-row sm:items-center"
>
	<div class="min-w-0 flex-1">
		<p class="truncate text-sm font-medium">{title}</p>
		<p class="text-muted-foreground line-clamp-2 text-xs">{description}</p>
	</div>

	<div class="flex flex-wrap items-center gap-1">
		<span class="text-muted-foreground mr-1 text-xs">Span</span>
		{#each WIDGET_SPANS as span (span)}
			<Button
				variant={widget.span === span ? "secondary" : "outline"}
				size="lg"
				class="h-8 min-w-8 px-2"
				aria-label={`Span ${span} of 4 columns`}
				aria-pressed={widget.span === span}
				onclick={() => onspan(index, span)}
			>
				{span}
			</Button>
		{/each}

		<span class="mx-1 h-4 w-px bg-border" aria-hidden="true"></span>

		<Button
			variant="ghost"
			size="icon-lg"
			class="size-8"
			aria-label="Move up"
			disabled={index === 0}
			onclick={() => onmove(index, -1)}
		>
			↑
		</Button>
		<Button
			variant="ghost"
			size="icon-lg"
			class="size-8"
			aria-label="Move down"
			disabled={index === count - 1}
			onclick={() => onmove(index, 1)}
		>
			↓
		</Button>
		<Button
			variant="destructive"
			size="icon-lg"
			class="size-8"
			aria-label={`Remove ${title}`}
			onclick={() => onremove(index)}
		>
			×
		</Button>
	</div>
</div>
