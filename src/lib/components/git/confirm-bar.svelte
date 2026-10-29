<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";

	/**
	 * The step between a button and a write: the question, and the two answers.
	 *
	 * Every action on the branches screen that changes the repository or the
	 * working tree — switching, creating, merging, reverting, aborting a merge —
	 * goes through this bar first, so nothing is written by a single click. The
	 * confirm button carries the verb the action will perform, not "OK", because
	 * the last thing read before a write should name the write.
	 */
	let {
		question,
		confirm,
		cancel = "Cancel",
		destructive = false,
		busy = false,
		busyLabel = "Working…",
		onconfirm,
		oncancel,
	}: {
		/** What is about to happen, in one sentence. */
		question: string;
		/** The confirming button's label — the action's own verb. */
		confirm: string;
		cancel?: string;
		/** Red confirm button for an action that throws work away. */
		destructive?: boolean;
		busy?: boolean;
		busyLabel?: string;
		onconfirm: () => void;
		oncancel: () => void;
	} = $props();
</script>

<div
	class="bg-muted/40 flex flex-wrap items-center gap-x-2 gap-y-1.5 rounded-md border px-2 py-1.5"
	role="group"
	aria-label="Confirmation"
>
	<span class="min-w-0 flex-1 text-xs">{question}</span>
	<div class="flex shrink-0 items-center gap-1">
		<Button
			size="sm"
			variant={destructive ? "destructive" : "default"}
			disabled={busy}
			onclick={onconfirm}
		>
			{busy ? busyLabel : confirm}
		</Button>
		<Button size="sm" variant="ghost" disabled={busy} onclick={oncancel}>
			{cancel}
		</Button>
	</div>
</div>
