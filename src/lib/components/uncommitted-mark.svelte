<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { cn } from "$lib/utils.js";

	/**
	 * The uncommitted marker, in one place so the sidebar's account tree and the
	 * register's rows say the same thing the same way: a small dot in the diff's
	 * `added` tone, with the word for it carried in the accessible name and the
	 * tooltip rather than on the row.
	 *
	 * Nothing is rendered when `modified` is false — callers place it
	 * unconditionally and let the component decide.
	 */
	let { modified, class: className }: { modified: boolean; class?: string } = $props();

	const UNCOMMITTED = "Uncommitted";
</script>

{#if modified}
	<!--
		`shrink-0` keeps the dot round inside truncating rows; `size-1.5` is the
		smallest mark that still reads at this text size.
	-->
	<span
		data-slot="uncommitted-mark"
		class={cn("bg-added inline-block size-1.5 shrink-0 rounded-full", className)}
		title={UNCOMMITTED}
	>
		<span class="sr-only">{UNCOMMITTED}</span>
	</span>
{/if}
