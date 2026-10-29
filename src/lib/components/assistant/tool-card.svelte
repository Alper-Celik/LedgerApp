<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { ArrowRight01Icon, Database01Icon } from "@hugeicons/core-free-icons";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";
	import { describeResult } from "$lib/assistant/tools.js";

	let {
		name,
		result,
		text,
		ok,
	}: {
		name: string;
		result: unknown;
		text: string;
		ok: boolean;
	} = $props();

	/** “12 rows”, or “error” — what the one-line card says after the arrow. */
	const summary = $derived(ok ? describeResult(result) : "error");

	/** The full answer, pretty-printed when it is structured. */
	const detail = $derived.by(() => {
		if (!ok) {
			if (result && typeof result === "object" && "error" in result) {
				return String((result as { error: unknown }).error);
			}
			return text;
		}
		if (result === null || result === undefined) return text;
		try {
			return JSON.stringify(result, null, 2) ?? text;
		} catch {
			return text;
		}
	});
</script>

<Collapsible.Root class="bg-card/60 w-full rounded-lg border">
	<Collapsible.Trigger
		class="group hover:bg-muted/50 flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs transition-colors"
	>
		<span
			class="flex size-5 shrink-0 items-center justify-center rounded-md {ok
				? 'bg-muted text-muted-foreground'
				: 'bg-destructive/10 text-destructive'}"
		>
			<HugeiconsIcon icon={Database01Icon} strokeWidth={2} class="size-3" />
		</span>
		<span class="min-w-0 flex-1 truncate">
			called <code class="font-mono text-[0.6875rem]">{name}</code>
			<span class="text-muted-foreground">→ {summary}</span>
		</span>
		<HugeiconsIcon
			icon={ArrowRight01Icon}
			strokeWidth={2}
			class="text-muted-foreground size-3 transition-transform group-data-[state=open]:rotate-90"
		/>
	</Collapsible.Trigger>
	<Collapsible.Content>
		<pre
			class="max-h-64 overflow-auto border-t px-2 py-1.5 font-mono text-[0.625rem] leading-relaxed break-words whitespace-pre-wrap">{detail}</pre>
	</Collapsible.Content>
</Collapsible.Root>
