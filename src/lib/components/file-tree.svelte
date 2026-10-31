<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { page } from "$app/state";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { ChevronRightIcon } from "@hugeicons/core-free-icons";
	import type { Change, JournalFile } from "../../bindings";
	import GitStatus from "$lib/components/git-status.svelte";
	import { cn } from "$lib/utils.js";

	/**
	 * The journal's files as the directory tree they live in.
	 *
	 * A flat list of labels makes the reader scan a whole path to find the
	 * folder they already know, so the same files are drawn by their path:
	 * folders fold, leaves open their file, and every row carries what the
	 * file list already carried — how many entries the file writes and how it
	 * stands in git. A folder's count is its descendants' and its letter is the
	 * strongest letter among them, so a subtree says at a glance whether it
	 * holds anything new.
	 *
	 * The rows are one flat list with per-depth padding rather than nested
	 * lists: the sidebar is narrow (and a phone narrower), and a nested `ul`'s
	 * own padding would spend the width on indentation instead of the path.
	 */
	let {
		files,
		compact = false,
		class: className,
	}: {
		/** The journal's files, as the backend lists them. */
		files: JournalFile[];
		/**
		 * Draw at the sidebar's scale and colours: the tree is used both there
		 * and full-width on the files screen, and the two contexts are only a
		 * size and an accent apart.
		 */
		compact?: boolean;
		class?: string;
	} = $props();

	/** One folder or file in the tree, with what it sums to. */
	type TreeNode = {
		kind: "folder" | "file";
		/** Last path segment, the row's visible name. */
		name: string;
		/** The full path, the row's identity and the link's target. */
		path: string;
		/** Transactions this node and everything under it writes. */
		transactions: number;
		/** The strongest change anywhere under it. */
		change: Change;
		children: TreeNode[];
	};

	/**
	 * Added outranks modified: a folder holding a file the last commit does not
	 * have is more useful to flag than one holding a rewrite, and the letter is
	 * a prompt to look rather than a description of every descendant.
	 */
	const CHANGE_ORDER: Record<Change, number> = { unchanged: 0, modified: 1, added: 2 };

	/**
	 * Splits every path into folders and a leaf. Segments are sorted into
	 * folders before files, each alphabetically, which is what makes a subtree
	 * readable; the backend's list order carries no meaning here.
	 */
	function buildTree(files: JournalFile[]): TreeNode[] {
		const roots: TreeNode[] = [];
		const folders = new Map<string, TreeNode>();
		const seen = new Set<string>();

		for (const file of files) {
			if (seen.has(file.path)) continue;
			seen.add(file.path);
			const segments = file.path.split("/").filter((segment) => segment.length > 0);
			if (segments.length === 0) continue;

			let children = roots;
			let prefix = "";
			for (const [index, segment] of segments.entries()) {
				prefix = `${prefix}/${segment}`;
				if (index === segments.length - 1) {
					children.push({
						kind: "file",
						name: segment,
						path: file.path,
						transactions: file.transactions,
						change: file.change,
						children: [],
					});
					continue;
				}
				let folder = folders.get(prefix);
				if (!folder) {
					folder = {
						kind: "folder",
						name: segment,
						path: prefix,
						transactions: 0,
						change: "unchanged",
						children: [],
					};
					folders.set(prefix, folder);
					children.push(folder);
				}
				children = folder.children;
			}
		}

		/** Totals and letters rise from the leaves; order is fixed on the way down. */
		const rollUp = (nodes: TreeNode[]) => {
			for (const node of nodes) {
				if (node.kind === "file") continue;
				rollUp(node.children);
				node.transactions = node.children.reduce((sum, child) => sum + child.transactions, 0);
				node.change = node.children.reduce<Change>(
					(strongest, child) =>
						CHANGE_ORDER[child.change] > CHANGE_ORDER[strongest] ? child.change : strongest,
					"unchanged"
				);
			}
			nodes.sort((left, right) =>
				left.kind === right.kind
					? left.name.localeCompare(right.name)
					: left.kind === "folder"
						? -1
						: 1
			);
		};
		rollUp(roots);
		return roots;
	}

	const tree = $derived(buildTree(files));

	/** Folded folders by path; absent means open, so everything starts open. */
	let folded = $state<Record<string, boolean>>({});

	const isOpen = (path: string) => folded[path] !== true;

	function toggle(path: string) {
		folded[path] = isOpen(path);
	}

	/** The file the route is showing, decoded from `/files/<path>`. */
	const activePath = $derived.by(() => {
		const prefix = "/files/";
		if (!page.url.pathname.startsWith(prefix)) return "";
		try {
			return decodeURIComponent(page.url.pathname.slice(prefix.length));
		} catch {
			return "";
		}
	});

	$effect(() => {
		// Reaching a file — by a link, a back button, or a typed URL — opens
		// every folder on its way down. Only `activePath` is read here; `folded`
		// is written, so a folder the reader closes stays closed until the route
		// names a file under it again.
		const active = activePath;
		if (!active) return;
		let prefix = "";
		for (const segment of active.split("/")) {
			if (!segment) continue;
			prefix = `${prefix}/${segment}`;
			if (prefix !== active) folded[prefix] = false;
		}
	});

	type Row = { node: TreeNode; depth: number };

	function flatten(nodes: TreeNode[], depth: number, rows: Row[] = []): Row[] {
		for (const node of nodes) {
			rows.push({ node, depth });
			if (node.kind === "folder" && isOpen(node.path)) flatten(node.children, depth + 1, rows);
		}
		return rows;
	}

	const rows = $derived(flatten(tree, 0));

	const linkTo = (path: string) => `/files/${encodeURIComponent(path)}`;

	const rowClass = $derived(
		cn(
			"flex w-full min-w-0 items-center gap-2 text-left outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring",
			compact
				? "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground h-7 rounded-none px-1.5 text-xs"
				: "hover:bg-accent hover:text-accent-foreground h-8 rounded-md px-1.5 text-sm"
		)
	);

	const currentClass = $derived(
		compact
			? "bg-sidebar-accent text-sidebar-accent-foreground font-medium"
			: "bg-accent text-accent-foreground font-medium"
	);
</script>

<ul
	class={cn("flex w-full flex-col gap-px", className)}
	data-slot="file-tree"
	aria-label="Journal files"
>
	{#each rows as row (row.node.path)}
		<!--
			`data-path` is the row's identity: tests and future callers address a
			row by the path it stands for, not by a name two folders can share.
		-->
		<li data-path={row.node.path} data-kind={row.node.kind}>
			{#if row.node.kind === "folder"}
				<button
					type="button"
					class={rowClass}
					style="padding-left: {6 + row.depth * 14}px"
					aria-expanded={isOpen(row.node.path)}
					title={row.node.path}
					onclick={() => toggle(row.node.path)}
				>
					<HugeiconsIcon
						icon={ChevronRightIcon}
						strokeWidth={2}
						data-icon="inline-start"
						class={cn(
							"transition-motion-fast size-4 shrink-0 transition-transform",
							isOpen(row.node.path) ? "rotate-90" : ""
						)}
					/>
					<span class="min-w-0 flex-1 truncate">{row.node.name}</span>
					<!-- The letter is absent when nothing changed; the slot keeps the rows aligned. -->
					<span class="flex w-3 shrink-0 justify-center">
						<GitStatus change={row.node.change} />
					</span>
					<span
						class="text-muted-foreground shrink-0 text-xs tabular-nums"
						data-slot="file-tree-count">{row.node.transactions}</span
					>
				</button>
			{:else}
				{@const current = row.node.path === activePath}
				<a
					href={linkTo(row.node.path)}
					class={cn(rowClass, current && currentClass)}
					style="padding-left: {6 + row.depth * 14}px"
					aria-current={current ? "page" : undefined}
					title={row.node.path}
				>
					<!-- Same width as a folder's chevron, so a leaf's name lines up with its siblings'. -->
					<span class="size-4 shrink-0" aria-hidden="true"></span>
					<span class="min-w-0 flex-1 truncate">{row.node.name}</span>
					<span class="flex w-3 shrink-0 justify-center">
						<GitStatus change={row.node.change} />
					</span>
					<span
						class="text-muted-foreground shrink-0 text-xs tabular-nums"
						data-slot="file-tree-count">{row.node.transactions}</span
					>
				</a>
			{/if}
		</li>
	{/each}
</ul>
