<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import FileTree from "$lib/components/file-tree.svelte";
	import type { PageProps } from "./$types";

	/**
	 * The journal's files, full width: the root the session was opened with and
	 * every `include` it pulls in, by the directory tree they live in. The
	 * sidebar carries the same tree at its own narrow scale; this screen is for
	 * reading a deep path without the sidebar's width deciding where a name is
	 * cut off.
	 */
	let { data }: PageProps = $props();
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Journal files</h1>
		<span class="text-muted-foreground text-xs">
			Every file the journal loads — its root and the files it includes — with what each writes
			and how it stands in git.
		</span>
	</div>

	{#if data.error}
		<Alert variant="destructive">{data.error}</Alert>
	{:else if data.noLedger}
		<div class="flex max-w-2xl flex-col gap-2 rounded-lg border px-3 py-4">
			<p class="text-sm font-medium">No ledger is open</p>
			<p class="text-muted-foreground text-xs">
				The journal's files are the files it loads, and no journal is loaded yet. Open a ledger
				folder, then come back.
			</p>
			<div>
				<Button size="sm" variant="secondary" href={resolve("/settings")}>Open Settings</Button>
			</div>
		</div>
	{:else if data.files.length === 0}
		<div class="flex max-w-2xl flex-col gap-2 rounded-lg border px-3 py-4">
			<p class="text-sm font-medium">This journal loads no files</p>
			<p class="text-muted-foreground text-xs">
				The listing came back empty, so there is no root or include to show.
			</p>
		</div>
	{:else}
		<!--
			The tree is the sidebar's, at content scale: same rows, same counts,
			same git letters, and the same default of every folder open.
		-->
		<div class="w-full rounded-lg border p-2">
			<FileTree files={data.files} />
		</div>
	{/if}
</div>
