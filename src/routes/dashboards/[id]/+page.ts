// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { DashboardData, Loaded } from "$lib/components/dashboards/data.js";
import { messageOf } from "$lib/errors.js";
import {
	commands,
	type AccountBalance,
	type BudgetRange,
	type Dashboard,
	type TransactionView,
} from "../../../bindings";
import type { PageLoad } from "./$types";

/** The ledger's account balances, or the reason they could not be read. */
async function readAccounts(): Promise<Loaded<AccountBalance[]>> {
	try {
		return { ok: true, data: await commands.accounts() };
	} catch (cause) {
		console.error("dashboard accounts failed", cause);
		return { ok: false, error: messageOf(cause) };
	}
}

/** The whole ledger's transactions, or the reason they could not be read. */
async function readTransactions(): Promise<Loaded<TransactionView[]>> {
	try {
		return { ok: true, data: await commands.accountTransactions("") };
	} catch (cause) {
		console.error("dashboard transactions failed", cause);
		return { ok: false, error: messageOf(cause) };
	}
}

/** The newest budgeted months, or the reason the budget could not be read. */
async function readBudget(): Promise<Loaded<BudgetRange>> {
	try {
		const result = await commands.budgetRange(null, null);
		return result.status === "ok"
			? { ok: true, data: result.data }
			: { ok: false, error: result.error };
	} catch (cause) {
		console.error("dashboard budget failed", cause);
		return { ok: false, error: messageOf(cause) };
	}
}

/** The saved dashboards, or the reason the store could not be read. */
async function readDashboards(): Promise<Loaded<Dashboard[]>> {
	try {
		const result = await commands.dashboards();
		return result.status === "ok"
			? { ok: true, data: result.data }
			: { ok: false, error: result.error };
	} catch (cause) {
		console.error("dashboard store failed", cause);
		return { ok: false, error: messageOf(cause) };
	}
}

/**
 * One dashboard and the three sources its widgets read.
 *
 * All four reads happen together, once for the page: a grid of six widgets must
 * not ask the ledger six times. Each source keeps its own failure value so one
 * broken command leaves the other widgets drawing — the page reports a store
 * failure above the grid, and a widget reports the source it needed.
 */
export const load: PageLoad = async ({ params }) => {
	const id = params.id ?? "";
	const [listed, accounts, transactions, budget] = await Promise.all([
		readDashboards(),
		readAccounts(),
		readTransactions(),
		readBudget(),
	]);
	const dashboard = listed.ok ? (listed.data.find((one) => one.id === id) ?? null) : null;

	return {
		id,
		dashboard,
		loadError: listed.ok ? "" : listed.error,
		sources: { accounts, transactions, budget } satisfies DashboardData,
	};
};
