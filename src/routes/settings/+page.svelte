<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { setMode, userPrefersMode } from "mode-watcher";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import ThemeEditor from "$lib/components/theme-editor.svelte";
	import { theme } from "$lib/theme/store.svelte.js";
	import {
		EDITOR_SCOPES,
		editorScope,
		setEditorScope,
		type EditorScope,
	} from "$lib/editor-scope.svelte.js";
	import { git, initialise, refreshGit, saveIdentity } from "$lib/git.svelte.js";
	import { chooseLedger, ledger } from "$lib/ledger.svelte.js";
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

	/*
		The identity as typed, held apart from the store so Save can tell when it
		differs from what the repository has. The effect reseeds the fields from
		the store — on mount, and after Save or any other refresh — without
		clobbering what is being typed, since typing never touches the store.
	*/
	let name = $state("");
	let email = $state("");
	let gitError = $state("");
	let gitBusy = $state(false);
	/** The theme editor is a section of its own, folded away until wanted. */
	let themeOpen = $state(false);

	const identityUnchanged = $derived(
		name.trim() === (git.user_name ?? "") && email.trim() === (git.user_email ?? "")
	);

	$effect(() => {
		name = git.user_name ?? "";
		email = git.user_email ?? "";
	});

	/*
		Read the state on mount, and again whenever the ledger is swapped — the
		folder can be opened from the section above, and the repository belongs to
		whichever directory the journal sits in, so the section must not keep
		describing the previous one.
	*/
	$effect(() => {
		ledger.summary?.path;
		void refreshGit();
	});

	async function onInitialise() {
		gitError = "";
		gitBusy = true;
		try {
			gitError = (await initialise()) ?? "";
		} finally {
			gitBusy = false;
		}
	}

	async function onSaveIdentity() {
		gitError = "";
		gitBusy = true;
		try {
			gitError = (await saveIdentity(name.trim(), email.trim())) ?? "";
		} finally {
			gitBusy = false;
		}
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="text-lg font-semibold tracking-tight">Settings</h1>
		<span class="text-muted-foreground text-xs"
			>the ledger, git, appearance and theme, the source editor and the sidebar</span
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
					onclick={chooseLedger}
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

		<div class="flex flex-col gap-3">
			<div class="flex flex-col gap-0.5">
				<h2 class="text-sm font-medium">Git</h2>
				<p class="text-muted-foreground text-xs">
					History for the ledger's own files: the diff and commit views compare and
					store them through the repository holding the journal, using only the
					local repository — nothing is pushed anywhere.
				</p>
			</div>
			<div class="flex items-center gap-2">
				<span class="text-muted-foreground min-w-0 truncate font-mono text-xs" title={git.root ?? ""}>
					{git.repo
						? `repository at ${git.root ?? "the ledger's directory"}`
						: "not a git repository"}
				</span>
			</div>
			{#if git.repo}
				<div class="flex flex-col gap-2">
					<label class="flex flex-col gap-1">
						<span class="text-muted-foreground text-xs">user.name</span>
						<Input bind:value={name} placeholder="Your name" autocomplete="name" />
					</label>
					<label class="flex flex-col gap-1">
						<span class="text-muted-foreground text-xs">user.email</span>
						<Input
							type="email"
							bind:value={email}
							placeholder="you@example.com"
							autocomplete="email"
						/>
					</label>
					<Button
						size="lg"
						class="w-full sm:w-auto"
						disabled={gitBusy || identityUnchanged}
						onclick={onSaveIdentity}
					>
						{gitBusy ? "Saving…" : "Save identity"}
					</Button>
				</div>
			{:else}
				<Button
					size="lg"
					variant="secondary"
					class="w-full sm:w-auto"
					disabled={gitBusy}
					onclick={onInitialise}
				>
					{gitBusy ? "Initialising…" : "Initialise repository"}
				</Button>
			{/if}
			{#if gitError}
				<Alert variant="destructive">{gitError}</Alert>
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
				<h2 class="text-sm font-medium">Theme</h2>
				<p class="text-muted-foreground text-xs">
					The shadcn tokens behind every surface, colour and corner: edit them here, or paste a
					theme exported by a shadcn/TweakCN builder. Overrides live in one style element, never
					in the stylesheet, so Reset restores exactly what the app ships.
				</p>
			</div>
			<Collapsible.Root bind:open={themeOpen}>
				<Collapsible.Trigger
					class="hover:bg-muted/60 focus-visible:ring-ring/50 flex w-full items-center justify-between gap-2 rounded-md border border-border px-3 py-2 text-left text-xs transition-colors focus-visible:ring-[3px] focus-visible:outline-none"
				>
					<span class="min-w-0 truncate">
						{theme.active ? theme.active.name : "Shipped defaults"}
					</span>
					<span class="text-muted-foreground shrink-0">
						{themeOpen ? "Hide" : "Customise"}
					</span>
				</Collapsible.Trigger>
				<Collapsible.Content>
					<div class="pt-2">
						<ThemeEditor />
					</div>
				</Collapsible.Content>
			</Collapsible.Root>
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
