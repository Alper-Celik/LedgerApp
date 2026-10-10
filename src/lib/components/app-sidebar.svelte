<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { onMount } from "svelte";
	import ChevronRight from "@lucide/svelte/icons/chevron-right";
	import { commands } from "../../bindings";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";

	type AccountNode = {
		name: string;
		fullName: string;
		children: AccountNode[];
	};

	function buildTree(accounts: string[]): AccountNode[] {
		const root: AccountNode = { name: "", fullName: "", children: [] };
		for (const account of [...accounts].sort()) {
			let node = root;
			let path = "";
			for (const part of account.split(":")) {
				path = path ? `${path}:${part}` : part;
				let child = node.children.find((c) => c.name === part);
				if (!child) {
					child = { name: part, fullName: path, children: [] };
					node.children.push(child);
				}
				node = child;
			}
		}
		return root.children;
	}

	let tree = $state<AccountNode[]>([]);
	let selected = $state<string | null>(null);

	onMount(async () => {
		tree = buildTree(await commands.accounts());
	});
</script>

{#snippet renderAccounts(nodes: AccountNode[], depth: number)}
	{#each nodes as node (node.fullName)}
		{#if node.children.length > 0}
			<Collapsible.Root open={depth === 0} class="group/collapsible">
				<Sidebar.MenuItem>
					<Collapsible.Trigger>
						{#snippet child({ props })}
							<Sidebar.MenuButton {...props}>
								<ChevronRight
									class="transition-transform group-data-[state=open]/collapsible:rotate-90"
								/>
								<span>{node.name}</span>
							</Sidebar.MenuButton>
						{/snippet}
					</Collapsible.Trigger>
					<Collapsible.Content>
						<Sidebar.MenuSub>
							{@render renderAccounts(node.children, depth + 1)}
						</Sidebar.MenuSub>
					</Collapsible.Content>
				</Sidebar.MenuItem>
			</Collapsible.Root>
		{:else if depth === 0}
			<Sidebar.MenuItem>
				<Sidebar.MenuButton
					isActive={selected === node.fullName}
					onclick={() => (selected = node.fullName)}
				>
					<span>{node.name}</span>
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		{:else}
			<Sidebar.MenuSubItem>
				<Sidebar.MenuSubButton
					isActive={selected === node.fullName}
					onclick={() => (selected = node.fullName)}
				>
					<span>{node.name}</span>
				</Sidebar.MenuSubButton>
			</Sidebar.MenuSubItem>
		{/if}
	{/each}
{/snippet}

<Sidebar.Root>
	<Sidebar.Header>test</Sidebar.Header>

	<Sidebar.Content>
		<Sidebar.Group>
			<Sidebar.GroupLabel>Accounts</Sidebar.GroupLabel>
			<Sidebar.GroupContent>
				<Sidebar.Menu>
					{@render renderAccounts(tree, 0)}
				</Sidebar.Menu>
			</Sidebar.GroupContent>
		</Sidebar.Group>
	</Sidebar.Content>
	<Sidebar.Footer />
</Sidebar.Root>
