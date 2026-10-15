/*
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
*/
import type * as Monaco from "monaco-editor";

/**
 * The ledger's own text, as Monaco needs to know it: the language id its models
 * take, the tokenizer for the syntax a `.bean` file is written in, the two
 * themes the app switches between, and the snippet that turns an entry's fields
 * into Tab stops.
 *
 * Nothing here reads or writes money, and nothing rewrites the ledger: this is
 * text in, text out. Monaco is imported for its types only, so the module costs
 * the initial bundle nothing.
 */

/** The language id the entry text is tokenized as. */
export const BEANCOUNT_LANGUAGE = "beancount";

/** Theme ids for the app's `dark` class and for its light default. */
export const BEANCOUNT_DARK = "beancount-dark";
export const BEANCOUNT_LIGHT = "beancount-light";

/**
 * A Monarch tokenizer over the syntax this app's files hold: the directives,
 * dates, transaction flags, accounts, commodities, numbers with thousands
 * separators, the `@`/`@@` price and `{…}`/`{{…}}` cost punctuation, quoted
 * strings, `;` comments and `#tag`/`^link`.
 *
 * It is a colouring aid and stays one: every rule is anchored to what a
 * character run looks like, never to how the file is structured, so a text it
 * misreads still tokenizes as ordinary text.
 */
const language: Monaco.languages.IMonarchLanguage = {
	defaultToken: "",
	tokenPostfix: ".beancount",
	tokenizer: {
		root: [
			// A `;` comments out the rest of the line; inside a quote the string
			// state below has already claimed it.
			[/;.*$/, "comment"],
			// Dates come before numbers, or `2026-01-10` reads as three of them.
			[/\d{4}-\d{2}-\d{2}/, "number.date"],
			[
				/\b(?:txn|option|plugin|include|open|close|commodity|pad|balance|price|note|document|event|query|custom|pushtag|poptag|pushmeta|popmeta)\b/,
				"keyword.directive",
			],
			// The flag after the date: `*` cleared, `!` to be reviewed.
			[/[*!]/, "keyword.flag"],
			// An account is a capitalised name with at least one colon. Its roots
			// are the file's own (`option "name_assets"`), so there is no list.
			[/[A-Z][A-Za-z0-9_]*(?::[A-Za-z0-9][A-Za-z0-9_-]*)+/, "type.account"],
			// Commodities are written in capitals (USD, AVEE, VTI); a quoted one
			// is a string and never reaches this rule.
			[/\b[A-Z][A-Z0-9'._-]{1,11}\b/, "constant.currency"],
			// `-1,234.56`, `1234.56`, `.5`, `+2`.
			[/[+-]?(?:\d[\d,]*(?:\.\d+)?|\.\d+)/, "number"],
			// `@ 1.10 USD`, `@@ 55.00 USD`, `{100.00 USD}`, `{{200.00 USD}}`.
			// The price mark is spelled `@{1,2}` and not `@@?`: Monarch reads a
			// literal `@@` in a rule as an escaped, single `@`.
			[/@{1,2}/, "operator.price"],
			[/\{\{|\}\}/, "delimiter.brace.pair"],
			[/[{}]/, "delimiter.brace"],
			[/=/, "operator"],
			[/#[A-Za-z0-9][A-Za-z0-9_./-]*/, "tag"],
			[/\^[A-Za-z0-9][A-Za-z0-9_./-]*/, "link"],
			[/"/, "string", "@string"],
		],
		string: [
			[/[^"\\]+/, "string"],
			[/\\./, "string.escape"],
			[/"/, "string", "@pop"],
		],
	},
};

/**
 * Colours follow the app: shadcn's neutral greys — `#252525` background and
 * `#fafafa` text in dark — with GitHub's syntax tones read against them, one
 * meaning per kind of field.
 */
const dark: Monaco.editor.IStandaloneThemeData = {
	base: "vs-dark",
	inherit: true,
	rules: [
		{ token: "comment", foreground: "8b949e", fontStyle: "italic" },
		{ token: "string", foreground: "a5d6ff" },
		{ token: "string.escape", foreground: "ff7b72" },
		{ token: "number", foreground: "79c0ff" },
		{ token: "number.date", foreground: "ffa657" },
		{ token: "keyword.directive", foreground: "ff7b72" },
		{ token: "keyword.flag", foreground: "e3b341" },
		{ token: "type.account", foreground: "d2a8ff" },
		{ token: "constant.currency", foreground: "7ee787" },
		{ token: "operator.price", foreground: "8b949e" },
		{ token: "operator", foreground: "8b949e" },
		{ token: "delimiter.brace", foreground: "79c0ff" },
		{ token: "delimiter.brace.pair", foreground: "79c0ff" },
		{ token: "tag", foreground: "56d4dd" },
		{ token: "link", foreground: "56d4dd" },
	],
	colors: {
		"editor.background": "#252525",
		"editor.foreground": "#fafafa",
		"editorLineNumber.foreground": "#6e6e6e",
		"editor.lineHighlightBackground": "#2e2e2e",
	},
};

/** The light half of the same palette: `#ffffff` paper, `#252525` ink. */
const light: Monaco.editor.IStandaloneThemeData = {
	base: "vs",
	inherit: true,
	rules: [
		{ token: "comment", foreground: "6e7781", fontStyle: "italic" },
		{ token: "string", foreground: "0a3069" },
		{ token: "string.escape", foreground: "cf222e" },
		{ token: "number", foreground: "0550ae" },
		{ token: "number.date", foreground: "953800" },
		{ token: "keyword.directive", foreground: "cf222e" },
		{ token: "keyword.flag", foreground: "7d4e00" },
		{ token: "type.account", foreground: "8250df" },
		{ token: "constant.currency", foreground: "116329" },
		{ token: "operator.price", foreground: "6e7781" },
		{ token: "operator", foreground: "6e7781" },
		{ token: "delimiter.brace", foreground: "0550ae" },
		{ token: "delimiter.brace.pair", foreground: "0550ae" },
		{ token: "tag", foreground: "0969da" },
		{ token: "link", foreground: "0969da" },
	],
	colors: {
		"editor.background": "#ffffff",
		"editor.foreground": "#252525",
		"editorLineNumber.foreground": "#8b949e",
		"editor.lineHighlightBackground": "#f5f5f5",
	},
};

/**
 * Registers the language, its tokenizer and both themes. Call it once, after
 * Monaco itself has loaded and before any editor is created — a model created
 * under an unregistered language falls back to plain text and never re-colours.
 */
export function registerBeanLanguage(monaco: typeof Monaco): void {
	// The ledger has no language service; the tokenizer is the whole of it, and
	// the language configuration is only there for `;` line comments.
	monaco.languages.register({ id: BEANCOUNT_LANGUAGE });
	monaco.languages.setLanguageConfiguration(BEANCOUNT_LANGUAGE, {
		comments: { lineComment: ";" },
	});
	monaco.languages.setMonarchTokensProvider(BEANCOUNT_LANGUAGE, language);
	monaco.editor.defineTheme(BEANCOUNT_DARK, dark);
	monaco.editor.defineTheme(BEANCOUNT_LIGHT, light);
}

/** A run of the entry's own characters that a snippet stop wraps. */
type Field = { start: number; end: number };

/** An entry opens with a date, then a flag: `*` cleared, `!` to review. */
const HEADER = /^(\d{4}-\d{2}-\d{2})([ \t]+)([*!]|txn)(?=$|[ \t])/;
/** A posting is indented, names an account, then gives it a value. */
const POSTING = /^[ \t]+([A-Z][A-Za-z0-9_]*(?::[A-Za-z0-9][A-Za-z0-9_-]*)+)[ \t]+/;
/** Its value's figure: `-1,234.56` of `-1,234.56 USD`. */
const AMOUNT = /^[+-]?(?:\d[\d,]*(?:\.\d+)?|\.\d+)/;
/**
 * The offsets of the fields an entry is made of, in reading order: the date,
 * the flag, its payee and narration when it carries them, and each posting's
 * amount figure. `null` when the text is not an entry this can read.
 */
function entryFields(text: string): Field[] | null {
	// The entry's lines with where each starts: `split` took the terminators
	// out, so the next line starts one character past this one.
	let offset = 0;
	const lines = text.split("\n").map((line) => {
		const start = offset;
		offset += line.length + 1;
		return { line, start };
	});

	// Blank lines and comments can lead an entry without being part of it. The
	// first line that is neither has to be the dated header: anything else
	// means this is not an entry this can read, and nothing of it gets touched.
	const head = lines.find(({ line }) => !/^\s*(?:;.*)?$/.test(line));
	const header = head && HEADER.exec(head.line);
	if (!head || !header) return null;

	const fields: Field[] = [];
	const date = { start: head.start, end: head.start + header[1].length };
	const flag = date.end + header[2].length;
	fields.push(date, { start: flag, end: flag + header[3].length });

	// The header's quoted strings are the payee then the narration when both
	// are there, and the narration alone when there is one. The stops go inside
	// the quotes, so replacing a field leaves them standing.
	const tail = head.line.slice(header[0].length);
	let strings = 0;
	for (const quoted of tail.matchAll(/"(?:[^"\\]|\\.)*"/g)) {
		if (strings === 2) break;
		strings += 1;
		const content = head.start + header[0].length + (quoted.index ?? 0) + 1;
		fields.push({ start: content, end: content + quoted[0].length - 2 });
	}

	// Each posting's amount figure, wherever on the line its account put it.
	// Lines that are not postings — metadata, comments, blanks — stay as they
	// are, with no stop of their own.
	for (const { line, start } of lines.slice(lines.indexOf(head) + 1)) {
		const posting = POSTING.exec(line);
		if (!posting) continue;
		const figure = AMOUNT.exec(line.slice(posting[0].length));
		if (!figure) continue;
		const figureStart = start + posting[0].length;
		fields.push({ start: figureStart, end: figureStart + figure[0].length });
	}

	return fields;
}

/**
 * Escapes the three characters Monaco's snippet parser would otherwise read:
 * `$` opens a placeholder or a variable, `}` closes a placeholder, `\` escapes
 * either. This is the parser's own `SnippetParser.escape` rule.
 */
function escapeSnippet(value: string): string {
	return value.replace(/\$|}|\\/g, "\\$&");
}

/**
 * A Monaco snippet template whose stops are the fields of `text`, each one
 * defaulting to the entry's own characters, so that inserting it writes back
 * exactly the text it was built from — the ledger is never re-spaced or
 * re-aligned. `null` when `text` is not an entry this can read.
 */
export function fieldSnippet(text: string): string | null {
	const fields = entryFields(text);
	if (!fields) return null;

	let snippet = "";
	let cursor = 0;
	fields.forEach((field, index) => {
		snippet += escapeSnippet(text.slice(cursor, field.start));
		snippet += `\${${index + 1}:${escapeSnippet(text.slice(field.start, field.end))}}`;
		cursor = field.end;
	});
	return snippet + escapeSnippet(text.slice(cursor));
}
