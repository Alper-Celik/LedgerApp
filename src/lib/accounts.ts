// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { AccountBalance } from "../bindings";

export type AccountNode = {
	/** Last path segment, e.g. "Checking" */
	name: string;
	/** Full Beancount account name, e.g. "Assets:Bank:Checking" */
	fullName: string;
	/** Own balance from the backend (exact decimal string); null for tree
	 *  intermediate nodes without their own postings */
	balance: string | null;
	children: AccountNode[];
};

export type AccountSection = {
	label: string;
	/** Top-level account names (e.g. "Assets") in this section. Omit for a catch-all of everything else. */
	roots?: string[];
};

/**
 * Sidebar layout. Edit here (or load from settings later) to change which
 * sections appear, their order, and whether they collapse to a single total.
 * Currently one tree of all accounts.
 */
export const ACCOUNT_SECTIONS: AccountSection[] = [{ label: "Accounts" }];

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
	for (const { name, balance } of [...accounts].sort((a, b) => a.name.localeCompare(b.name))) {
		let path = "";
		let node = root;
		for (const part of name.split(":")) {
			path = path ? `${path}:${part}` : part;
			let child = byName.get(path);
			if (!child) {
				child = { name: part, fullName: path, balance: null, children: [] };
				byName.set(path, child);
				node.children.push(child);
			}
			node = child;
		}
		node.balance = balance;
	}
	const rank = (name: string) => {
		const index = TOP_LEVEL_ORDER.indexOf(name);
		return index === -1 ? TOP_LEVEL_ORDER.length : index;
	};
	root.children.sort((a, b) => rank(a.name) - rank(b.name) || a.name.localeCompare(b.name));
	return root.children;
}

const amountFormat = new Intl.NumberFormat(undefined, {
	signDisplay: "exceptZero",
	minimumFractionDigits: 2,
	maximumFractionDigits: 2,
});

export function formatAmount(value: string | null): string {
	return value === null ? "" : amountFormat.format(Number(value));
}

/** Green for positive, red for negative, muted for zero. */
/** Invert income/expense display here later (e.g. multiply by -1 per section). */
export function amountColorClass(value: string | null): string {
	const amount = value === null ? 0 : Number(value);
	if (amount > 0) return "text-emerald-600 dark:text-emerald-400";
	if (amount < 0) return "text-red-600 dark:text-red-400";
	return "text-muted-foreground";
}
