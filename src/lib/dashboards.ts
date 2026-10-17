// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { IconSvgElement } from "@hugeicons/svelte";
import { Activity01Icon, Wallet01Icon } from "@hugeicons/core-free-icons";
import type { Component } from "svelte";
import { commands, type AccountBalance, type TransactionView } from "../bindings";
import ActivityDashboard from "./components/dashboards/activity.svelte";
import BalancesDashboard from "./components/dashboards/balances.svelte";

/**
 * The dashboards, in the order the list page shows them.
 *
 * Adding one is one entry here plus the single component it names. This array
 * is the only place that knows a dashboard exists: `/dashboards` lists whatever
 * is in it, and `/dashboards/<id>` renders that entry's `component` — neither
 * route has a case for any particular dashboard, and a third one needs no edit
 * to either.
 *
 * Data comes from the entry's `load`. Left out, the dashboard is handed
 * `ledgerData()`: the ledger's accounts and all of its transactions, which is
 * everything the two dashboards below read. A dashboard with its own data
 * source — another command, another shape — names its own `load` and types the
 * data it answers with; the route runs it and passes the result to `component`
 * as the `data` prop, knowing nothing else about it.
 */
export const dashboards: readonly DashboardEntry[] = [
	{
		id: "balances",
		title: "Balances",
		blurb: "Where the money is: the top-level totals, then the largest accounts.",
		icon: Wallet01Icon,
		component: BalancesDashboard,
	},
	{
		id: "activity",
		title: "Activity",
		blurb: "What happened: entries per month with the money they moved, and the newest entries.",
		icon: Activity01Icon,
		component: ActivityDashboard,
	},
];

/**
 * What the shared loader hands a dashboard: the ledger's accounts (each with
 * its own subtree balance) and its transactions, newest first.
 */
export type LedgerData = {
	accounts: AccountBalance[];
	transactions: TransactionView[];
};

/**
 * The ledger, read once, for a dashboard that names no `load` of its own.
 *
 * Both commands are asked together, and a failure is left to throw: the route
 * turns it into an Alert, which reads as "the ledger cannot be read" rather
 * than as a broken page.
 */
export async function ledgerData(): Promise<LedgerData> {
	const [accounts, transactions] = await Promise.all([
		commands.accounts(),
		commands.accountTransactions(""),
	]);
	return { accounts, transactions };
}

/**
 * One dashboard. `Data` is what its `load` answers and what its component reads
 * back off `data`; the registry erases it, because a route holds entries of
 * every shape and cannot know which one it will render — so each dashboard
 * types `Data` in its own component, and the registry sees the default.
 */
export type DashboardEntry<Data = any> = {
	/** Route segment: `/dashboards/<id>`. */
	id: string;
	title: string;
	/** One line, shown on the card in the list and under the title on the page. */
	blurb: string;
	icon: IconSvgElement;
	component: Component<{ data: Data }>;
	/** Where this dashboard's data comes from. Omitted: `ledgerData`. */
	load?: () => Promise<Data>;
};
