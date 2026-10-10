// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { AccountBalance } from "../bindings";

export type AccountNode = {
	/** Last path segment, e.g. "Checking" */
	name: string;
	/** Full Beancount account name, e.g. "Assets:Bank:Checking" */
	fullName: string;
	/** Own balance for leaves; subtree sum for parents */
	balance: number;
	children: AccountNode[];
};

export type AccountSection = {
	label: string;
	/** Top-level account names (e.g. "Assets") in this section. Omit for a catch-all of everything else. */
	roots?: string[];
	/** Render only the section total instead of the account tree. */
	sumOnly?: boolean;
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

/** Builds the colon-path tree; leaves carry their balance, parents sum their subtree. */
export function buildAccountTree(accounts: AccountBalance[]): AccountNode[] {
	const root: AccountNode = { name: "", fullName: "", balance: 0, children: [] };
	const byName = new Map<string, AccountNode>();
	for (const { name, balance } of [...accounts].sort((a, b) => a.name.localeCompare(b.name))) {
		let path = "";
		let node = root;
		for (const part of name.split(":")) {
			path = path ? `${path}:${part}` : part;
			let child = byName.get(path);
			if (!child) {
				child = { name: part, fullName: path, balance: 0, children: [] };
				byName.set(path, child);
				node.children.push(child);
			}
			node = child;
		}
		// bindings type is `number | null` (specta f64 quirk; also None for parents)
		node.balance += balance ?? 0;
	}
	const sumSubtree = (node: AccountNode): number => {
		node.balance = node.children.reduce((acc, child) => acc + sumSubtree(child), node.balance);
		return node.balance;
	};
	for (const child of root.children) sumSubtree(child);
	const rank = (name: string) => {
		const index = TOP_LEVEL_ORDER.indexOf(name);
		return index === -1 ? TOP_LEVEL_ORDER.length : index;
	};
	root.children.sort((a, b) => rank(a.name) - rank(b.name) || a.name.localeCompare(b.name));
	return root.children;
}

/** Route path for an account page; the full name is one URL segment. */
export function accountHref(fullName: string): string {
	return `/account/${encodeURIComponent(fullName)}`;
}

const amountFormat = new Intl.NumberFormat(undefined, {
	signDisplay: "exceptZero",
	minimumFractionDigits: 2,
	maximumFractionDigits: 2,
});

export function formatAmount(value: number): string {
	return amountFormat.format(value);
}

/** Green for positive, red for negative, muted for zero. */
/** Invert income/expense display here later (e.g. multiply by -1 per section). */
export function amountColorClass(value: number): string {
	if (value > 0) return "text-emerald-600 dark:text-emerald-400";
	if (value < 0) return "text-red-600 dark:text-red-400";
	return "text-muted-foreground";
}
