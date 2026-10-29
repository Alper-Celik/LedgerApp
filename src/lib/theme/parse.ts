// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { tokenKind, type Theme, type ThemeTokens } from "./types.js";

/**
 * Importing a theme is the only place the app takes free-form text that ends up
 * in a stylesheet, so every value is whitelisted here: the parser never hands a
 * raw string to the DOM. Nothing may terminate a declaration (`;`), close a
 * block (`}`), open one (`{`), start an at-rule (`@`), escape (`\`), tag the
 * document (`<`, `>`) or fetch anything (`url(...)`), and a value must be one
 * of the three shapes a token can take — a colour, a length, or a font stack.
 */

/** A token that could not be imported, named exactly as it was written. */
export interface ThemeParseError {
	token: string;
	message: string;
}

/** What both import formats return: what survived, and why the rest did not. */
export interface ParseResult {
	theme: Theme;
	errors: ThemeParseError[];
}

export type Sanitised = { ok: true; value: string } | { ok: false; error: string };

/** `--` and lower-case words, digits and dashes: the only names a token may have. */
export const TOKEN_NAME_RE = /^--[a-z0-9-]+$/;

const NUMBER = String.raw`[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:e[+-]?\d+)?`;

const LENGTH_RE = new RegExp(
	`^${NUMBER}(?:%|px|rem|em|ex|ch|pt|pc|in|cm|mm|q|vh|vw|vmin|vmax|svh|svw|lvh|lvw|dvh|dvw)?$`,
	"i",
);

const HEX_COLOUR_RE = /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;

const COLOUR_FUNCTIONS = ["rgb", "rgba", "hsl", "hsla", "oklch", "oklab", "lab", "lch", "color"];

const COLOUR_FN_RE = new RegExp(`^(?:${COLOUR_FUNCTIONS.join("|")})\\(([^()]*)\\)$`, "i");

/** The inside of a colour function: numbers, units, separators — nothing else. */
const COLOUR_FN_BODY_RE = /^[0-9a-z.,%/+\-\s]*$/i;

const COLOUR_KEYWORDS: Record<string, true> = {
	transparent: true,
	currentcolor: true,
	inherit: true,
	initial: true,
	unset: true,
	revert: true,
	none: true,
};

/** Letters, digits, spaces, dots, underscores and dashes: a family name. */
const FONT_CHAR_RE = /^[\p{L}\p{N} ._-]+$/u;

/** Control characters would break the declaration; newlines are a subset. */
const CONTROL_RE = /[\u0000-\u001f\u007f]/;

/** Anything that could escape the declaration or load a resource. */
const UNSAFE_RE = /[;{}<>\\@]|url\s*\(|\/\*/i;

const EMPTY_VALUE = "is empty";

function colourProblem(value: string): string | null {
	if (HEX_COLOUR_RE.test(value)) return null;
	if (Object.hasOwn(COLOUR_KEYWORDS, value.toLowerCase())) return null;
	const call = COLOUR_FN_RE.exec(value);
	if (call && COLOUR_FN_BODY_RE.test(call[1])) return null;
	return `is not a colour: expected #hex, ${COLOUR_FUNCTIONS.map((name) => `${name}()`).join(
		"/",
	)}, or a keyword such as transparent`;
}

function lengthProblem(value: string): string | null {
	if (LENGTH_RE.test(value)) return null;
	return "is not a length: expected a number with an optional unit (rem, px, em, %, …)";
}

function fontProblem(value: string): string | null {
	if (value.length > 200) return "is too long for a font stack";
	for (const part of value.split(",")) {
		const family = part.trim();
		if (!family) return "has an empty font family — stacks are comma-separated names";
		const quoted = /^(['"])(.*)\1$/.exec(family);
		const name = quoted ? quoted[2] : family;
		if (!FONT_CHAR_RE.test(name)) {
			return `has a family "(${family})" that may only contain letters, digits, spaces, dots, underscores and dashes`;
		}
	}
	return null;
}

/**
 * The single gate every value passes through, on import and again when a saved
 * theme is read back. `token` picks the shape (a colour, a length, a font
 * stack); a name this build does not know is accepted if it is any one of the
 * three, since an imported theme may carry a custom property of its own.
 */
export function sanitiseValue(token: string, raw: string): Sanitised {
	const value = raw.trim();
	if (!value) return { ok: false, error: EMPTY_VALUE };
	if (value.length > 300) return { ok: false, error: "is too long" };
	if (CONTROL_RE.test(value)) {
		return { ok: false, error: "contains a control character or line break" };
	}
	if (UNSAFE_RE.test(value)) {
		return {
			ok: false,
			error:
				"contains a character that is not allowed (`;`, `{`, `}`, `<`, `>`, `@`, `\\` or `url(`): a value can never leave its declaration",
		};
	}

	const kind = tokenKind(token);
	if (kind === "color") {
		const problem = colourProblem(value);
		return problem ? { ok: false, error: problem } : { ok: true, value };
	}
	if (kind === "length") {
		const problem = lengthProblem(value);
		return problem ? { ok: false, error: problem } : { ok: true, value };
	}
	if (kind === "font") {
		const problem = fontProblem(value);
		return problem ? { ok: false, error: problem } : { ok: true, value };
	}

	if (!colourProblem(value) || !lengthProblem(value) || !fontProblem(value)) {
		return { ok: true, value };
	}
	return {
		ok: false,
		error:
			"is not a colour, a length, or a font stack — imported custom properties must be one of those",
	};
}

/** Splits a rule body into its own declarations, dropping any nested rule. */
function declarations(body: string): string[] {
	const parts: string[] = [];
	let depth = 0;
	let current = "";
	for (const character of body) {
		if (character === "{") {
			depth += 1;
			continue;
		}
		if (character === "}") {
			depth = Math.max(0, depth - 1);
			// Whatever a nested rule declared is not a `:root` declaration.
			current = "";
			continue;
		}
		if (depth > 0) continue;
		if (character === ";") {
			parts.push(current);
			current = "";
			continue;
		}
		current += character;
	}
	parts.push(current);
	return parts;
}

interface CssBlock {
	selector: string;
	body: string;
}

/** Every style rule in a stylesheet text, including rules inside at-rules. */
function cssBlocks(css: string): CssBlock[] {
	const found: CssBlock[] = [];
	let start = 0;
	let depth = 0;
	let selector = "";
	let bodyStart = 0;
	for (let index = 0; index < css.length; index += 1) {
		const character = css[index];
		if (character === "{") {
			if (depth === 0) {
				selector = css.slice(start, index).trim();
				bodyStart = index + 1;
			}
			depth += 1;
		} else if (character === "}") {
			depth -= 1;
			if (depth <= 0) {
				depth = 0;
				if (selector) {
					const body = css.slice(bodyStart, index);
					if (selector.startsWith("@")) found.push(...cssBlocks(body));
					else found.push({ selector, body });
				}
				selector = "";
				start = index + 1;
			}
		} else if (character === ";" && depth === 0) {
			start = index + 1;
		}
	}
	return found;
}

function stripComments(css: string): string {
	return css.replace(/\/\*[\s\S]*?\*\//g, "");
}

/**
 * Reads a shadcn/tweakcn CSS export — `:root { … }` plus an optional
 * `.dark { … }` — from a file that may also carry `@import` lines, comments,
 * `@custom-variant` and any other at-rule. Only `--name: value` declarations
 * whose name matches `^--[a-z0-9-]+$` are taken; everything else in the file
 * is ignored, and each taken value is sanitised.
 */
export function fromCss(text: string): ParseResult {
	const errors: ThemeParseError[] = [];
	const light: ThemeTokens = {};
	const dark: ThemeTokens = {};

	for (const block of cssBlocks(stripComments(text))) {
		const darkBlock = /(^|[\s>+~,(])\.dark(?![\w-])/.test(block.selector);
		const rootBlock = !darkBlock && /(^|[\s>+~,(])(:root|html)(?![\w-])/.test(block.selector);
		if (!darkBlock && !rootBlock) continue;

		for (const declaration of declarations(block.body)) {
			const match = /^\s*(--[a-z0-9-]+)\s*:([\s\S]*)$/.exec(declaration);
			if (!match) continue;
			const token = match[1];
			if (!TOKEN_NAME_RE.test(token)) continue;
			const result = sanitiseValue(token, match[2]);
			if (!result.ok) {
				errors.push({ token, message: result.error });
				continue;
			}
			(darkBlock ? dark : light)[token] = result.value;
		}
	}

	if (Object.keys(light).length === 0 && Object.keys(dark).length === 0 && errors.length === 0) {
		errors.push({
			token: ":root",
			message:
				"no theme found: paste a CSS export with a `:root { --… }` block (and optionally `.dark { --… }`)",
		});
	}

	return { theme: { id: "imported", name: "Imported", light, dark }, errors };
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function collectTokens(
	source: Record<string, unknown>,
	errors: ThemeParseError[],
	requirePrefix: boolean,
): ThemeTokens {
	const tokens: ThemeTokens = {};
	for (const [key, raw] of Object.entries(source)) {
		if (requirePrefix && !key.startsWith("--")) continue;
		const token = key.startsWith("--") ? key : `--${key}`;
		if (!TOKEN_NAME_RE.test(token)) {
			errors.push({
				token: key,
				message: "is not a custom property: names must match ^--[a-z0-9-]+$",
			});
			continue;
		}
		if (typeof raw === "object" && raw !== null) {
			errors.push({ token, message: "is not a single value" });
			continue;
		}
		const value = typeof raw === "string" ? raw : typeof raw === "number" ? String(raw) : null;
		if (value === null) {
			errors.push({ token, message: "is not a string or a number" });
			continue;
		}
		const result = sanitiseValue(token, value);
		if (!result.ok) {
			errors.push({ token, message: result.error });
			continue;
		}
		tokens[token] = result.value;
	}
	return tokens;
}

/**
 * Reads a theme builder's JSON: either `{ light: { … }, dark: { … } }` (also
 * nested under a `cssVars` key, as the shadcn registry writes it, and with an
 * optional shared `theme` map folded into light) or a flat map of
 * `--token: value` pairs, which becomes the light map.
 */
export function fromJson(text: string): ParseResult {
	const errors: ThemeParseError[] = [];
	let data: unknown;
	try {
		data = JSON.parse(text);
	} catch (error) {
		errors.push({
			token: "json",
			message: `is not valid JSON: ${error instanceof Error ? error.message : String(error)}`,
		});
		return { theme: { id: "imported", name: "Imported", light: {}, dark: {} }, errors };
	}
	if (!isRecord(data)) {
		errors.push({ token: "json", message: "must be an object of theme tokens" });
		return { theme: { id: "imported", name: "Imported", light: {}, dark: {} }, errors };
	}

	const shell = isRecord(data.cssVars) ? data.cssVars : data;
	const hasModes = isRecord(shell.light) || isRecord(shell.dark);
	const shared = isRecord(shell.theme) ? collectTokens(shell.theme, errors, false) : null;

	let light: ThemeTokens;
	let dark: ThemeTokens;
	if (hasModes) {
		light = isRecord(shell.light) ? collectTokens(shell.light, errors, false) : {};
		dark = isRecord(shell.dark) ? collectTokens(shell.dark, errors, false) : {};
		if (shared) light = { ...shared, ...light };
	} else {
		light = collectTokens(shell, errors, true);
		dark = {};
	}

	if (Object.keys(light).length === 0 && Object.keys(dark).length === 0 && errors.length === 0) {
		errors.push({
			token: "json",
			message: "no custom properties found: expected a flat map or `light`/`dark` objects",
		});
	}

	const name = typeof data.name === "string" && data.name.trim() ? data.name.trim() : "Imported";
	return { theme: { id: "imported", name, light, dark }, errors };
}

/**
 * Seeds a native `<input type="color">`, which only understands `#rrggbb`.
 * Handles hex and `rgb()`/`rgba()`; every other notation returns `null` and the
 * picker falls back to black, because the text field remains the source of
 * truth and no colour model is worth reimplementing here.
 */
export function toHexColor(value: string): string | null {
	const text = value.trim();
	const hex = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(text);
	if (hex) {
		const expand = (digits: string) =>
			digits.length <= 4
				? digits
						.split("")
						.map((digit) => digit + digit)
						.join("")
				: digits;
		return `#${expand(hex[1]).slice(0, 6).toLowerCase()}`;
	}

	const call = /^rgba?\(([^()]*)\)$/i.exec(text);
	if (!call) return null;
	const parts = call[1].split(/[\s,/]+/).filter(Boolean);
	if (parts.length < 3) return null;

	const channels: number[] = [];
	for (const part of parts.slice(0, 3)) {
		const numeric = Number.parseFloat(part);
		if (!Number.isFinite(numeric)) return null;
		const scaled = part.endsWith("%") ? (numeric / 100) * 255 : numeric;
		channels.push(Math.round(Math.min(255, Math.max(0, scaled))));
	}
	return `#${channels.map((channel) => channel.toString(16).padStart(2, "0")).join("")}`;
}
