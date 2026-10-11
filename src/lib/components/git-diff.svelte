<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
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
	const tone = {
		file: "text-foreground font-medium bg-foreground/5",
		hunk: "text-muted-foreground",
		added: "text-emerald-700 bg-emerald-500/10 dark:text-emerald-400",
		removed: "text-red-700 bg-red-500/10 dark:text-red-400",
		context: "text-muted-foreground/80",
	} as const;
</script>

<pre class="overflow-x-auto rounded-md border py-2 font-mono text-xs leading-relaxed">{#each lines as line, index (index)}<span
			class="block px-3 whitespace-pre {tone[line.kind]}">{line.text}</span
		>{/each}</pre>
