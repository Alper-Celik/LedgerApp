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
		ChevronRightIcon,
		FileDiffIcon,
		GitCommitIcon,
		Settings01Icon,
	} from "@hugeicons/core-free-icons";
	import { commands, type AccountBalance } from "../../bindings";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import {
		ACCOUNT_SECTIONS,
		accountsForSection,
		buildAccountTree,
		type AccountNode,
	} from "$lib/accounts.js";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import {
		SIDEBAR_WIDTH_STORAGE_KEY,
		setSidebarWidth,
		sidebarWidth,
	} from "$lib/sidebar-width.svelte.js";
	import { cn } from "$lib/utils.js";

	let accounts = $state<AccountBalance[]>([]);

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
	 * Non-account views. Future budget and dashboard views belong in this group
	 * (or a sibling group) the same way.
	 */
	const GIT_VIEWS = [
		{ href: "/diff", label: "Diff", icon: FileDiffIcon },
		{ href: "/commit", label: "Commit", icon: GitCommitIcon },
	] as const;

	/**
	 * Balances, or an empty list if the backend cannot answer — a rejected or
	 * failing command must leave the sidebar standing, not throw out of the
	 * render that called it.
	 */
	async function loadAccounts() {
		try {
			return await commands.accounts();
		} catch (cause) {
			console.error("accounts failed", cause);
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
		localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(sidebarWidth.px));
	}

	let sections = $derived(
		ACCOUNT_SECTIONS.map((section) => ({
			section,
			tree: buildAccountTree(accountsForSection(accounts, section, ACCOUNT_SECTIONS)),
		}))
	);

	onMount(async () => {
		accounts = await loadAccounts();
	});

	$effect(() => {
		// Balances are fetched once per page load, so an edit saved elsewhere (or a
		// commit) has to bump the revision to bring this list back in step.
		if (ledgerRevision.value === 0) return;
		loadAccounts();
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
				<Sidebar.MenuButton size="lg" isActive={page.url.pathname === resolve("/")}>
					{#snippet child({ props })}
						<a {...props} href={resolve("/")}>
							<span class="min-w-0 flex-1 truncate">Ledger</span>
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Header>

	<Sidebar.Content>
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
