<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { resolve } from "$app/paths";
	import { page } from "$app/state";
	import { onMount } from "svelte";
	import ChevronRight from "@lucide/svelte/icons/chevron-right";
	import { commands, type AccountBalance } from "../../bindings";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";
	import {
		ACCOUNT_SECTIONS,
		accountsForSection,
		amountColorClass,
		buildAccountTree,
		formatAmount,
		type AccountNode,
	} from "$lib/accounts.js";

	let accounts = $state<AccountBalance[]>([]);

	let sections = $derived(
		ACCOUNT_SECTIONS.map((section) => ({
			section,
			tree: buildAccountTree(accountsForSection(accounts, section, ACCOUNT_SECTIONS)),
		}))
	);

	onMount(async () => {
		accounts = await commands.accounts();
	});
</script>

{#snippet linkInner(node: AccountNode, href: string)}
	<span>{node.name}</span>
	<span class="ml-auto w-24 text-left tabular-nums {amountColorClass(node.balance)}">
		{formatAmount(node.balance)}
	</span>
{/snippet}

{#snippet renderAccounts(nodes: AccountNode[], depth: number)}
	{#each nodes as node, i (node.fullName)}
		{@const href = resolve("/account/[id]", { id: node.fullName })}
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
					<Sidebar.MenuButton isActive={page.url.pathname === href}>
						{#snippet child({ props })}
							<a {...props} {href}>
								{@render linkInner(node, href)}
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
				<Sidebar.MenuButton isActive={page.url.pathname === href}>
					{#snippet child({ props })}
						<a {...props} {href}>
							{@render linkInner(node, href)}
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		{:else}
			<Sidebar.MenuSubItem>
				<Sidebar.MenuSubButton {href} isActive={page.url.pathname === href}>
					{@render linkInner(node, href)}
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
					<Sidebar.Menu>
						{@render renderAccounts(entry.tree, 0)}
					</Sidebar.Menu>
				</Sidebar.GroupContent>
			</Sidebar.Group>
		{/each}
	</Sidebar.Content>
	<Sidebar.Footer />
</Sidebar.Root>
