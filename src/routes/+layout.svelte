<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

<script lang="ts">
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import AppSidebar from "$lib/components/app-sidebar.svelte";
	import { sidebarWidth } from "$lib/sidebar-width.svelte.js";
	import "../app.css";
	import { ModeWatcher } from "mode-watcher";

	let { children } = $props();
</script>

<!-- The passed style lands after the component's default `--sidebar-width`, so it wins. -->
<Sidebar.Provider style="--sidebar-width: {sidebarWidth.px}px">
	<ModeWatcher />
	<AppSidebar />
	<Sidebar.Inset>
		{@render children?.()}
	</Sidebar.Inset>
</Sidebar.Provider>
