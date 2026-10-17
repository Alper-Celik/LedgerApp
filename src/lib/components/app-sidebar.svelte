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
		Add01Icon,
		ChevronRightIcon,
		FileDiffIcon,
		GitCommitIcon,
		Settings01Icon,
	} from "@hugeicons/core-free-icons";
	import { commands, type AccountBalance, type JournalFile } from "../../bindings";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Alert, AlertDescription } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import { today } from "$lib/dates.js";
	import {
		ACCOUNT_SECTIONS,
		accountsForSection,
		buildAccountTree,
		type AccountNode,
	} from "$lib/accounts.js";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import UncommittedMark from "$lib/components/uncommitted-mark.svelte";
	import { bumpLedgerRevision, ledgerRevision } from "$lib/ledger-revision.svelte.js";
	import {
		SIDEBAR_WIDTH_STORAGE_KEY,
		setSidebarWidth,
		sidebarWidth,
	} from "$lib/sidebar-width.svelte.js";
	import { cn } from "$lib/utils.js";

	let accounts = $state<AccountBalance[]>([]);
	/** The journal's files, for the browser below the accounts — listed and fetched together. */
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

	/**
	 * The journal's files, on the same terms: the browser below the accounts is
	 * this sidebar's own, so it is fetched here rather than routed — an edit
	 * anywhere changes what a file holds, and the revision effect below brings
	 * both lists back in step.
	 */
	async function loadFiles() {
		try {
			return await commands.journalFiles();
		} catch (cause) {
			console.error("journalFiles failed", cause);
			return [];
		}
	}

	/** Folded subtrees by account name; everything opens by default. */
	let folded = $state<Record<string, boolean>>({});
	let toggleFold = (name: string) => (folded[name] = !folded[name]);

	/**
	 * Opening an account, inline in the tree: the row above the accounts is the
	 * form. An account *is* an `open` directive, so the form writes one into the
	 * file it names, through the same reload, refusal and staging pipeline as an
	 * entry — and the loader decides whether the name is an account and whether
	 * it has one already.
	 */
	let opening = $state(false);
	let openSaving = $state(false);
	let openError = $state("");
	let draft = $state({ name: "", date: "", currencies: "" });

	/** A file picked by hand; the deduction keeps its hands off while this is set. */
	let chosen = $state<string | null>(null);
	/** What the backend deduced for this name, and the name it was asked about. */
	let suggested = $state<{ path: string; from: string } | null>(null);
	/** The file the account will be opened in. */
	let file = $derived(chosen ?? suggested?.path ?? files[0]?.path ?? "");
	/** That file's own name, which is what the picker shows — the path is its id. */
	let fileLabel = $derived(files.find((option) => option.path === file)?.label ?? "");

	/**
	 * Ask the backend which file this account belongs in, and ask again as the
	 * name is typed: the answer comes from the account's family — where its
	 * ancestors are opened, or where their entries live.
	 */
	$effect(() => {
		if (!opening) return;
		const name = draft.name.trim();
		let live = true;
		const timer = setTimeout(async () => {
			const path = await commands.suggestAccountFile(name);
			if (live) suggested = path ? { path, from: name } : null;
		}, 150);
		return () => {
			live = false;
			clearTimeout(timer);
		};
	});

	/** Open the form on a clean draft: today, in the file the ledger starts in. */
	function startOpening() {
		draft.name = "";
		draft.date = today();
		draft.currencies = "";
		chosen = null;
		suggested = null;
		openError = "";
		opening = true;
	}

	/**
	 * Write the `open` directive. The commodities are a comma-separated list, and
	 * an empty one is beancount's "any commodity"; the loader's complaint about
	 * the name, or about an account that is already open, is what the form shows.
	 */
	async function saveAccount() {
		if (openSaving) return;
		openSaving = true;
		openError = "";
		try {
			const result = await commands.addAccount({
				file,
				name: draft.name.trim(),
				date: draft.date.trim(),
				currencies: draft.currencies
					.split(",")
					.map((currency) => currency.trim())
					.filter((currency) => currency !== ""),
			});
			if (result.status === "error") {
				openError = result.error;
				return;
			}
		} catch (cause) {
			openError = cause instanceof Error ? cause.message : String(cause);
			return;
		} finally {
			openSaving = false;
		}
		opening = false;
		// The tree this sidebar draws is fetched rather than routed, so it is the
		// revision that brings the new account in — with an empty balance, which
		// is what an account with nothing in it has.
		bumpLedgerRevision();
	}

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
		files = await loadFiles();
	});

	$effect(() => {
		// Balances and files are fetched once per page load, so an edit saved
		// elsewhere (or a commit) has to bump the revision to bring both lists
		// back in step.
		if (ledgerRevision.value === 0) return;
		void loadAccounts().then((balances) => (accounts = balances));
		void loadFiles().then((journal) => (files = journal));
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
							Opening an account: the row is the trigger, and the form
							takes its place under it — in the tree the account will
							appear in, which is where one looks for it afterwards.
						-->
						<Sidebar.MenuItem>
							<Sidebar.MenuButton size="sm" class="rounded-none" onclick={startOpening}>
								<HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
								<span class="min-w-0 flex-1 truncate">Add account</span>
							</Sidebar.MenuButton>
						</Sidebar.MenuItem>
						{#if opening}
							<Sidebar.MenuItem>
								<form
									class="flex flex-col gap-1.5 px-1.5 pb-2"
									onsubmit={(event) => {
										event.preventDefault();
										saveAccount();
									}}
								>
									<Input
										class="h-6 px-1 text-xs"
										placeholder="Expenses:Coffee"
										bind:value={draft.name}
										aria-label="Account name"
									/>
									<Input
										class="h-6 px-1 text-xs tabular-nums"
										placeholder="YYYY-MM-DD"
										bind:value={draft.date}
										aria-label="Opened on"
									/>
									<Input
										class="h-6 px-1 text-xs"
										placeholder="commodities, optional"
										bind:value={draft.currencies}
										aria-label="Commodities"
									/>
									<Select.Root
										type="single"
										value={file}
										onValueChange={(next) => next && (chosen = next)}
									>
										<Select.Trigger class="w-full" aria-label="File">
											<span data-slot="select-value" class={fileLabel ? "" : "text-muted-foreground"}>
												{fileLabel || "no file"}
											</span>
										</Select.Trigger>
										<Select.Content>
											{#each files as option (option.path)}
												<Select.Item value={option.path} label={option.label} />
											{/each}
										</Select.Content>
									</Select.Root>
									<p class="text-muted-foreground text-xs">
										{#if files.length === 0}
											No journal file is loaded, so there is nowhere to open an account.
										{:else if !chosen && suggested && suggested.from !== ""}
											Suggested from where {suggested.from}'s family lives.
										{:else}
											The account is opened at the end of this file.
										{/if}
									</p>
									<div class="flex items-center gap-1">
										<Button size="sm" type="submit" disabled={openSaving || file === ""}>
											Open account
										</Button>
										<Button
											size="sm"
											variant="ghost"
											disabled={openSaving}
											onclick={() => (opening = false)}
										>
											Cancel
										</Button>
									</div>
									{#if openError}
										<Alert variant="destructive">
											<AlertDescription>{openError}</AlertDescription>
										</Alert>
									{/if}
								</form>
							</Sidebar.MenuItem>
						{/if}
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

		<!--
			The journal's files, listed the way the accounts are: this is the file
			browser, and every row is a link to that file's own page, where it is
			read as transactions or as text.
		-->
		<Sidebar.Group>
			<Sidebar.GroupLabel class="rounded-none">Files</Sidebar.GroupLabel>
			<Sidebar.GroupContent>
				<Sidebar.Menu>
					{#each files as file (file.path)}
						<!--
							The path is the file's identity, so it is what the route
							carries — percent-encoded into one path segment.
							`resolve` is not used here because it refuses a parameter
							that starts with a slash, which every absolute path does.
						-->
						{@const href = `/files/${encodeURIComponent(file.path)}`}
						<Sidebar.MenuItem>
							<Sidebar.MenuButton
								size="sm"
								class="rounded-none"
								isActive={page.url.pathname === href}
							>
								{#snippet child({ props })}
									<a {...props} {href} title={file.path}>
										<span class="flex min-w-0 flex-1 items-center gap-2">
											<span class="min-w-0 truncate">{file.label}</span>
											<UncommittedMark modified={file.modified} />
											<span class="text-muted-foreground ml-auto shrink-0 text-xs tabular-nums">
												{file.transactions}
											</span>
										</span>
									</a>
								{/snippet}
							</Sidebar.MenuButton>
						</Sidebar.MenuItem>
					{/each}
					{#if files.length === 0}
						<Sidebar.MenuItem>
							<span class="text-muted-foreground px-2 py-1 text-xs">
								No journal file is loaded.
							</span>
						</Sidebar.MenuItem>
					{/if}
				</Sidebar.Menu>
			</Sidebar.GroupContent>
		</Sidebar.Group>
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
