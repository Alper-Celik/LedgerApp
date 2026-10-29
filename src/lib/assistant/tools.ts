// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The tools the panel runs itself.
 *
 * The model only ever sees JSON: it names a tool, this module validates the
 * arguments and calls the matching backend command, and the command's answer
 * goes back as JSON text. Nothing here computes a ledger figure — every number
 * in an answer is one of these results, verbatim.
 *
 * The names map onto the commands the app has: accounts, the register, the
 * journal's files, a file's text, the budget, the stored index series, the
 * lifestyle report, the price targets and BQL. `query` is called through
 * `invoke` by its Tauri name until it reaches `src/bindings.ts`, the way
 * `api.ts` calls the `ai_*` commands. Names are accepted in the endpoint's own
 * spelling: snake_case, camelCase, or with a `ledger`/namespace prefix. An
 * unknown name is answered as an error the model can read and correct, never
 * thrown.
 */
import { invoke } from "@tauri-apps/api/core";
import { commands } from "../../bindings";
import { messageOf } from "../errors.js";
import type { ToolCall } from "./api.js";

export type DispatchOutcome = {
	/** Whether the tool actually ran. False whenever `result` is `{ error }`. */
	ok: boolean;
	/** The JSON text handed back to the model, and shown in the card's details. */
	text: string;
	/** The parsed answer, for the card's row count and details. */
	result: unknown;
};

/**
 * The most JSON text one tool answer may send back to the model. A result past
 * this is refused with a sentence the model can act on ("ask for a narrower
 * range") rather than silently truncated into wrong figures.
 */
const MAX_TEXT = 80_000;

/** Aliases an endpoint or a schema may use for the same command. */
const ALIASES: Record<string, string> = {
	account_balances: "accounts",
	list_accounts: "accounts",
	balance: "accounts",
	transactions: "account_transactions",
	account_activity: "account_transactions",
	files: "journal_files",
	list_files: "journal_files",
	read_file: "file_text",
	file: "file_text",
	budget: "budget_range",
	budget_months: "budget_range",
	indexes: "index_series",
	index: "index_series",
	lifestyle: "lifestyle_report",
	prices: "price_targets",
	bql: "query",
	sql: "query",
	run_query: "query",
};

/** The tool names this panel can answer, for the unknown-name error. */
const NAMES = [
	"accounts",
	"account_transactions",
	"journal_files",
	"file_text",
	"budget_range",
	"index_series",
	"lifestyle_report",
	"price_targets",
	"query",
];

/**
 * How many rows a `query` asks for when the model does not say. The answer says
 * whether the cap cut anything, so a wider read is one explicit call away.
 */
const DEFAULT_QUERY_ROWS = 200;

/** A BQL answer: the column names and every row as the strings the engine produced. */
type QueryAnswer = {
	columns: string[];
	rows: string[][];
	truncated: boolean;
};

/**
 * One BQL statement through the backend's query engine.
 *
 * `query` is not in `src/bindings.ts` yet — the command layer that registers it
 * is being added separately — so the call goes through `invoke` by its Tauri
 * name; the row cap is camelCased from the Rust `max_rows` argument. A refused
 * statement rejects with the engine's own message, which the dispatcher hands
 * to the model as it came.
 */
function queryRows(bql: string, maxRows: number): Promise<QueryAnswer> {
	return invoke<QueryAnswer>("query", { bql, maxRows });
}

/** Run one call from the model. */
export async function dispatchTool(call: ToolCall): Promise<DispatchOutcome> {
	let args: Record<string, unknown>;
	try {
		args = parseArguments(call.arguments);
	} catch (cause) {
		return failure(`the arguments of \`${call.name}\` could not be read: ${messageOf(cause)}`);
	}

	try {
		switch (canonical(call.name)) {
			case "accounts":
				return success(await commands.accounts());

			case "account_transactions": {
				const account = stringArg(args, "account");
				return success(await commands.accountTransactions(account));
			}

			case "journal_files":
				return success(await commands.journalFiles());

			case "file_text": {
				const path = stringArg(args, "path", "file");
				return success(await unwrap(commands.fileText(path)));
			}

			case "budget_range": {
				const from = optionalStringArg(args, "from");
				const to = optionalStringArg(args, "to");
				return success(await unwrap(commands.budgetRange(from, to)));
			}

			case "index_series":
				return success(await unwrap(commands.indexSeries()));

			case "lifestyle_report": {
				const index = stringArg(args, "index").trim();
				if (!index) {
					throw new Error(
						"`index` must name a stored series; call `index_series` first to list them"
					);
				}
				const months = optionalPositiveInt(args, "months") ?? 12;
				return success(await unwrap(commands.lifestyleReport(months, index)));
			}

			case "price_targets":
				return success(await commands.priceTargets());

			case "query": {
				const bql = stringArg(args, "bql", "query").trim();
				if (!bql) {
					throw new Error(
						"`bql` must be a statement, e.g. `SELECT account, sum(position) WHERE account ~ 'Expenses'`"
					);
				}
				const maxRows =
					optionalPositiveInt(args, "max_rows") ??
					optionalPositiveInt(args, "maxRows") ??
					DEFAULT_QUERY_ROWS;
				return success(await queryRows(bql, maxRows));
			}

			default:
				throw new Error(
					`unknown tool \`${call.name}\`; this panel can run ${NAMES.map((name) => `\`${name}\``).join(", ")}`
				);
		}
	} catch (cause) {
		return failure(messageOf(cause));
	}
}

/**
 * The one-line result summary the tool card shows, e.g. `12 rows`.
 *
 * Arrays count themselves; an object counts the first array-valued field a
 * command returns (a register's `rows`, a budget's `months`, a series' `points`)
 * and otherwise counts its fields.
 */
export function describeResult(result: unknown): string {
	if (Array.isArray(result)) return rows(result.length);
	if (result && typeof result === "object") {
		const record = result as Record<string, unknown>;
		for (const key of [
			"rows",
			"transactions",
			"accounts",
			"postings",
			"sessions",
			"months",
			"points",
			"series",
			"categories",
			"groups",
			"envelopes",
			"targets",
			"quotes",
			"files",
			"items",
			"data",
		]) {
			const value = record[key];
			if (Array.isArray(value)) return rows(value.length);
		}
		const fields = Object.keys(record).length;
		return fields === 1 ? "1 field" : `${fields} fields`;
	}
	if (result === null || result === undefined) return "nothing";
	return "1 value";
}

/** Split an endpoint's tool name into the command this panel knows. */
function canonical(name: string): string {
	let key = name.trim().toLowerCase();
	const namespace = Math.max(key.lastIndexOf("."), key.lastIndexOf("/"));
	if (namespace >= 0) key = key.slice(namespace + 1);
	key = key.replace(/^ledger_/, "").replace(/[\s-]+/g, "_");
	return ALIASES[key] ?? key;
}

function parseArguments(text: string): Record<string, unknown> {
	if (!text.trim()) return {};
	const value: unknown = JSON.parse(text);
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new Error("the arguments must be a JSON object");
	}
	return value as Record<string, unknown>;
}

function stringArg(args: Record<string, unknown>, key: string, alternate?: string): string {
	const value = args[key] ?? (alternate ? args[alternate] : undefined);
	if (typeof value !== "string") {
		throw new Error(`\`${key}\` must be a string${alternate ? ` (or \`${alternate}\`)` : ""}`);
	}
	return value;
}

function optionalStringArg(args: Record<string, unknown>, key: string): string | null {
	const value = args[key];
	if (value === undefined || value === null) return null;
	if (typeof value !== "string") throw new Error(`\`${key}\` must be a string when given`);
	return value.trim() === "" ? null : value;
}

function optionalPositiveInt(args: Record<string, unknown>, key: string): number | null {
	const value = args[key];
	if (value === undefined || value === null) return null;
	if (
		typeof value !== "number" ||
		!Number.isInteger(value) ||
		value <= 0 ||
		value > 1200
	) {
		throw new Error(`\`${key}\` must be a whole number between 1 and 1200`);
	}
	return value;
}

/** Unwrap the `typedError` envelope every fallible command returns. */
async function unwrap<T>(
	result: Promise<{ status: "ok"; data: T } | { status: "error"; error: string }>
): Promise<T> {
	const answer = await result;
	if (answer.status === "error") throw new Error(answer.error);
	return answer.data;
}

function success(result: unknown): DispatchOutcome {
	let text: string;
	try {
		text = JSON.stringify(result) ?? "null";
	} catch (cause) {
		return failure(`the answer could not be serialised: ${messageOf(cause)}`);
	}
	if (text.length > MAX_TEXT) {
		return failure(
			`the answer is too large to send (${text.length} characters; the limit is ${MAX_TEXT}). Ask for a narrower account, month or file.`
		);
	}
	return { ok: true, result, text };
}

function failure(message: string): DispatchOutcome {
	const result = { error: message };
	return { ok: false, result, text: JSON.stringify(result) };
}

function rows(count: number): string {
	return count === 1 ? "1 row" : `${count} rows`;
}
