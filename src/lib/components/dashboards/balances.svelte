<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { buildAccountTree, type AccountNode } from "$lib/accounts.js";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import type { LedgerData } from "$lib/dashboards.js";
	import { signOf } from "$lib/money.js";
	import Panel from "./panel.svelte";

	let { data }: { data: LedgerData } = $props();

	/** How many accounts the list shows before it stops being a glance. */
	const LARGEST = 8;

	type HeldAccount = AccountNode & { balance: { units: string; currency: string } };

	/**
	 * The ledger as a tree. The backend reports every ancestor's whole-subtree
	 * balance, so the top level of this tree is the total per section — Assets,
	 * Liabilities, then the rest, in the sidebar's own order.
	 */
	const totals = $derived(buildAccountTree(data.accounts));

	/**
	 * The accounts that hold something themselves. An ancestor's balance is its
	 * whole subtree's, so listing it beside its children would count the same
	 * money twice; a leaf is where money actually sits, and one sitting at zero
	 * has nothing to say about where it is.
	 */
	const holders = $derived(
		leavesOf(totals).filter(
			(node): node is HeldAccount => node.balance !== null && signOf(node.balance.units) !== 0
		)
	);

	const largest = $derived([...holders].sort(biggerFirst).slice(0, LARGEST));

	const hint = $derived(
		holders.length > largest.length
			? `the ${largest.length} largest of ${holders.length} accounts`
			: `${holders.length} accounts`
	);

	/** Leaf accounts of the tree: the nodes that carry their own postings. */
	function leavesOf(nodes: AccountNode[]): AccountNode[] {
		const found: AccountNode[] = [];
		for (const node of nodes) {
			if (node.children.length === 0) found.push(node);
			else found.push(...leavesOf(node.children));
		}
		return found;
	}

	/** Biggest first, on the digits — see `compareMagnitudes`. */
	function biggerFirst(a: HeldAccount, b: HeldAccount): number {
		return compareMagnitudes(b.balance.units, a.balance.units);
	}

	/**
	 * Orders two decimals by magnitude, on their digits: the signs are dropped (a
	 * liability of 500 is as large as an asset of 500), the fractions are padded
	 * to a common scale, and the digit runs then compare as numbers do. Money
	 * never passes through a float, not even to decide which account is larger.
	 */
	function compareMagnitudes(a: string, b: string): number {
		const [wholeA, fractionA] = splitDecimal(a);
		const [wholeB, fractionB] = splitDecimal(b);
		const scale = Math.max(fractionA.length, fractionB.length);
		return compareDigits(
			wholeA + fractionA.padEnd(scale, "0"),
			wholeB + fractionB.padEnd(scale, "0")
		);
	}

	/** Whole and fraction digits of a decimal string, sign dropped. */
	function splitDecimal(units: string): [string, string] {
		const [whole = "0", fraction = ""] = units.trim().replace("-", "").split(".");
		return [whole, fraction];
	}

	/** Equal-length digit runs compare lexicographically; a longer run, leading zeros gone, is the larger number. */
	function compareDigits(a: string, b: string): number {
		const left = a.replace(/^0+/, "");
		const right = b.replace(/^0+/, "");
		if (left.length !== right.length) return left.length - right.length;
		return left < right ? -1 : left > right ? 1 : 0;
	}
</script>

{#if data.accounts.length === 0}
	<p class="text-muted-foreground text-sm">
		The ledger has no accounts yet, so there is nothing to total.
	</p>
{:else}
	<div class="flex flex-col gap-4">
		<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
			{#each totals as node (node.fullName)}
				<Panel title={node.fullName} hint="subtree total">
					{#if node.balance}
						<MoneyAmount
							units={node.balance.units}
							currency={node.balance.currency}
							class="text-base font-medium"
						/>
					{:else}
						<!--
							An ancestor the backend did not report a balance for:
							its children are totalled below the tree, not here.
						-->
						<span class="text-muted-foreground text-sm">—</span>
					{/if}
				</Panel>
			{/each}
		</div>

		<Panel title="Largest accounts" {hint}>
			{#if largest.length === 0}
				<p class="text-muted-foreground text-xs">Every account stands at zero.</p>
			{:else}
				<ul class="flex flex-col">
					{#each largest as node (node.fullName)}
						<li
							class="flex items-baseline justify-between gap-3 border-b border-border/60 py-1 last:border-b-0"
						>
							<span class="min-w-0 truncate text-xs" title={node.fullName}>{node.fullName}</span>
							<MoneyAmount
								units={node.balance.units}
								currency={node.balance.currency}
								class="text-xs"
							/>
						</li>
					{/each}
				</ul>
			{/if}
		</Panel>
	</div>
{/if}
