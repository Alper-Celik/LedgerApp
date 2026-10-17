<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { TransactionView } from "../../../bindings";
	import MoneyAmount from "$lib/components/money-amount.svelte";
	import * as Table from "$lib/components/ui/table/index.js";
	import type { LedgerData } from "$lib/dashboards.js";
	import { signOf, sumDecimals } from "$lib/money.js";
	import Panel from "./panel.svelte";

	let { data }: { data: LedgerData } = $props();

	/** How much the panels show before they stop being a glance. */
	const MONTHS = 12;
	const RECENT = 8;

	type Amount = { units: string; currency: string };

	type MonthRow = {
		/** `YYYY-MM`, the month the entries date into. */
		key: string;
		count: number;
		/** Legs arriving in an account, summed per commodity. */
		incoming: Amount[];
		/** Legs leaving one, summed per commodity — still negative, the colour says so. */
		outgoing: Amount[];
		/** Share of the busiest month, for the bar: counts again, no money is measured here. */
		share: number;
	};

	const allMonths = $derived(monthsOf(data.transactions));
	const months = $derived(allMonths.slice(0, MONTHS));
	const recent = $derived(data.transactions.slice(0, RECENT));

	/**
	 * One row per month, newest first: how many entries the register would show
	 * for it, and what their legs add up to.
	 *
	 * A leg is counted by its own sign — a positive posting is money arriving in
	 * its account, a negative one money leaving it — because the whole ledger's
	 * legs sum to zero per commodity, and netting them would leave every month
	 * looking as if nothing had happened. The two columns are therefore the
	 * register's own amount columns, and an entry posted in two commodities (a
	 * currency purchase) keeps a side in each instead of cancelling out.
	 */
	function monthsOf(transactions: TransactionView[]): MonthRow[] {
		type Bucket = { count: number; incoming: Map<string, string[]>; outgoing: Map<string, string[]> };
		const buckets = new Map<string, Bucket>();
		for (const tx of transactions) {
			// An ISO date slices as text; no date has to be parsed to bucket it.
			const key = tx.date.slice(0, 7);
			let bucket = buckets.get(key);
			if (!bucket) {
				bucket = { count: 0, incoming: new Map(), outgoing: new Map() };
				buckets.set(key, bucket);
			}
			bucket.count += 1;
			for (const posting of tx.postings) {
				const side = signOf(posting.units) < 0 ? bucket.outgoing : bucket.incoming;
				const legs = side.get(posting.currency);
				if (legs) legs.push(posting.units);
				else side.set(posting.currency, [posting.units]);
			}
		}
		const busiest = Math.max(1, ...[...buckets.values()].map((bucket) => bucket.count));
		return [...buckets]
			.map(([key, bucket]) => ({
				key,
				count: bucket.count,
				incoming: sums(bucket.incoming),
				outgoing: sums(bucket.outgoing),
				share: Math.round((bucket.count / busiest) * 100),
			}))
			.sort((a, b) => b.key.localeCompare(a.key));
	}

	/** Each commodity's legs as one exact sum — `sumDecimals` adds integers, not floats. */
	function sums(legs: Map<string, string[]>): Amount[] {
		return [...legs].map(([currency, units]) => ({ units: sumDecimals(units), currency }));
	}

	/** What one entry brought in: the same reading as a month's incoming column, so the two panels agree. */
	function broughtIn(tx: TransactionView): Amount[] {
		const legs = new Map<string, string[]>();
		for (const posting of tx.postings) {
			if (signOf(posting.units) < 0) continue;
			const list = legs.get(posting.currency);
			if (list) list.push(posting.units);
			else legs.set(posting.currency, [posting.units]);
		}
		return sums(legs);
	}
</script>

{#snippet amounts(values: Amount[])}
	<span class="inline-flex flex-wrap justify-end gap-x-2">
		{#each values as value, index (index)}
			<MoneyAmount units={value.units} currency={value.currency} signed={false} />
		{/each}
	</span>
{/snippet}

{#if data.transactions.length === 0}
	<p class="text-muted-foreground text-sm">
		The ledger holds no entries yet, so there is no activity to show.
	</p>
{:else}
	<div class="flex flex-col gap-4">
		<Panel
			title="By month"
			hint={allMonths.length > months.length
				? `the last ${months.length} of ${allMonths.length} months`
				: `${allMonths.length} months`}
		>
			<Table.Root>
				<Table.Header>
					<Table.Row>
						<Table.Head>Month</Table.Head>
						<Table.Head>Entries</Table.Head>
						<!-- The bars carry no text, so the column is named here. -->
						<Table.Head class="w-40"><span class="sr-only">Share of entries</span></Table.Head>
						<Table.Head class="text-right">In</Table.Head>
						<Table.Head class="pr-3 text-right">Out</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each months as month (month.key)}
						<Table.Row>
							<Table.Cell class="tabular-nums">{month.key}</Table.Cell>
							<Table.Cell class="tabular-nums">{month.count}</Table.Cell>
							<Table.Cell>
								<!-- The bar repeats the count as a length; it is never the only place a number is read. -->
								<div class="h-2 w-full overflow-hidden rounded-full bg-muted" aria-hidden="true">
									<div class="h-full rounded-full bg-primary/60" style:width="{month.share}%"></div>
								</div>
							</Table.Cell>
							<Table.Cell class="text-right">{@render amounts(month.incoming)}</Table.Cell>
							<Table.Cell class="pr-3 text-right">{@render amounts(month.outgoing)}</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
			<p class="text-muted-foreground text-xs">
				In is the legs that arrive in an account, Out the legs that leave one; each column keeps the
				commodities its entries were posted in.
			</p>
		</Panel>

		<Panel title="Latest entries" hint="the register's newest rows, by what they brought in">
			<ul class="flex flex-col">
				{#each recent as tx (tx.id)}
					<li
						class="flex items-baseline justify-between gap-3 border-b border-border/60 py-1 last:border-b-0"
					>
						<span class="flex min-w-0 items-baseline gap-2">
							<span class="text-muted-foreground text-xs tabular-nums">{tx.date}</span>
							<span class="truncate text-xs">
								{#if tx.payee && tx.payee !== tx.narration}
									<span class="font-medium">{tx.payee}</span>
									<span class="text-muted-foreground">— {tx.narration}</span>
								{:else}
									{tx.narration}
								{/if}
							</span>
						</span>
						{@render amounts(broughtIn(tx))}
					</li>
				{/each}
			</ul>
		</Panel>
	</div>
{/if}
