// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { BudgetSettings, EnvelopeSetting } from "../bindings";
import type { BudgetRow } from "./budget-grid.js";

/** The namespace the envelope plugin mirrors the ledger's accounts into. */
export const ENVELOPE_NAMESPACE = "Equity:Envelope:";

/**
 * The category path an emitted envelope carries: the plugin mirrors
 * `Expenses:Food:Groceries` as `Equity:Envelope:Food:Groceries`, so the part
 * after the namespace is the expense account with its root removed. A row that
 * is not in the namespace is returned unchanged; it can then only match a
 * setting by its whole account name.
 */
export function envelopePath(fullName: string): string {
	return fullName.startsWith(ENVELOPE_NAMESPACE)
		? fullName.slice(ENVELOPE_NAMESPACE.length)
		: fullName;
}

/**
 * The expense root the configured accounts suggest, used to spell an account
 * that has no `fund`/`rollover` directive yet. The plugin strips the ledger's
 * expense root when it mirrors an account; the settings only ever name expense
 * accounts, so their commonest first component is the root. `Expenses` is the
 * beancount default, and the last resort for a budget with no settings at all.
 */
export function expenseRoot(settings: BudgetSettings | null): string {
	const counts = new Map<string, number>();
	for (const envelope of settings?.envelopes ?? []) {
		const colon = envelope.account.indexOf(":");
		if (colon <= 0) continue;
		const root = envelope.account.slice(0, colon);
		counts.set(root, (counts.get(root) ?? 0) + 1);
	}
	let root = "Expenses";
	let count = 0;
	for (const [candidate, seen] of counts) {
		if (seen > count) {
			root = candidate;
			count = seen;
		}
	}
	return root;
}

/**
 * The configured envelope behind a grid row, joined on the account name: the
 * emitted row names the plugin's mirrored account (`Equity:Envelope:Food:…`),
 * while a `fund`/`rollover` directive names the ledger account
 * (`Expenses:Food:…`). Both reduce to the same category path, which is what is
 * compared here. `null` means the row has no setting yet — a category the
 * plugin saw activity in but that was never funded or given a rollover.
 */
export function envelopeSetting(
	settings: BudgetSettings | null,
	row: BudgetRow
): EnvelopeSetting | null {
	if (!settings || row.fullName === "") return null;
	const path = envelopePath(row.fullName);
	for (const setting of settings.envelopes) {
		// `Expenses:Food:Groceries` and `Equity:Envelope:Food:Groceries` reduce to
		// `Food:Groceries`; a setting whose account is exactly the path matches too.
		const colon = setting.account.indexOf(":");
		const tail = colon === -1 ? setting.account : setting.account.slice(colon + 1);
		if (setting.account === path || tail === path) return setting;
	}
	return null;
}

/**
 * The ledger account a write for this row names: the setting's own account
 * when there is one, else the plugin's mirroring rule reversed — the default
 * expense root plus the row's path. The backend validates it again; this only
 * decides which line the directive is written about.
 */
export function categoryAccount(settings: BudgetSettings | null, row: BudgetRow): string {
	if (row.fullName === "") return "";
	return (
		envelopeSetting(settings, row)?.account ??
		`${expenseRoot(settings)}:${envelopePath(row.fullName)}`
	);
}

/**
 * The file a write for this row goes into: where the account's newest
 * directive already lives, else the file the configuration block was found in.
 * Empty when neither is known, which is what disables the row's controls.
 */
export function envelopeFile(settings: BudgetSettings | null, row: BudgetRow): string {
	return envelopeSetting(settings, row)?.file || settings?.config?.file || "";
}

/** A rollover policy in a few words, for the row's control and its `aria-label`. */
export function rolloverLabel(setting: EnvelopeSetting | null, currency: string): string {
	const rollover = setting?.rollover;
	if (!rollover) return "not set";
	switch (rollover.kind) {
		case "carry":
			return "carry";
		case "reset":
			return "reset";
		case "cap": {
			const amount = rollover.amount;
			return amount ? `cap ${amount.units} ${amount.currency}` : `cap ${currency}`;
		}
		default:
			return "not set";
	}
}
