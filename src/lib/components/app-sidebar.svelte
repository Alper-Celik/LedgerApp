<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { page } from "$app/state";
	import { onMount } from "svelte";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import {
		AiChat01Icon,
		AlertCircleIcon,
		ChevronRightIcon,
		CoinsIcon,
		DashboardSquare01Icon,
		FileDiffIcon,
		FolderOpenIcon,
		GitBranchIcon,
		GitCommitIcon,
		HistoryIcon,
		Settings01Icon,
		TrendingUpIcon,
		Wallet01Icon,
	} from "@hugeicons/core-free-icons";
	import { commands, type AccountBalance, type JournalFile } from "../../bindings";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import {
		ACCOUNT_SECTIONS,
		accountsForSection,
		buildAccountTree,
		type AccountNode,
	} from "$lib/accounts.js";
	import GitStatus from "$lib/components/git-status.svelte";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { git, refreshGit } from "$lib/git.svelte.js";
	import { chooseLedger, ledger, refreshLedger } from "$lib/ledger.svelte.js";
	import { ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import { setSidebarWidth, sidebarWidth } from "$lib/sidebar-width.svelte.js";
	import { cn } from "$lib/utils.js";

	let accounts = $state<AccountBalance[]>([]);

	/**
	 * The journal's own files — the root the session was opened with and every
	 * `include` it pulls in — newest list from the backend. Each row carries what
	 * the file writes and how it stands in git, which is what makes the tree say
	 * where an edit landed.
	 */
	let files = $state<JournalFile[]>([]);

	const sidebar = Sidebar.useSidebar();

	/**
	 * Below `md` this sidebar is a sheet, and a sheet left open covers the page
	 * the link that was tapped navigated to. Every navigation closes it — the
	 * pathname read is the dependency — while the desktop sidebar's own open
	 * state is left alone.
	 */
	$effect(() => {
		void page.url.pathname;
		if (sidebar.isMobile) sidebar.setOpenMobile(false);
	});

	/**
	 * The accounts the backend marks uncommitted, by full name. Its flag is
	 * already rolled up every ancestor, so a row asks by its own name — and the
	 * virtual root row, which stands for all of them, asks whether the set has
	 * anything in it.
	 */
	const modifiedAccounts = $derived(
		new Set(accounts.filter((account) => account.modified).map((account) => account.name))
	);

	/** Non-account views: the ledger's git routes. */
	const GIT_VIEWS = [
		{ href: "/diff", label: "Diff", icon: FileDiffIcon },
		{ href: "/commit", label: "Commit", icon: GitCommitIcon },
	] as const;

	/** The ledger-wide views beside the accounts. */
	const LEDGER_VIEWS = [
		{ href: "/budget", label: "Budget", icon: Wallet01Icon },
		{ href: "/dashboards", label: "Dashboards", icon: DashboardSquare01Icon },
		{ href: "/inflation", label: "Inflation", icon: TrendingUpIcon },
		{ href: "/prices", label: "Prices", icon: CoinsIcon },
		{ href: "/branches", label: "Branches", icon: GitBranchIcon },
		{ href: "/conflicts", label: "Conflicts", icon: AlertCircleIcon },
		{ href: "/history", label: "History", icon: HistoryIcon },
		{ href: "/assistant", label: "Assistant", icon: AiChat01Icon },
	] as const;

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

	let resizing = false;
	let dragStartX = 0;
	let dragStartWidth = 0;

	function startResize(event: PointerEvent) {
		resizing = true;
		dragStartX = event.clientX;
		dragStartWidth = sidebarWidth.px;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		// Vendored width transitions would trail the pointer (see app.css).
		document.documentElement.classList.add("sidebar-resizing");
	}

	function moveResize(event: PointerEvent) {
		if (!resizing) return;
		setSidebarWidth(dragStartWidth + event.clientX - dragStartX);
	}

	function endResize(event: PointerEvent) {
		if (!resizing) return;
		resizing = false;
		const handle = event.currentTarget as HTMLElement;
		if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
		document.documentElement.classList.remove("sidebar-resizing");
	}

	let sections = $derived(
		ACCOUNT_SECTIONS.map((section) => ({
			section,
			tree: buildAccountTree(accountsForSection(accounts, section, ACCOUNT_SECTIONS)),
		}))
	);

	onMount(async () => {
		accounts = await loadAccounts();
		files = await loadFiles();
		// Which journal the app opened (the CLI argument, LEDGER_PATH or
		// main.bean), so Settings can name it before a folder is picked.
		refreshLedger();
		// Whether it is under git decides if the Diff/Commit views exist at all.
		refreshGit();
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

<Sidebar.Root>
	<Sidebar.Header>
		<Sidebar.Menu>
			<Sidebar.MenuItem>
				<Sidebar.MenuButton size="lg" isActive={page.url.pathname === resolve("/ledger")}>
					{#snippet child({ props })}
						<a {...props} href={resolve("/ledger")}>
							<span class="min-w-0 flex-1 truncate">Ledger</span>
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Header>

	<Sidebar.Content>
		<!--
			Git views only exist when the ledger is a repository: a ledger that is
			not under git has no diff to show and nothing to commit, and offering
			the two links would only lead to a page that says so.
		-->
		{#if git.repo}
			<Sidebar.Group>
				<Sidebar.GroupLabel class="rounded-none">Git</Sidebar.GroupLabel>
				<Sidebar.GroupContent>
					<Sidebar.Menu>
						{#each GIT_VIEWS as item (item.href)}
							<Sidebar.MenuItem>
								<Sidebar.MenuButton
									size="sm"
									class="rounded-none"
									isActive={page.url.pathname === resolve(item.href)}
								>
									{#snippet child({ props })}
										<a {...props} href={resolve(item.href)}>
											<HugeiconsIcon
												icon={item.icon}
												strokeWidth={2}
												data-icon="inline-start"
											/>
											<span class="min-w-0 flex-1 truncate">{item.label}</span>
										</a>
									{/snippet}
								</Sidebar.MenuButton>
							</Sidebar.MenuItem>
						{/each}
					</Sidebar.Menu>
				</Sidebar.GroupContent>
			</Sidebar.Group>
		{/if}
		<Sidebar.Group>
			<Sidebar.GroupContent>
				<Sidebar.Menu>
					{#each LEDGER_VIEWS as item (item.href)}
						<Sidebar.MenuItem>
							<Sidebar.MenuButton
								size="sm"
								class="rounded-none"
								isActive={page.url.pathname === resolve(item.href)}
							>
								{#snippet child({ props })}
									<a {...props} href={resolve(item.href)}>
										<HugeiconsIcon
											icon={item.icon}
											strokeWidth={2}
											data-icon="inline-start"
										/>
										<span class="min-w-0 flex-1 truncate">{item.label}</span>
									</a>
								{/snippet}
							</Sidebar.MenuButton>
						</Sidebar.MenuItem>
					{/each}
				</Sidebar.Menu>
			</Sidebar.GroupContent>
		</Sidebar.Group>

		<!--
			The journal's own files: the root the session was opened with and
			every `include` it pulls in, each with what git says about it and how
			many entries it writes. A file is opened by path in the register's
			file view, so this is also how a new entry reaches a file other than
			the one it would otherwise be filed into.
		-->
		{#if files.length > 0}
			<Sidebar.Group>
				<Sidebar.GroupLabel class="rounded-none">Journal files</Sidebar.GroupLabel>
				<Sidebar.GroupContent>
					<Sidebar.Menu>
						{#each files as file (file.path)}
							{@const href = `/files/${encodeURIComponent(file.path)}`}
							<Sidebar.MenuItem>
								<Sidebar.MenuButton
									size="sm"
									class="rounded-none"
									isActive={page.url.pathname === href}
								>
									{#snippet child({ props })}
										<a {...props} {href} title={file.path}>
											<!-- The letter is absent when nothing changed; the slot keeps the labels aligned. -->
											<span class="flex w-3 shrink-0 justify-center">
												<GitStatus change={file.change} />
											</span>
											<span class="min-w-0 flex-1 truncate">{file.label}</span>
											<span class="text-muted-foreground shrink-0 text-xs tabular-nums">
												{file.transactions}
											</span>
										</a>
									{/snippet}
								</Sidebar.MenuButton>
							</Sidebar.MenuItem>
						{/each}
					</Sidebar.Menu>
				</Sidebar.GroupContent>
			</Sidebar.Group>
		{/if}
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
	</Sidebar.Content>
	<Sidebar.Footer>
		<Sidebar.Menu>
			<!--
				Pointing the app at another ledger: the desktop's own folder
				chooser, whose choice is loaded as the session's journal (the
				in-app picker stands in on runtimes that cannot draw one). In the
				sidebar (and so in the mobile sheet) because it is app-wide, like
				Settings; `+layout.svelte` mounts the picker itself.
			-->
			<Sidebar.MenuItem>
				<Sidebar.MenuButton
					size="sm"
					class="rounded-none"
					aria-disabled={ledger.opening}
					onclick={chooseLedger}
				>
					<HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} data-icon="inline-start" />
					<span class="min-w-0 flex-1 truncate">
						{ledger.opening ? "Opening…" : "Open folder…"}
					</span>
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
			<Sidebar.MenuItem>
				<Sidebar.MenuButton
					size="sm"
					class="rounded-none"
					isActive={page.url.pathname === resolve("/settings")}
				>
					{#snippet child({ props })}
						<a {...props} href={resolve("/settings")}>
							<HugeiconsIcon
								icon={Settings01Icon}
								strokeWidth={2}
								data-icon="inline-start"
							/>
							<span class="min-w-0 flex-1 truncate">Settings</span>
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Footer>

	<!--
		Desktop-only resize handle. It sits inside the fixed sidebar container,
		so `end-0` is the sidebar's edge; dragging sets `--sidebar-width`.
	-->
	<button
		type="button"
		aria-label="Resize sidebar"
		tabindex={-1}
		class="absolute inset-y-0 end-0 z-20 hidden w-1.5 cursor-col-resize touch-none bg-transparent transition-colors transition-motion-fast select-none hover:bg-sidebar-border md:block"
		onpointerdown={startResize}
		onpointermove={moveResize}
		onpointerup={endResize}
		onpointercancel={endResize}
	></button>
</Sidebar.Root>
