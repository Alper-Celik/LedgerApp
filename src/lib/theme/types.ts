// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * A theme is nothing but the shadcn custom properties the app reads, split by
 * mode. The maps are sparse: a token that is absent falls through to whatever
 * `app.css` declares, which is what makes an override style element able to
 * exist without copying the shipped theme into JS.
 */
export type ThemeTokens = Record<string, string>;

/** The two blocks a theme declares: `:root` and `.dark`. */
export type ThemeMode = "light" | "dark";

/** A named set of overrides; `id` is only used to tell saved themes apart. */
export interface Theme {
	id: string;
	name: string;
	light: ThemeTokens;
	dark: ThemeTokens;
}

/** How a value is validated — every token is one of these three shapes. */
export type TokenKind = "color" | "length" | "font";

export interface TokenSpec {
	/** The custom property, leading `--` included. */
	token: string;
	/** Human name for the field, used as its accessible label. */
	label: string;
	kind: TokenKind;
}

export interface TokenGroup {
	id: "colours" | "charts" | "sidebar" | "app" | "shape";
	label: string;
	hint: string;
	/**
	 * `false` for tokens the app declares once, outside the mode blocks
	 * (`--radius`, `--font-sans`): they are edited once and live in `:root`.
	 */
	modeScoped: boolean;
	tokens: readonly TokenSpec[];
}

/*
	Everything below is the set `src/app.css` actually declares. There is no
	`--success` / `--warning` in the app (its own extras are the diff tones
	`--added` / `--removed`), so those names are deliberately absent: a token
	the stylesheet does not read would be an editor field that changes nothing.
*/
export const TOKEN_GROUPS: readonly TokenGroup[] = [
	{
		id: "colours",
		label: "Colours",
		hint: "Surfaces, text, borders and the interactive palette.",
		modeScoped: true,
		tokens: [
			{ token: "--background", label: "Background", kind: "color" },
			{ token: "--foreground", label: "Foreground", kind: "color" },
			{ token: "--card", label: "Card", kind: "color" },
			{ token: "--card-foreground", label: "Card foreground", kind: "color" },
			{ token: "--popover", label: "Popover", kind: "color" },
			{ token: "--popover-foreground", label: "Popover foreground", kind: "color" },
			{ token: "--primary", label: "Primary", kind: "color" },
			{ token: "--primary-foreground", label: "Primary foreground", kind: "color" },
			{ token: "--secondary", label: "Secondary", kind: "color" },
			{ token: "--secondary-foreground", label: "Secondary foreground", kind: "color" },
			{ token: "--muted", label: "Muted", kind: "color" },
			{ token: "--muted-foreground", label: "Muted foreground", kind: "color" },
			{ token: "--accent", label: "Accent", kind: "color" },
			{ token: "--accent-foreground", label: "Accent foreground", kind: "color" },
			{ token: "--destructive", label: "Destructive", kind: "color" },
			{ token: "--border", label: "Border", kind: "color" },
			{ token: "--input", label: "Input", kind: "color" },
			{ token: "--ring", label: "Ring", kind: "color" },
		],
	},
	{
		id: "charts",
		label: "Charts",
		hint: "The five series colours charts and dashboards cycle through.",
		modeScoped: true,
		tokens: [
			{ token: "--chart-1", label: "Chart 1", kind: "color" },
			{ token: "--chart-2", label: "Chart 2", kind: "color" },
			{ token: "--chart-3", label: "Chart 3", kind: "color" },
			{ token: "--chart-4", label: "Chart 4", kind: "color" },
			{ token: "--chart-5", label: "Chart 5", kind: "color" },
		],
	},
	{
		id: "sidebar",
		label: "Sidebar",
		hint: "The navigation panel keeps its own palette, independent of the page.",
		modeScoped: true,
		tokens: [
			{ token: "--sidebar", label: "Sidebar", kind: "color" },
			{ token: "--sidebar-foreground", label: "Sidebar foreground", kind: "color" },
			{ token: "--sidebar-primary", label: "Sidebar primary", kind: "color" },
			{ token: "--sidebar-primary-foreground", label: "Sidebar primary foreground", kind: "color" },
			{ token: "--sidebar-accent", label: "Sidebar accent", kind: "color" },
			{ token: "--sidebar-accent-foreground", label: "Sidebar accent foreground", kind: "color" },
			{ token: "--sidebar-border", label: "Sidebar border", kind: "color" },
			{ token: "--sidebar-ring", label: "Sidebar ring", kind: "color" },
		],
	},
	{
		id: "app",
		label: "Ledger tones",
		hint: "The diff tones a patch, and an increase or decrease, is painted with.",
		modeScoped: true,
		tokens: [
			{ token: "--added", label: "Added", kind: "color" },
			{ token: "--removed", label: "Removed", kind: "color" },
		],
	},
	{
		id: "shape",
		label: "Shape and type",
		hint: "One radius and one family: the app derives every corner and size from them.",
		modeScoped: false,
		tokens: [
			{ token: "--radius", label: "Corner radius", kind: "length" },
			{ token: "--font-sans", label: "Sans-serif family", kind: "font" },
		],
	},
];

/** Every editable token, flattened. */
export const ALL_TOKENS: readonly TokenSpec[] = TOKEN_GROUPS.flatMap((group) => group.tokens);

/** Tokens declared once for both modes. */
export const GLOBAL_TOKENS: ReadonlySet<string> = new Set(
	TOKEN_GROUPS.filter((group) => !group.modeScoped).flatMap((group) =>
		group.tokens.map((spec) => spec.token),
	),
);

/** `--radius` and `--font-sans` are written to `:root`, never to `.dark`. */
export function isGlobalToken(token: string): boolean {
	return GLOBAL_TOKENS.has(token);
}

/**
 * The kind of a token the app declares, or `null` for a custom property an
 * imported theme brought in on its own — the parser then accepts any of the
 * three shapes rather than rejecting a token the app might still use.
 */
export function tokenKind(token: string): TokenKind | null {
	const known = ALL_TOKENS.find((spec) => spec.token === token);
	return known?.kind ?? null;
}

/** A theme with no overrides at all — the shipped appearance. */
export function emptyTheme(name = "Custom"): Theme {
	return { id: "custom", name, light: {}, dark: {} };
}
