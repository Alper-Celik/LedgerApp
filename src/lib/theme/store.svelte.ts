// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { TOKEN_NAME_RE, sanitiseValue } from "./parse.js";
import { ALL_TOKENS, isGlobalToken, type Theme, type ThemeTokens } from "./types.js";

/*
	The override lives in one `<style id="app-theme">` element appended to the
	document, after every stylesheet `app.css` produced. `app.css` is never
	rewritten, so removing the element restores the shipped theme exactly, and
	a theme is only ever what the app does not already declare — the maps are
	sparse and anything left out keeps its shipped value.
*/
const STYLE_ID = "app-theme";
const STORAGE_KEY = "app_theme_v1";
const STORAGE_VERSION = 1;

/** The custom properties the shipped stylesheet declares, per mode. */
export interface ThemeDefaults {
	light: ThemeTokens;
	dark: ThemeTokens;
}

export const theme = $state<{
	/** What is in the override element right now; `null` means shipped defaults. */
	active: Theme | null;
	/** The theme saved for the next boot, even while another one is previewed. */
	custom: Theme | null;
	/** `app.css`'s own values, read from the live stylesheets. */
	defaults: ThemeDefaults;
}>({
	active: null,
	custom: null,
	defaults: { light: {}, dark: {} },
});

function hasOverrides(value: Theme | null): boolean {
	if (!value) return false;
	return Object.keys(value.light).length > 0 || Object.keys(value.dark).length > 0;
}

function declarations(tokens: ThemeTokens): string {
	return Object.entries(tokens)
		.map(([token, value]) => `\t${token}: ${value};`)
		.join("\n");
}

/**
 * The stylesheet text for a theme: its overrides in `:root` and `.dark`, plus
 * one `html` rule for the font family. Tailwind's `@theme inline` inlines
 * `--font-sans` into the `font-sans` utility rather than referencing the
 * variable, so the family has to be set on the element as well — the variable
 * is still written for anything that reads it directly.
 */
export function toCss(value: Theme): string {
	const blocks: string[] = [];
	const light = declarations(value.light);
	if (light) blocks.push(`:root {\n${light}\n}`);
	const dark = declarations(value.dark);
	if (dark) blocks.push(`.dark {\n${dark}\n}`);
	const family = value.light["--font-sans"];
	if (family) blocks.push(`html {\n\tfont-family: ${family};\n}`);
	return blocks.join("\n");
}

function styleElement(): HTMLStyleElement {
	const existing = document.getElementById(STYLE_ID);
	if (existing instanceof HTMLStyleElement) return existing;
	const element = document.createElement("style");
	element.id = STYLE_ID;
	document.body.append(element);
	return element;
}

/**
 * Applies a theme, or removes the override when given `null`. The element is
 * kept as the last child of the body so it stays after every stylesheet,
 * including the ones a dev server injects or re-injects.
 */
export function apply(next: Theme | null) {
	theme.active = next;
	if (typeof document === "undefined") return;
	const css = next ? toCss(next) : "";
	if (!css) {
		document.getElementById(STYLE_ID)?.remove();
		return;
	}
	const element = styleElement();
	element.textContent = css;
	if (document.body.lastElementChild !== element) document.body.append(element);
}

/** Persists the theme for the next boot; `null` clears the saved one. */
export function save(next: Theme | null = theme.active) {
	theme.custom = next;
	if (typeof localStorage === "undefined") return;
	if (!hasOverrides(next)) {
		localStorage.removeItem(STORAGE_KEY);
		return;
	}
	try {
		localStorage.setItem(
			STORAGE_KEY,
			JSON.stringify({ version: STORAGE_VERSION, theme: next }),
		);
	} catch {
		// A full or disabled store: the theme stays applied for this session.
	}
}

/** Removes every override and the saved theme: back to what `app.css` ships. */
export function reset() {
	apply(null);
	theme.custom = null;
	if (typeof localStorage !== "undefined") localStorage.removeItem(STORAGE_KEY);
}

/*
	Defaults are read from the stylesheets themselves rather than duplicated in
	JS: a rule whose selector is `:root` (or `html`) is light, a `.dark` rule is
	dark, and the last declaration of a token wins, exactly as the cascade does.
	Reading the CSSOM also gives the right answer whatever mode the app is
	currently in, which computed styles cannot.
*/
function collectDefaults(rules: CSSRuleList, light: ThemeTokens, dark: ThemeTokens) {
	for (const rule of Array.from(rules)) {
		if (rule instanceof CSSStyleRule) {
			const selector = rule.selectorText;
			const isDark = /(^|[\s>+~,(])\.dark(?![\w-])/.test(selector);
			const isRoot = !isDark && /(^|[\s>+~,(])(:root|html)(?![\w-])/.test(selector);
			if (!isDark && !isRoot) continue;
			const style = rule.style;
			for (let index = 0; index < style.length; index += 1) {
				const token = style.item(index);
				if (!token.startsWith("--")) continue;
				const value = style.getPropertyValue(token).trim();
				if (!value) continue;
				(isDark ? dark : light)[token] = value;
			}
			continue;
		}
		if ("cssRules" in rule) collectDefaults((rule as CSSGroupingRule).cssRules, light, dark);
	}
}

function readShippedDefaults(): ThemeDefaults {
	const light: ThemeTokens = {};
	const dark: ThemeTokens = {};
	if (typeof document === "undefined") return { light, dark };

	for (const sheet of Array.from(document.styleSheets)) {
		const owner = sheet.ownerNode;
		if (owner instanceof Element && owner.id === STYLE_ID) continue;
		try {
			collectDefaults(sheet.cssRules, light, dark);
		} catch {
			// A stylesheet from another origin cannot be inspected; skip it.
		}
	}

	// A sheet injected with `@theme inline` may not emit a variable; fall back
	// to the computed value, which is mode-dependent, so only use it when the
	// app is not in dark mode — except for the tokens that never vary by mode.
	const htmlIsDark = document.documentElement.classList.contains("dark");
	const computed = getComputedStyle(document.documentElement);
	for (const spec of ALL_TOKENS) {
		if (light[spec.token]) continue;
		if (htmlIsDark && !isGlobalToken(spec.token)) continue;
		const value = computed.getPropertyValue(spec.token).trim();
		if (value) light[spec.token] = value;
	}

	return { light, dark };
}

/** Re-sanitises a persisted theme: storage is text a user (or a bug) can edit. */
function reviveTokens(value: unknown): ThemeTokens {
	const tokens: ThemeTokens = {};
	if (typeof value !== "object" || value === null) return tokens;
	for (const [token, raw] of Object.entries(value)) {
		if (!TOKEN_NAME_RE.test(token) || typeof raw !== "string") continue;
		const result = sanitiseValue(token, raw);
		if (result.ok) tokens[token] = result.value;
	}
	return tokens;
}

function revive(raw: unknown): Theme | null {
	if (typeof raw !== "object" || raw === null) return null;
	const candidate = raw as Partial<Theme>;
	const revived: Theme = {
		id: typeof candidate.id === "string" ? candidate.id : "custom",
		name:
			typeof candidate.name === "string" && candidate.name.trim()
				? candidate.name
				: "Custom",
		light: reviveTokens(candidate.light),
		dark: reviveTokens(candidate.dark),
	};
	return hasOverrides(revived) ? revived : null;
}

function readStored(): Theme | null {
	if (typeof localStorage === "undefined") return null;
	const raw = localStorage.getItem(STORAGE_KEY);
	if (!raw) return null;
	try {
		const parsed = JSON.parse(raw) as { version?: unknown; theme?: unknown };
		if (parsed.version !== STORAGE_VERSION) {
			localStorage.removeItem(STORAGE_KEY);
			return null;
		}
		return revive(parsed.theme);
	} catch {
		localStorage.removeItem(STORAGE_KEY);
		return null;
	}
}

let loaded = false;

/**
 * Boot entry point: reads `app.css`'s defaults and re-applies the saved theme.
 * Idempotent, so the layout and the editor can both call it.
 */
export function load() {
	if (loaded) return;
	loaded = true;
	theme.defaults = readShippedDefaults();
	const stored = readStored();
	theme.custom = stored;
	apply(stored);
}
