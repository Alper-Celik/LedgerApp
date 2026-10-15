<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { cn } from "$lib/utils.js";

	/** Renders a unified diff the way a diff tool colours one. */
	let { diff }: { diff: string } = $props();

	type Kind = "file" | "hunk" | "added" | "removed" | "context";
	let lines = $derived(
		diff.split("\n").map((text) => {
			const kind: Kind = /^diff --git|^index |^new file|^deleted file|^--- |^\+\+\+ /.test(text)
				? "file"
				: text.startsWith("@@")
					? "hunk"
					: text.startsWith("+")
						? "added"
						: text.startsWith("-")
							? "removed"
							: "context";
			return { text, kind };
		})
	);
	const tone: Record<Kind, string> = {
		file: "text-foreground bg-muted font-medium",
		hunk: "text-muted-foreground",
		added: "text-added bg-added/10",
		removed: "text-removed bg-removed/10",
		context: "text-muted-foreground",
	};
</script>

<pre class="overflow-x-auto rounded-md border py-2 font-mono text-xs leading-relaxed">{#each lines as line, index (index)}<span
			class={cn("block px-3 whitespace-pre", tone[line.kind])}>{line.text}</span
		>{/each}</pre>
