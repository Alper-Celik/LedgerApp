<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { setMode, userPrefersMode } from "mode-watcher";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import {
		EDITOR_SCOPES,
		editorScope,
		setEditorScope,
		type EditorScope,
	} from "$lib/editor-scope.svelte.js";
	import { ledger, openFolderPicker } from "$lib/ledger.svelte.js";
	import {
		SIDEBAR_DEFAULT_WIDTH,
		setSidebarWidth,
		sidebarWidth,
	} from "$lib/sidebar-width.svelte.js";

	/** `setMode`'s own parameter type, so the list below cannot drift from the library. */
	type Mode = Parameters<typeof setMode>[0];

	/*
		The theme preference as three choices. `setMode` writes `userPrefersMode`,
		which is the preference itself, so "System" stays selected while the
		system happens to be dark — `mode` only reads back the resolved
		light/dark and could never show "System" as the choice.
	*/
	const MODES = [
		{ id: "system", label: "System", hint: "Follow the operating system" },
		{ id: "light", label: "Light", hint: "Always light" },
		{ id: "dark", label: "Dark", hint: "Always dark" },
	] as const satisfies readonly { id: Mode; label: string; hint: string }[];
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Settings</h1>
		<span class="text-muted-foreground text-xs"
			>the ledger, appearance, the source editor and the sidebar</span
		>
	</div>

	<div class="flex max-w-2xl flex-col gap-6">
		<div class="flex flex-col gap-2">
			<div class="flex flex-col gap-0.5">
				<h2 class="text-sm font-medium">Ledger</h2>
				<p class="text-muted-foreground text-xs">
					The journal the app has open. Opening a folder loads the journal inside it —
					<code>main.bean</code> or the first root file the loader recognises.
				</p>
			</div>
			<div class="flex flex-wrap items-center gap-2">
				<Button
					size="sm"
					variant="secondary"
					disabled={ledger.opening}
					onclick={openFolderPicker}
				>
					{ledger.opening ? "Opening…" : "Open folder…"}
				</Button>
				<span
					class="text-muted-foreground min-w-0 truncate font-mono text-xs"
					title={ledger.summary?.path ?? ""}
				>
					{ledger.summary?.path ?? "no ledger loaded"}
				</span>
			</div>
			{#if ledger.error}
				<Alert variant="destructive">{ledger.error}</Alert>
			{/if}
		</div>

		<div class="flex flex-col gap-2">
			<div class="flex flex-col gap-0.5">
				<h2 class="text-sm font-medium">Appearance</h2>
				<p class="text-muted-foreground text-xs">
					Light, dark, or follow the operating system. Applies to every view.
				</p>
			</div>
			<ToggleGroup.Root
				type="single"
				value={userPrefersMode.current}
				onValueChange={(next) => next && setMode(next as Mode)}
				aria-label="Appearance"
			>
				{#each MODES as option (option.id)}
					<ToggleGroup.Item value={option.id} title={option.hint}>{option.label}</ToggleGroup.Item>
				{/each}
			</ToggleGroup.Root>
		</div>

		<div class="flex flex-col gap-2">
			<div class="flex flex-col gap-0.5">
				<h2 class="text-sm font-medium">Source editor</h2>
				<p class="text-muted-foreground text-xs">
					What the source editor opens while an entry is edited: the entry's own lines, or its whole
					.bean file. The register follows this choice.
				</p>
			</div>
			<ToggleGroup.Root
				type="single"
				value={editorScope.value}
				onValueChange={(next) => next && setEditorScope(next as EditorScope)}
				aria-label="Source editor scope"
			>
				{#each EDITOR_SCOPES as option (option.id)}
					<ToggleGroup.Item value={option.id} title={option.hint}>{option.label}</ToggleGroup.Item>
				{/each}
			</ToggleGroup.Root>
		</div>

		<div class="flex flex-col gap-2">
			<div class="flex flex-col gap-0.5">
				<h2 class="text-sm font-medium">Sidebar</h2>
				<p class="text-muted-foreground text-xs">
					Width of the navigation sidebar. Drag its edge to resize it; the default is
					{SIDEBAR_DEFAULT_WIDTH}px.
				</p>
			</div>
			<div class="flex flex-wrap items-center gap-2">
				<span class="text-muted-foreground text-xs">{sidebarWidth.px}px</span>
				<Button
					size="sm"
					variant="secondary"
					disabled={sidebarWidth.px === SIDEBAR_DEFAULT_WIDTH}
					onclick={() => setSidebarWidth(SIDEBAR_DEFAULT_WIDTH)}
				>
					Reset to default
				</Button>
			</div>
		</div>
	</div>
</div>
