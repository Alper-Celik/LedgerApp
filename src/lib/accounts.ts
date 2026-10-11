// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { AccountBalance } from "../bindings";
import { signOf } from "./money.js";

export type AccountNode = {
	/** Last path segment, e.g. "Checking" */
	name: string;
	/** Full Beancount account name, e.g. "Assets:Bank:Checking" */
	fullName: string;
	/** Balance from the backend (exact decimal string + its commodity); null
	 *  for tree intermediate nodes the backend did not report */
	balance: { units: string; currency: string } | null;
	children: AccountNode[];
};

export type AccountSection = {
	label: string;
	/** Top-level account names (e.g. "Assets") in this section. Omit for a catch-all of everything else. */
	roots?: string[];
};

/** Label of the whole ledger: the sidebar's virtual root row and the `/ledger` title. */
export const LEDGER_LABEL = "All accounts";

/**
 * Sidebar layout. Edit here (or load from settings later) to change which
 * sections appear, their order, and whether they collapse to a single total.
 * Currently one tree of all accounts.
 */
export const ACCOUNT_SECTIONS: AccountSection[] = [{ label: LEDGER_LABEL }];

/** Top-level accounts pinned before the rest (rest stays alphabetical). */
export const TOP_LEVEL_ORDER: string[] = ["Assets", "Liabilities"];

/** Accounts belonging to a section; the section without `roots` catches the rest. */
export function accountsForSection(
	accounts: AccountBalance[],
	section: AccountSection,
	sections: AccountSection[]
): AccountBalance[] {
	if (section.roots) {
		return accounts.filter((a) => section.roots!.includes(a.name.split(":")[0]));
	}
	const claimed = new Set(sections.flatMap((s) => s.roots ?? []));
	return accounts.filter((a) => !claimed.has(a.name.split(":")[0]));
}

/** Builds the colon-path tree; every node carries its own backend balance. */
export function buildAccountTree(accounts: AccountBalance[]): AccountNode[] {
	const root: AccountNode = { name: "", fullName: "", balance: null, children: [] };
	const byName = new Map<string, AccountNode>();
	for (const account of [...accounts].sort((a, b) => a.name.localeCompare(b.name))) {
		let path = "";
		let node = root;
		for (const part of account.name.split(":")) {
			path = path ? `${path}:${part}` : part;
			let child = byName.get(path);
			if (!child) {
				child = { name: part, fullName: path, balance: null, children: [] };
				byName.set(path, child);
				node.children.push(child);
			}
			node = child;
		}
		node.balance = { units: account.balance, currency: account.currency };
	}
	const rank = (name: string) => {
		const index = TOP_LEVEL_ORDER.indexOf(name);
		return index === -1 ? TOP_LEVEL_ORDER.length : index;
	};
	root.children.sort((a, b) => rank(a.name) - rank(b.name) || a.name.localeCompare(b.name));
	return root.children;
}

/**
 * True when `account` is `ancestor` itself or a sub-account of it. An empty
 * `ancestor` is the virtual root, which contains every account.
 */
export function isSubAccountOf(account: string, ancestor: string): boolean {
	if (ancestor === "") return true;
	return account === ancestor || account.startsWith(`${ancestor}:`);
}

/** Green for positive, red for negative, muted for zero. */
/** Invert income/expense display here later (e.g. multiply by -1 per section). */
export function amountColorClass(units: string | null): string {
	const sign = units === null ? 0 : signOf(units);
	if (sign > 0) return "text-emerald-600 dark:text-emerald-400";
	if (sign < 0) return "text-red-600 dark:text-red-400";
	return "text-muted-foreground";
}
