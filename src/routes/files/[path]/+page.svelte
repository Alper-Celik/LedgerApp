<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import FileView from "$lib/components/file-view.svelte";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();
</script>

<div class="flex flex-col gap-4 p-4">
	<!--
		One page per file: the route pattern is the same for all of them, so the
		view is keyed by the path — a different file is a different view, and the
		text editor must not carry one file's text into another.
	-->
	{#key data.path}
		<FileView
			path={data.path}
			file={data.file}
			transactions={data.transactions}
			error={data.error}
		/>
	{/key}
</div>
