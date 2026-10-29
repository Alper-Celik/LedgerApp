<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

<script lang="ts">
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import AppSidebar from "$lib/components/app-sidebar.svelte";
	import FolderPicker from "$lib/components/folder-picker.svelte";
	import SyncBar from "$lib/components/sync-bar.svelte";
	import QuickAdd from "$lib/components/quick-add.svelte";
	import { sidebarWidth } from "$lib/sidebar-width.svelte.js";
	import { syncTree } from "$lib/ledger.svelte.js";
	import { refreshSyncWatch } from "$lib/sync.svelte.js";
	import { ledgerEpoch, ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import { resetLanguageServer } from "$lib/lsp.js";
	import { refreshGit } from "$lib/git.svelte.js";
	import { load as loadTheme } from "$lib/theme/store.svelte.js";
	import "../app.css";
	import { ModeWatcher } from "mode-watcher";

	let { children } = $props();

	// Before the first paint: re-apply the saved token overrides, if any.
	loadTheme();

	// A ledger in a SAF tree is edited through a mirror; every revision bump is a
	// successful write, so that is when the tree gets its copy back. On desktop
	// this is one cheap command call that answers "local" and returns.
	$effect(() => {
		void ledgerRevision.value;
		void syncTree();
		// Every write also tells whether the folder moved behind us; the bar in
		// the inset offers the reload.
		void refreshSyncWatch();
	});

	$effect(() => {
		// A different journal is open: the language server the backend stopped is
		// not the one the next editor should attach to, and the repository the
		// sidebar gates Diff/Commit on is the new ledger's, not the old one's.
		void ledgerEpoch.value;
		resetLanguageServer();
		void refreshGit();
	});
</script>

<!-- The passed style lands after the component's default `--sidebar-width`, so it wins. -->
<Sidebar.Provider style="--sidebar-width: {sidebarWidth.px}px">
	<ModeWatcher />
	<AppSidebar />
	<Sidebar.Inset>
		<!--
			Below `md` the sidebar is a sheet, so this bar is the only way in: it
			carries the trigger and stays while the page scrolls. Desktop keeps
			the fixed sidebar and sees none of it.
		-->
		<header
			class="bg-background/85 sticky top-0 z-20 flex h-11 items-center gap-1 border-b px-2 backdrop-blur md:hidden"
		>
			<Sidebar.Trigger class="size-9 -ms-1" aria-label="Open navigation" />
			<span class="truncate text-sm font-semibold">Ledger</span>
		</header>
		<SyncBar />
		{@render children?.()}
	</Sidebar.Inset>
	<!-- One picker for the whole app: the sidebar and Settings both just open it. -->
	<FolderPicker />
	<!-- One quick-add button for the whole app, so navigating never remounts it. -->
	<QuickAdd />
</Sidebar.Provider>
