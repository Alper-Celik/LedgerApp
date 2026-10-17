<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { Change } from "../../bindings";
	import { cn } from "$lib/utils.js";

	/**
	 * How something stands against the ledger's last commit, said the way a git
	 * client's file list says it: one letter — `A` for what the commit does not
	 * have, `M` for what it has and has been rewritten — coloured, and nothing at
	 * all when there is nothing to say.
	 *
	 * The letter is only as good as what it is about: an entry's is about the
	 * entry's own lines, a file's about the file, an account's about the strongest
	 * change anywhere under it. A ledger git does not track reads as unchanged
	 * until it is written to — not being committed is not a change in itself.
	 */
	let { change, class: className }: { change: Change; class?: string } = $props();

	/** The two kinds that have a letter, and what the letter means. */
	const SAYS = {
		added: { letter: "A", title: "New since the last commit" },
		modified: { letter: "M", title: "Changed since the last commit" },
	} as const;
</script>

{#if change !== "unchanged"}
	{@const says = SAYS[change]}
	<span
		data-slot="git-status"
		role="img"
		aria-label={says.title}
		title={says.title}
		class={cn(
			"inline-flex w-3 shrink-0 justify-center font-mono text-[0.6875rem] leading-none font-semibold",
			// Green is the ledger's own "added"; modified takes the git client's
			// amber, which no token carries because nothing else in the app needs it.
			change === "added" ? "text-added" : "text-amber-600 dark:text-amber-400",
			className
		)}>{says.letter}</span
	>
{/if}
