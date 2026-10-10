<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import ChevronRight from "@lucide/svelte/icons/chevron-right";
	import { page } from "$app/state";
	import { commands, type AccountBalance } from "../../bindings";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";
	import {
		ACCOUNT_SECTIONS,
		accountsForSection,
		accountHref,
		amountColorClass,
		buildAccountTree,
		formatAmount,
		type AccountNode,
	} from "$lib/accounts.js";

	let accounts = $state<AccountBalance[]>([]);

	let sections = $derived(
		ACCOUNT_SECTIONS.map((section) => {
			const list = accountsForSection(accounts, section, ACCOUNT_SECTIONS);
			return {
				section,
				tree: buildAccountTree(list),
				total: list.reduce((acc, account) => acc + (account.balance ?? 0), 0),
			};
		})
	);

	onMount(async () => {
		accounts = await commands.accounts();
	});
</script>

{#snippet linkInner(node: AccountNode)}
	<span>{node.name}</span>
	<span class="ml-auto w-24 text-left tabular-nums {amountColorClass(node.balance)}">
		{formatAmount(node.balance)}
	</span>
{/snippet}

{#snippet renderAccounts(nodes: AccountNode[], depth: number)}
	{#each nodes as node, i (node.fullName)}
		{#if i > 0}
			<Sidebar.MenuSubItem><Sidebar.Separator /></Sidebar.MenuSubItem>
		{/if}
		{#if node.children.length > 0}
			<Collapsible.Root open={true} class="group/collapsible">
				<Sidebar.MenuItem class="flex items-center">
					<Collapsible.Trigger>
						{#snippet child({ props })}
							<Sidebar.MenuButton
								{...props}
								class="w-auto px-1.5"
								aria-label={`Toggle ${node.fullName}`}
							>
								<ChevronRight
									class="transition-transform group-data-[state=open]/collapsible:rotate-90"
								/>
							</Sidebar.MenuButton>
						{/snippet}
					</Collapsible.Trigger>
					<Sidebar.MenuButton isActive={page.url.pathname === accountHref(node.fullName)}>
						{#snippet child({ props })}
							<a {...props} href={accountHref(node.fullName)}>
								{@render linkInner(node)}
							</a>
						{/snippet}
					</Sidebar.MenuButton>
				</Sidebar.MenuItem>
				<Collapsible.Content>
					<Sidebar.MenuSub>
						{@render renderAccounts(node.children, depth + 1)}
					</Sidebar.MenuSub>
				</Collapsible.Content>
			</Collapsible.Root>
		{:else if depth === 0}
			<Sidebar.MenuItem>
				<Sidebar.MenuButton isActive={page.url.pathname === accountHref(node.fullName)}>
					{#snippet child({ props })}
						<a {...props} href={accountHref(node.fullName)}>
							{@render linkInner(node)}
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		{:else}
			<Sidebar.MenuSubItem>
				<Sidebar.MenuSubButton
					href={accountHref(node.fullName)}
					isActive={page.url.pathname === accountHref(node.fullName)}
				>
					{@render linkInner(node)}
				</Sidebar.MenuSubButton>
			</Sidebar.MenuSubItem>
		{/if}
	{/each}
{/snippet}

<Sidebar.Root>
	<Sidebar.Header>test</Sidebar.Header>

	<Sidebar.Content>
		{#each sections as entry (entry.section.label)}
			<Sidebar.Group>
				<Sidebar.GroupLabel>{entry.section.label}</Sidebar.GroupLabel>
				<Sidebar.GroupContent>
					{#if entry.section.sumOnly}
						<Sidebar.Menu>
							<Sidebar.MenuItem>
								<Sidebar.MenuButton>
									<span>{entry.section.label}</span>
									<span
										class="ml-auto w-24 text-left tabular-nums {amountColorClass(entry.total)}"
									>
										{formatAmount(entry.total)}
									</span>
								</Sidebar.MenuButton>
							</Sidebar.MenuItem>
						</Sidebar.Menu>
					{:else}
						<Sidebar.Menu>
							{@render renderAccounts(entry.tree, 0)}
						</Sidebar.Menu>
					{/if}
				</Sidebar.GroupContent>
			</Sidebar.Group>
		{/each}
	</Sidebar.Content>
	<Sidebar.Footer />
</Sidebar.Root>
