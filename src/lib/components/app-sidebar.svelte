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
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import SidebarTabs from "$lib/components/sidebar-tabs.svelte";
	import { git, refreshGit } from "$lib/git.svelte.js";
	import { chooseLedger, ledger, refreshLedger, refreshStorageError } from "$lib/ledger.svelte.js";
	import { setSidebarWidth, sidebarWidth } from "$lib/sidebar-width.svelte.js";

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

	onMount(() => {
		// Which journal the app opened (the CLI argument, LEDGER_PATH or
		// main.bean), so Settings can name it before a folder is picked.
		refreshLedger();
		// And, when there is none, why the folder it was pointed at could not be
		// reopened: the restore happened before this window existed.
		refreshStorageError();
		// Whether it is under git decides if the Diff/Commit views exist at all.
		refreshGit();
	});
</script>

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

	<!--
		The navigation above the lists: the git views and the ledger-wide screens.
		They are not part of either tab — switching to Files must not take the way
		to Budget away — so the tabbed region below replaces only the account and
		file lists that used to stack here.
	-->
	<Sidebar.Content class="overflow-hidden">
		<!--
			Git views only exist when the ledger is a repository: a ledger that is
			not under git has no diff to show and nothing to commit, and offering
			the two links would only lead to a page that says so.
		-->
		{#if git.repo}
			<Sidebar.Group class="shrink-0">
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
		<Sidebar.Group class="shrink-0">
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

		<!-- The two lists that used to stack: one tab each, scrolled on their own. -->
		<SidebarTabs />
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
