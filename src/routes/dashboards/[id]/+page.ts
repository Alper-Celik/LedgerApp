// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import {
	prepareWidgets,
	reportWindow,
	type DashboardData,
	type Loaded,
} from "$lib/components/dashboards/data.js";
import { messageOf } from "$lib/errors.js";
import {
	commands,
	type AccountBalance,
	type BudgetRange,
	type CashflowMonth,
	type CategoryMonths,
	type Dashboard,
	type NetWorthMonth,
} from "../../../bindings";
import type { PageLoad } from "./$types";

/** What a fallible command answers, before the failure is turned into data. */
type Answer<T> = { status: "ok"; data: T } | { status: "error"; error: string };

/** The ledger's account balances, or the reason they could not be read. */
async function readAccounts(): Promise<Loaded<AccountBalance[]>> {
	try {
		return { ok: true, data: await commands.accounts() };
	} catch (cause) {
		console.error("dashboard accounts failed", cause);
		return { ok: false, error: messageOf(cause) };
	}
}

/**
 * One month report over `months` months, or the reason it could not be read.
 *
 * The three reports are one call each for the page, over the window the
 * dashboard's widgets ask for — never a walk of the whole register.
 */
async function readReport<T>(
	label: string,
	call: () => Promise<Answer<T[]>>
): Promise<Loaded<T[]>> {
	try {
		const answer = await call();
		return answer.status === "ok"
			? { ok: true, data: answer.data }
			: { ok: false, error: answer.error };
	} catch (cause) {
		console.error(`dashboard ${label} report failed`, cause);
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

/** A source nothing on this dashboard needs, so none of it was read. */
function unread<T>(): Loaded<T[]> {
	const data: T[] = [];
	return { ok: true, data };
}

/**
 * One dashboard and the sources its widgets read.
 *
 * The dashboard is read first, because its widgets decide the report window;
 * then every source is read together, once for the page: a grid of six widgets
 * must not ask the ledger six times, and no source is a register dump. Each
 * source keeps its own failure value so one broken command leaves the other
 * widgets drawing — the page reports a store failure above the grid, and a
 * widget reports the source it needed, with the backend's message as it came.
 */
export const load: PageLoad = async ({ params }) => {
	const id = params.id ?? "";
	const listed = await readDashboards();
	const dashboard = listed.ok ? (listed.data.find((one) => one.id === id) ?? null) : null;
	// Each report is read once for the widest window any widget on the page asks
	// for; a widget then slices its own, shorter window off the end.
	const months = dashboard ? reportWindow(prepareWidgets(dashboard)) : 0;
	const [accounts, netWorth, cashflow, categories, budget] = await Promise.all([
		readAccounts(),
		months > 0
			? readReport("net worth", () => commands.reportNetWorth(months))
			: unread<NetWorthMonth>(),
		months > 0
			? readReport("cash flow", () => commands.reportCashflow(months))
			: unread<CashflowMonth>(),
		months > 0
			? readReport("categories", () => commands.reportCategories(months))
			: unread<CategoryMonths>(),
		readBudget(),
	]);

	return {
		id,
		dashboard,
		loadError: listed.ok ? "" : listed.error,
		sources: { accounts, netWorth, cashflow, categories, budget } satisfies DashboardData,
	};
};
