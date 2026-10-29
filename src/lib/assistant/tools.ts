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
 * lifestyle report, the price targets and BQL — every one a generated binding
 * in `src/bindings.ts`, so a rename in the backend fails here at compile time.
 * Names are accepted in the endpoint's own spelling: snake_case, camelCase, or
 * with a `ledger`/namespace prefix. An unknown name is answered as an error the
 * model can read and correct, never thrown.
 *
 * `run_python` is the one tool that is not a backend command: it runs a script
 * in a worker (see `python/sandbox.ts`), and every `ledger.*` call that script
 * makes arrives back here as an RPC, so a sandboxed figure is read by this
 * same dispatcher and cannot disagree with a tool's.
 *
 * The two writes are different in kind. `add_transaction` and `write_file`
 * validate their arguments into a [`Proposal`] and stop there: the transcript
 * draws the exact entry or diff and the model is told the change is pending.
 * Only [`applyProposal`], called from the card's Apply button, runs the write
 * command — so a model can ask for an edit and never make one.
 */
import { invalidateAll } from "$app/navigation";
import {
	commands,
	type NewPosting,
	type NewTransaction,
	type ToolCall,
	type ToolSchema,
} from "../../bindings";
import { messageOf } from "../errors.js";
import { bumpLedgerRevision } from "../ledger-revision.svelte.js";
import type { PythonRun } from "./artifacts/types.js";
import { runPython } from "./python/sandbox.js";
import type { PythonRpcAnswer } from "./python/protocol.js";

export type DispatchOutcome = {
	/** Whether the tool actually ran: false on a refusal, or on a script that raised. */
	ok: boolean;
	/** The JSON text handed back to the model, and shown in the card's details. */
	text: string;
	/** The parsed answer, for the card's row count and details. */
	result: unknown;
	/**
	 * A write the model asked for, staged for the user. Its presence is the
	 * whole difference: the card renders it and only an explicit click calls
	 * [`applyProposal`], and the tool answer above tells the model it is pending.
	 */
	proposal?: Proposal;
};

/**
 * A change the model proposed and the user has not approved.
 *
 * It is plain data so it survives in the conversation's stored message: the
 * card draws it, [`applyProposal`] performs it, and nothing else may. A
 * transaction appends `entry`; a file write replaces `file` with `after`.
 */
export type Proposal = {
	/** The tool the model called: `add_transaction` or `write_file`. */
	tool: string;
	kind: "transaction" | "file";
	/** The journal file the write targets, as `journal_files` reported it. */
	file: string;
	/** The validated entry for a transaction proposal, `null` otherwise. */
	entry: NewTransaction | null;
	/** The file's text before and after for a whole-file write, `null` otherwise. */
	before: string | null;
	after: string | null;
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
	python: "run_python",
	run_code: "run_python",
	add_entry: "add_transaction",
	new_transaction: "add_transaction",
	create_transaction: "add_transaction",
	write_text: "write_file",
	replace_file: "write_file",
	update_file: "write_file",
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
	"run_python",
	"add_transaction",
	"write_file",
];

/**
 * The reads a sandboxed script may make. `run_python` is not one of them: a
 * script cannot start another sandbox through its own `ledger` module.
 */
const PYTHON_RPC: Record<string, true> = {
	accounts: true,
	account_transactions: true,
	journal_files: true,
	file_text: true,
	budget_range: true,
	index_series: true,
	lifestyle_report: true,
	price_targets: true,
	query: true,
};

/**
 * The tools this panel offers on its own, beside the backend's catalogue.
 *
 * `run_python` is the panel's own: the script runs in a worker here, and every
 * `ledger` call it makes comes back through [`dispatchTool`], so a sandboxed
 * figure is the same figure the model would have read from a tool.
 */
const LOCAL_TOOLS: ToolSchema[] = [
	{
		name: "run_python",
		description:
			"Run Python 3.14 in a sandboxed worker with the ledger exposed as the `ledger` module. " +
			"Every `ledger` function is a coroutine that reads through the same commands as the other tools: " +
			"`await ledger.accounts()`, `await ledger.account_transactions(account)`, `await ledger.journal_files()`, " +
			"`await ledger.file_text(path)`, `await ledger.budget_range('2026-01', '2026-06')`, " +
			"`await ledger.index_series()`, `await ledger.lifestyle_report(months, index)`, " +
			"`await ledger.price_targets()`, `await ledger.query(bql, max_rows=200)`; each answers the same JSON as the tool of that name. " +
			"`await ledger.df(bql)` answers a pandas DataFrame (the runtime and its packages ship with the app, so it works offline; `ledger.query` is faster for plain aggregates). " +
			"`ledger.table(name, data)` puts a table in the transcript (a DataFrame, a list of dicts/rows, or a dict of columns); " +
			"`ledger.chart({...})` puts one chart there: `kind` line|bar|area|donut, `currency`, and `series: [{name, points: [{label, value}]}]` with amounts as decimal strings (a donut draws the first series as its slices). " +
			"Use print() for prose; stdout, tables and the chart come back with the run. " +
			"The sandbox has no real filesystem, network, subprocess or socket access (Pyodide's `os` sees an in-memory filesystem only). " +
			"One run may take 20 s and keep 200 000 characters of output.",
		parameters: {
			type: "object",
			properties: {
				code: {
					type: "string",
					description:
						"The Python source to run. Use top-level `await` for ledger calls, e.g. `rows = await ledger.query('SELECT account, sum(position) GROUP BY account')`.",
				},
			},
			required: ["code"],
			additionalProperties: false,
		},
	},
];

/**
 * The backend's catalogue plus the panel's own tools, without duplicates: a
 * backend that learns `run_python` later wins, and the local schema is only a
 * fallback for a panel whose command list is older.
 */
export function offeredTools(remote: ToolSchema[]): ToolSchema[] {
	const names = new Set(remote.map((tool) => tool.name));
	return [...remote, ...LOCAL_TOOLS.filter((tool) => !names.has(tool.name))];
}

/** Whether a call names the sandbox, in any spelling the endpoint may use. */
export function isRunPython(name: string): boolean {
	return canonical(name) === "run_python";
}

/**
 * How many rows a `query` asks for when the model does not say. The answer says
 * whether the cap cut anything, so a wider read is one explicit call away.
 */
const DEFAULT_QUERY_ROWS = 200;

/**
 * One `ledger.*` call from the sandbox, run through this same dispatcher.
 *
 * Only the read tools are reachable: a script cannot ask for `run_python` or
 * for a name the panel does not know. The answer is the executor's own JSON
 * text and its `ok`, which `ledger.py` turns back into a Python value — or
 * raises as `LedgerError` with the app's sentence.
 */
async function ledgerRpc(name: string, argsJson: string): Promise<PythonRpcAnswer> {
	if (!PYTHON_RPC[name]) {
		return {
			ok: false,
			text: JSON.stringify({ error: `the sandbox has no \`${name}\` ledger function` }),
		};
	}
	const outcome = await dispatchTool({ id: "python", name, arguments: argsJson });
	return { ok: outcome.ok, text: outcome.text };
}

/**
 * A finished run as a tool outcome.
 *
 * `result` is the sandbox's full answer — stdout, traceback, tables, chart —
 * because that is what the transcript draws. `text`, which is what the model
 * reads, is the same answer with the bulky parts clipped to a size a context
 * window can take; a script that raised is `ok: false` so the card says
 * `error`, and its traceback goes back as text the model can act on.
 */
function runOutcome(run: PythonRun): DispatchOutcome {
	return { ok: run.error === "", result: run, text: runText(run) };
}

/** The run as the model reads it, shrunk until it fits one tool answer. */
function runText(run: PythonRun): string {
	const compact = (stdoutLimit: number, rowLimit: number) =>
		JSON.stringify({
			stdout: clip(run.stdout, stdoutLimit),
			error: run.error,
			tables: run.tables.map((table) => ({
				name: table.name,
				columns: table.columns,
				rows: table.rows.slice(0, rowLimit),
				truncated: table.truncated === true || table.rows.length > rowLimit,
			})),
			chart: run.chart,
		});

	let text = compact(60_000, 100);
	if (text.length <= MAX_TEXT) return text;
	text = compact(10_000, 20);
	if (text.length <= MAX_TEXT) return text;

	return JSON.stringify({
		stdout: clip(run.stdout, 4_000),
		error: clip(run.error, 4_000),
		tables: run.tables.map((table) => ({
			name: clip(table.name, 120),
			columns: table.columns.slice(0, 10).map((column) => clip(column, 80)),
			rows: [],
			truncated: true,
		})),
		chart: run.chart
			? {
					kind: run.chart.kind,
					currency: run.chart.currency,
					title: run.chart.title ? clip(run.chart.title, 120) : undefined,
					series: run.chart.series.slice(0, 3).map((series) => ({
						name: clip(series.name, 120),
						points: series.points.slice(0, 50).map((point) => ({
							label: clip(point.label, 120),
							value: clip(point.value, 60),
						})),
					})),
				}
			: null,
		note: "the result was too large to send in full; print less and read a narrower frame",
	});
}

/** Text cut to `limit` characters, with a note that it was cut. */
function clip(text: string, limit: number): string {
	return text.length <= limit ? text : `${text.slice(0, limit)}\n… truncated`;
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

			case "grep": {
				// The read-only file tools live in the backend's `tools` module
				// (hash-anchored reads, an ignore-aware search); they are run by
				// name so a change there needs no change here.
				const pattern = stringArg(args, "pattern");
				const glob = optionalStringArg(args, "glob");
				const json = await unwrap(
					commands.toolInvoke("grep", JSON.stringify({ pattern, glob: glob ?? undefined }))
				);
				return success(JSON.parse(json) as unknown);
			}

			case "read_lines": {
				const path = stringArg(args, "path");
				const json = await unwrap(commands.toolInvoke("read", JSON.stringify({ path })));
				return success(JSON.parse(json) as unknown);
			}

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
				return success(await unwrap(commands.query(bql, maxRows)));
			}

			case "run_python": {
				const code = stringArg(args, "code", "source");
				if (!code.trim()) throw new Error("`code` must be Python source");
				return runOutcome(await runPython(code, {}, ledgerRpc));
			}

			case "add_transaction":
				// A write is never dispatched: it is validated into a proposal the
				// card renders, and `applyProposal` runs only on a click.
				return proposed(await transactionProposal(call.name, args));

			case "write_file":
				return proposed(await fileProposal(call.name, args));

			default:
				throw new Error(
					`unknown tool \`${call.name}\`; this panel can run ${NAMES.map((name) => `\`${name}\``).join(", ")}`
				);
		}
	} catch (cause) {
		return failure(messageOf(cause));
	}
}

// -- the writes ---------------------------------------------------------------

/**
 * The sentence every proposal's tool answer repeats to the model.
 *
 * A model that reads "proposal" as "done" is the one failure this whole path
 * exists to prevent, so the answer says it in as many words.
 */
const PENDING =
	"This change is staged as a proposal for the user to approve. Nothing has been written. Do not say or imply that it was applied, and do not repeat the proposal unless the user asks for it again.";

/** `YYYY-MM-DD`, the one date shape the write command accepts. */
const DATE = /^\d{4}-\d{2}-\d{2}$/;
/** An exact decimal string, never a float. */
const DECIMAL = /^-?\d+(?:\.\d+)?$/;
/** A Beancount account: a capitalized root and at least one colon segment. */
const ACCOUNT = /^[A-Z][A-Za-z0-9-]*(?::[A-Za-z0-9-]+)+$/;
/** A commodity symbol, e.g. `USD` or `AAPL`. */
const CURRENCY = /^[A-Z][A-Z0-9'._-]*$/;

/**
 * A journal file's canonical path, from the spelling the model used.
 *
 * The write commands take a path `journal_files` reported; the model may echo
 * the label instead, so both are matched and the canonical path is what the
 * proposal carries. An unknown path is refused here with the files that do
 * exist — before anything is staged — rather than at apply time.
 */
async function knownFile(file: string): Promise<string> {
	const name = file.trim();
	if (!name) throw new Error("`file` must name a journal file; call `files` first");
	const files = await commands.journalFiles();
	const found =
		files.find((candidate) => candidate.path === name) ??
		files.find((candidate) => candidate.label === name);
	if (!found) {
		const paths = files.map((candidate) => `\`${candidate.path}\``).join(", ");
		throw new Error(
			`\`${name}\` is not a file of this journal; the files are ${paths || "none"}`
		);
	}
	return found.path;
}

/** One posting's optional string field, blank treated as absent. */
function postingText(value: unknown, position: number, field: string): string | null {
	if (value === undefined || value === null) return null;
	if (typeof value !== "string") {
		throw new Error(`posting ${position}: \`${field}\` must be a string`);
	}
	const text = value.trim();
	return text === "" ? null : text;
}

/** One posting object, validated field by field. */
function postingArg(raw: unknown, position: number): NewPosting {
	if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
		throw new Error(`posting ${position} must be an object`);
	}
	const posting = raw as Record<string, unknown>;
	const account = typeof posting.account === "string" ? posting.account.trim() : "";
	if (!ACCOUNT.test(account)) {
		throw new Error(`posting ${position}: \`${account || "?"}\` is not an account name`);
	}
	const units = postingText(posting.units, position, "units");
	const currency = postingText(posting.currency, position, "currency");
	if (units !== null && !DECIMAL.test(units)) {
		throw new Error(`posting ${position}: \`${units}\` is not an exact decimal amount`);
	}
	if (currency !== null && !CURRENCY.test(currency)) {
		throw new Error(`posting ${position}: \`${currency}\` is not a commodity`);
	}
	if (units === null && currency !== null) {
		throw new Error(`posting ${position}: a currency needs an amount to go with it`);
	}
	return { account, units, currency };
}

/** A transaction proposal's postings, refused whole if any one is wrong. */
function postingsArg(args: Record<string, unknown>, key: string): NewPosting[] {
	const value = args[key];
	if (!Array.isArray(value)) throw new Error(`\`${key}\` must be a list of postings`);
	return value.map((raw, index) => postingArg(raw, index + 1));
}

/**
 * `add_transaction`'s arguments as a proposal, validated before it is shown.
 *
 * The validation mirrors what the write command will itself refuse — a real
 * date shape, a flag beancount reads, an account name, a decimal amount — so a
 * malformed entry is a sentence the model can correct, not a card the user has
 * to reject.
 */
async function transactionProposal(
	tool: string,
	args: Record<string, unknown>
): Promise<Proposal> {
	const file = await knownFile(stringArg(args, "file"));
	const date = stringArg(args, "date").trim();
	const flag = stringArg(args, "flag").trim();
	const narration = stringArg(args, "narration").trim();
	const payee = optionalStringArg(args, "payee")?.trim() ?? "";
	const postings = postingsArg(args, "postings");

	if (!DATE.test(date)) throw new Error(`\`date\` must be YYYY-MM-DD, not \`${date}\``);
	if (flag !== "*" && flag !== "!") {
		throw new Error("`flag` must be `*` (cleared) or `!` (to review)");
	}
	if (!narration) throw new Error("`narration` must say what the entry is for");
	if (postings.length < 2) throw new Error("a transaction needs at least two postings");

	const entry: NewTransaction = {
		file,
		date,
		flag,
		payee: payee || null,
		narration,
		postings,
	};
	return { tool, kind: "transaction", file, entry, before: null, after: null };
}

/**
 * `write_file`'s arguments as a proposal: the file's current text, read through
 * the same session the editor uses, and the text the model proposes to put in
 * its place. The current text is what the card diffs against.
 */
async function fileProposal(tool: string, args: Record<string, unknown>): Promise<Proposal> {
	const file = await knownFile(stringArg(args, "path", "file"));
	const after = stringArg(args, "text", "content");
	if (!after.trim()) throw new Error("`text` must be the file's whole new text");
	const before = await unwrap(commands.fileText(file));
	if (before === after) {
		throw new Error(`the proposed text is identical to \`${file}\`, so there is nothing to write`);
	}
	return { tool, kind: "file", file, entry: null, before, after };
}

/**
 * A proposal as a tool outcome.
 *
 * `result` is the card's data (and what the conversation stores, so a reopened
 * transcript draws the same card); `text` is the model's — compact, and saying
 * in as many words that nothing was written. `ok` is true because validating
 * and staging succeeded: the proposal is pending, not an error.
 */
function proposed(proposal: Proposal): DispatchOutcome {
	const result = { status: "proposed", ...proposal, note: PENDING };
	const text =
		proposal.kind === "transaction"
			? JSON.stringify({
					status: "proposed",
					tool: proposal.tool,
					file: proposal.file,
					entry: proposal.entry,
					note: PENDING,
				})
			: JSON.stringify({
					status: "proposed",
					tool: proposal.tool,
					file: proposal.file,
					lines: (proposal.after ?? "").split("\n").length,
					note: PENDING,
				});
	return { ok: true, result, text, proposal };
}

/**
 * The proposal a stored tool answer carries, for a transcript reopened from the
 * ledger. `null` for any other answer, including a refusal.
 */
export function proposalOf(result: unknown): Proposal | null {
	if (!result || typeof result !== "object" || Array.isArray(result)) return null;
	const record = result as Record<string, unknown>;
	if (record.status !== "proposed") return null;
	const kind = record.kind;
	if (kind !== "transaction" && kind !== "file") return null;
	if (typeof record.tool !== "string" || typeof record.file !== "string") return null;

	if (kind === "transaction") {
		const entry = record.entry;
		if (!entry || typeof entry !== "object" || Array.isArray(entry)) return null;
		return {
			tool: record.tool,
			kind,
			file: record.file,
			entry: entry as NewTransaction,
			before: null,
			after: null,
		};
	}
	if (typeof record.before !== "string" || typeof record.after !== "string") return null;
	return {
		tool: record.tool,
		kind,
		file: record.file,
		entry: null,
		before: record.before,
		after: record.after,
	};
}

/**
 * Perform a proposal the user approved — the only place a proposal becomes a
 * write.
 *
 * The command is the app's own (`add_transaction` / `update_file_text`), so the
 * ledger is reloaded, validated and staged exactly as an edit from the register
 * is; a refusal rejects with the backend's message, which the card shows
 * verbatim. On success every surface reading the ledger is told.
 */
export async function applyProposal(proposal: Proposal): Promise<void> {
	if (proposal.kind === "transaction") {
		if (!proposal.entry) throw new Error("this proposal has no entry to write");
		await unwrap(commands.addTransaction(proposal.entry));
	} else {
		if (proposal.after === null || proposal.before === null) {
			throw new Error("this proposal has no text to write");
		}
		// The proposal was staged against the file as it stood then. If anything
		// has written it since — the register, the source editor, a reload — the
		// model's text would silently replace that edit, so it is refused instead
		// and the reader can ask again against the current text.
		const current = await unwrap(commands.fileText(proposal.file));
		if (current !== proposal.before) {
			throw new Error(
				`\`${proposal.file}\` changed since this was proposed; ask again so it is staged against the current text`
			);
		}
		await unwrap(commands.updateFileText(proposal.file, proposal.after, proposal.before));
	}
	bumpLedgerRevision();
	await invalidateAll();
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
