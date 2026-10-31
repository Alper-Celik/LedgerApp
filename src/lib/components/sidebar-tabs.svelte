<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { page } from "$app/state";
	import { onMount } from "svelte";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { ChevronRightIcon } from "@hugeicons/core-free-icons";
	import { commands, type AccountBalance, type JournalFile } from "../../bindings";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import {
		ACCOUNT_SECTIONS,
		accountsForSection,
		buildAccountTree,
		type AccountNode,
	} from "$lib/accounts.js";
	import FileTree from "$lib/components/file-tree.svelte";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import { SIDEBAR_TABS, selectSidebarTab, sidebarTab } from "$lib/sidebar-tab.svelte.js";
	import { cn } from "$lib/utils.js";

	let accounts = $state<AccountBalance[]>([]);

	/**
	 * The journal's own files — the root the session was opened with and every
	 * `include` it pulls in — newest list from the backend. Each row carries what
	 * the file writes and how it stands in git, which is what makes the tree say
	 * where an edit landed.
	 */
	let files = $state<JournalFile[]>([]);

	/**
	 * The accounts the backend marks uncommitted, by full name. Its flag is
	 * already rolled up every ancestor, so a row asks by its own name — and the
	 * virtual root row, which stands for all of them, asks whether the set has
	 * anything in it.
	 */
	const modifiedAccounts = $derived(
		new Set(accounts.filter((account) => account.modified).map((account) => account.name))
	);

	/**
	 * Balances, or an empty list if the backend cannot answer — a rejected or
	 * failing command must leave the sidebar standing, not throw out of the
	 * render that called it.
	 */
	async function loadAccounts() {
		try {
			return (await commands.accounts()) ?? [];
		} catch (cause) {
			console.error("accounts failed", cause);
			return [];
		}
	}

	/** The journal's files, or none when the backend cannot answer (same rule). */
	async function loadFiles() {
		try {
			// `?? []`: a bridge that answers nothing at all (a plain browser, a
			// backend mid-restart) must leave the tree standing.
			return (await commands.journalFiles()) ?? [];
		} catch (cause) {
			console.error("journalFiles failed", cause);
			return [];
		}
	}

	/** Folded subtrees by account name; everything opens by default. */
	let folded = $state<Record<string, boolean>>({});
	let toggleFold = (name: string) => (folded[name] = !folded[name]);

	let sections = $derived(
		ACCOUNT_SECTIONS.map((section) => ({
			section,
			tree: buildAccountTree(accountsForSection(accounts, section, ACCOUNT_SECTIONS)),
		}))
	);

	/**
	 * Moves selection and focus together — the tabs activate on focus, so an
	 * arrow key does what a click does and the panel follows without a second
	 * press. Home and End jump to the ends; both arrows wrap.
	 */
	function moveTo(event: KeyboardEvent, from: number) {
		const last = SIDEBAR_TABS.length - 1;
		let next: number;
		switch (event.key) {
			case "ArrowRight":
			case "ArrowDown":
				next = from === last ? 0 : from + 1;
				break;
			case "ArrowLeft":
			case "ArrowUp":
				next = from === 0 ? last : from - 1;
				break;
			case "Home":
				next = 0;
				break;
			case "End":
				next = last;
				break;
			default:
				return;
		}
		event.preventDefault();
		selectSidebarTab(SIDEBAR_TABS[next].id);
		// The roving tabindex has already moved to the selected tab, so focus
		// follows the selection and the next key starts where the reader is.
		const tablist = (event.currentTarget as HTMLElement).closest('[role="tablist"]');
		tablist?.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
	}

	onMount(async () => {
		accounts = await loadAccounts();
		files = await loadFiles();
	});

	$effect(() => {
		// Balances and files are fetched once per page load, so an edit saved
		// elsewhere, a commit, or another ledger being opened has to bump the
		// revision to bring both lists back in step.
		const revision = ledgerRevision.value;
		if (revision === 0) return;
		// The fetches are async, so a slower older answer must not overwrite a
		// newer one: only the latest revision is allowed to land.
		void loadAccounts().then((next) => {
			if (revision === ledgerRevision.value) accounts = next;
		});
		void loadFiles().then((next) => {
			if (revision === ledgerRevision.value) files = next;
		});
	});
</script>

{#snippet linkInner(node: AccountNode)}
	<!--
		Single wrapper span: both MenuButton and MenuSubButton truncate their
		`span:last-child`, so the amount must not be that child.
	-->
	<span class="flex min-w-0 flex-1 items-center gap-2">
		<span class="min-w-0 truncate">{node.name}</span>
		<UncommittedMark modified={modifiedAccounts.has(node.fullName)} />
		{#if node.balance}
			<MoneyAmount
				units={node.balance.units}
				currency={node.balance.currency}
				class="ml-auto shrink-0"
			/>
		{/if}
	</span>
{/snippet}

{#snippet renderAccounts(nodes: AccountNode[], depth: number)}
	{#each nodes as node (node.fullName)}
		{@const href = resolve("/account/[id]", { id: node.fullName })}
		{#if node.children.length > 0}
			<Sidebar.MenuItem class="flex items-center">
				<Sidebar.MenuButton
					size="sm"
					class="w-auto rounded-none px-1.5"
					aria-label={`Toggle ${node.fullName}`}
					aria-expanded={!folded[node.fullName]}
					onclick={() => toggleFold(node.fullName)}
				>
					<HugeiconsIcon
						icon={ChevronRightIcon}
						strokeWidth={2}
						data-icon="inline-start"
						class={cn(
							"transition-transform transition-motion-fast",
							folded[node.fullName] ? "" : "rotate-90"
						)}
					/>
				</Sidebar.MenuButton>
				<Sidebar.MenuButton size="sm" class="rounded-none" isActive={page.url.pathname === href}>
					{#snippet child({ props })}
						<a {...props} {href}>
							{@render linkInner(node)}
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
			<!--
				Fold: the row track goes 1fr to 0fr, which animates the child list's
				own height either way — no measured height needed, so it works
				without the bits-ui collapsible (whose height variable is unusable
				here).
			-->
			<div
				class={cn("fold", !folded[node.fullName] && "fold-open")}
			>
				<div class="overflow-hidden">
					<!--
						Guide per level, flush with the items it precedes: the list
						keeps only the indent its rows need (`ml-3.5` 14px + the 1px
						border) and no left padding, so the vertical line sits at the
						start of each item. `mr-0`/`pr-0` keep nested rows' right edge
						flush with their parents'.
					-->
					<Sidebar.MenuSub class="ml-3.5 mr-0 translate-x-0 gap-0 py-0 pl-0 pr-0">
						{@render renderAccounts(node.children, depth + 1)}
					</Sidebar.MenuSub>
				</div>
			</div>
		{:else if depth === 0}
			<Sidebar.MenuItem>
				<Sidebar.MenuButton size="sm" class="rounded-none" isActive={page.url.pathname === href}>
					{#snippet child({ props })}
						<a {...props} {href}>
							{@render linkInner(node)}
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		{:else}
			<Sidebar.MenuSubItem>
				<Sidebar.MenuSubButton
					{href}
					class="rounded-none"
					isActive={page.url.pathname === href}
				>
					{@render linkInner(node)}
				</Sidebar.MenuSubButton>
			</Sidebar.MenuSubItem>
		{/if}
	{/each}
{/snippet}

<!--
	The sidebar's two lists, one at a time. The tab row sits above a panel area
	that scrolls by itself, so a deep account tree or a long file list moves
	without taking the navigation above it along. Both panels stay mounted,
	hidden rather than destroyed: switching tabs must not lose a fold the
	reader made in either tree.
-->
<div class="flex min-h-0 flex-1 flex-col" data-slot="sidebar-tabs">
	<div
		class="border-sidebar-border flex shrink-0 items-center gap-1 border-b px-2 py-1.5"
		role="tablist"
		aria-label="Sidebar sections"
	>
		{#each SIDEBAR_TABS as tab, index (tab.id)}
			{@const selected = sidebarTab.value === tab.id}
			<button
				type="button"
				role="tab"
				id={`sidebar-tab-${tab.id}`}
				aria-selected={selected}
				aria-controls={`sidebar-panel-${tab.id}`}
				tabindex={selected ? 0 : -1}
				class={cn(
					"focus-visible:ring-sidebar-ring h-7 min-w-0 flex-1 rounded-[calc(var(--radius-sm)_+_2px)] px-2 text-xs transition-colors focus-visible:ring-2 focus-visible:outline-hidden",
					selected
						? "bg-sidebar-accent text-sidebar-accent-foreground font-medium"
						: "text-sidebar-foreground/70 hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
				)}
				onclick={() => selectSidebarTab(tab.id)}
				onkeydown={(event) => moveTo(event, index)}
			>
				{tab.label}
			</button>
		{/each}
	</div>

	<div class="min-h-0 flex-1 overflow-y-auto">
		<div
			role="tabpanel"
			id="sidebar-panel-accounts"
			aria-labelledby="sidebar-tab-accounts"
			hidden={sidebarTab.value !== "accounts"}
		>
			{#if accounts.length === 0}
				<Sidebar.Group>
					<Sidebar.GroupContent>
						<p class="text-sidebar-foreground/60 px-2 py-1 text-xs">No accounts</p>
					</Sidebar.GroupContent>
				</Sidebar.Group>
			{:else}
				{#each sections as entry (entry.section.label)}
					<Sidebar.Group>
						<Sidebar.GroupContent>
							<Sidebar.Menu>
								<!--
									Virtual root: the whole ledger as one row, with every
									account folded under it. It has no balance of its own —
									summing all accounts is meaningless — so it shows only
									its label and the fold control.
								-->
								<Sidebar.MenuItem class="flex items-center">
									<Sidebar.MenuButton
										size="sm"
										class="w-auto rounded-none px-1.5"
										aria-label={`Toggle ${entry.section.label}`}
										aria-expanded={!folded[entry.section.label]}
										onclick={() => toggleFold(entry.section.label)}
									>
										<HugeiconsIcon
											icon={ChevronRightIcon}
											strokeWidth={2}
											data-icon="inline-start"
											class={cn(
												"transition-transform transition-motion-fast",
												folded[entry.section.label] ? "" : "rotate-90"
											)}
										/>
									</Sidebar.MenuButton>
									<Sidebar.MenuButton
										size="sm"
										class="rounded-none"
										isActive={page.url.pathname === resolve("/ledger")}
									>
										{#snippet child({ props })}
											<a {...props} href={resolve("/ledger")}>
												<span class="flex min-w-0 flex-1 items-center gap-2">
													<span class="min-w-0 truncate">{entry.section.label}</span>
													<UncommittedMark modified={modifiedAccounts.size > 0} />
												</span>
											</a>
										{/snippet}
									</Sidebar.MenuButton>
								</Sidebar.MenuItem>
								<div
									class={cn("fold", !folded[entry.section.label] && "fold-open")}
								>
									<div class="overflow-hidden">
										<!-- Accounts are children of the virtual root, one level in. -->
										<Sidebar.MenuSub class="ml-3.5 mr-0 translate-x-0 gap-0 py-0 pl-0 pr-0">
											{@render renderAccounts(entry.tree, 1)}
										</Sidebar.MenuSub>
									</div>
								</div>
							</Sidebar.Menu>
						</Sidebar.GroupContent>
					</Sidebar.Group>
				{/each}
			{/if}
		</div>

		<div
			role="tabpanel"
			id="sidebar-panel-files"
			aria-labelledby="sidebar-tab-files"
			hidden={sidebarTab.value !== "files"}
		>
			<!--
				The journal's own files: the root the session was opened with and
				every `include` it pulls in, drawn as the directory tree they live
				in, each file with what git says about it and how many entries it
				writes. A file is opened by path in the register's file view, so
				this is also how a new entry reaches a file other than the one it
				would otherwise be filed into. The section's label leads to the
				full-width screen of the same tree for when the sidebar is too
				narrow to read a deep path.
			-->
			<Sidebar.Group>
				<Sidebar.GroupLabel class="rounded-none">
					{#snippet child({ props })}
						<a
							{...props}
							href={resolve("/files")}
							class="hover:text-sidebar-foreground flex h-full w-full items-center"
						>
							Journal files
						</a>
					{/snippet}
				</Sidebar.GroupLabel>
				<Sidebar.GroupContent>
					{#if files.length > 0}
						<FileTree {files} compact />
					{:else}
						<!--
							The section stays so its label is still the way to the files
							screen, which is where "no ledger open" is explained.
						-->
						<p class="text-sidebar-foreground/60 px-2 py-1 text-xs">No journal files</p>
					{/if}
				</Sidebar.GroupContent>
			</Sidebar.Group>
		</div>
	</div>
</div>
