// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/** An exact amount as the ledger writes it: decimal units and their commodity. */
export type Amount = { units: string; currency: string };

/**
 * Where a currency symbol sits relative to the digits. Most currencies are
 * written before the amount ("$12.34", "₺1.234,56"), many after it ("12,34 €"),
 * and a rare few in between — so placement is data here, not an assumption
 * baked into the rendering code.
 */
type CurrencyPlacement = "prefix" | "middle" | "suffix";

type CurrencyStyle = {
	/** Currency symbol, or the commodity code for a quantity ("2.68 VTI"). */
	symbol: string;
	/** `prefix`: "€12", `middle`: "12€,34", `suffix`: "12,34 €". */
	placement: CurrencyPlacement;
	/** Write a space between symbol and digits ("1 234,56 €" but "$1,234.56"). */
	spaced: boolean;
};

/**
 * Locale for money formatting (grouping separator and decimal mark), taken
 * from the system locale the webview reports. `LANG=C`/`POSIX` resolves to
 * `en-US-POSIX`, which carries no grouping separator, so those are skipped.
 */
const localeTags =
	typeof navigator === "undefined" ? [] : [...(navigator.languages ?? []), navigator.language];
const locale =
	localeTags
		.map((tag) => {
			try {
				return new Intl.NumberFormat(tag).resolvedOptions().locale;
			} catch {
				return null; // not a well-formed language tag
			}
		})
		.find((resolved) => resolved && !resolved.toLowerCase().includes("posix")) ?? "en-US";

/**
 * Locale separators, read once from a fixed sample. No account value ever goes
 * through `Intl.NumberFormat.format()`, which takes a float: amounts stay exact
 * decimal strings and grouping/padding are done on the digits.
 */
const sampleParts = new Intl.NumberFormat(locale, { minimumFractionDigits: 2 }).formatToParts(
	1234567.89
);
const GROUP_SEPARATOR = sampleParts.find((part) => part.type === "group")?.value ?? ",";
const DECIMAL_SEPARATOR = sampleParts.find((part) => part.type === "decimal")?.value ?? ".";

/**
 * Exact sign of a decimal string: -1, 0 or 1, decided from the digits alone —
 * `Number()` would round a long amount and can even flip its sign at the edge.
 */
export function signOf(units: string): number {
	const text = units.trim();
	const negative = text.startsWith("-");
	const digits = (negative ? text.slice(1) : text).replace(".", "");
	if (/^0*$/.test(digits)) return 0;
	return negative ? -1 : 1;
}

/**
 * A decimal string (`-1234.50`, `125`, `0.00`) split into its sign and its two
 * runs of digits. Keeping the digits as text is what lets an amount be scaled
 * to an integer — `+`, `-`, `.` and leading zeroes dropped — without ever
 * passing through a float.
 */
export type Decimal = { negative: boolean; whole: string; fraction: string };

/**
 * `text` as a sign and digit runs, or `null` when it is not a plain decimal.
 *
 * A leading `+` is accepted (an exporter may write one) and an empty fraction
 * (`5.`) is simply no places. Anything else — `""`, `.5`, `1e5` — is
 * refused rather than guessed at: this module compares and sums ledger
 * amounts, and an amount it cannot read must not silently become a number.
 */
export function parseDecimal(text: string): Decimal | null {
	const match = /^([+-]?)(\d+)(?:\.(\d*))?$/.exec(text.trim());
	if (!match) return null;
	return { negative: match[1] === "-", whole: match[2] ?? "0", fraction: match[3] ?? "" };
}

/**
 * `value` as an integer at `scale` decimal places: the fraction is padded out
 * to that many places and the digits are joined, so `-12.30` at scale 3 is
 * `-12300n`. `scale` must be at least the value's own fraction length (callers
 * pass the wider of the two values being compared, which is why the padding
 * only ever grows). `-0` scales to `0n`, so it stays the same amount as `0`.
 */
export function scaledAt(value: Decimal, scale: number): bigint {
	const magnitude = BigInt(`${value.whole}${value.fraction.padEnd(scale, "0")}`);
	return value.negative ? -magnitude : magnitude;
}

/**
 * Exact ordering of two decimal strings: `-1`, `0` or `1`.
 *
 * The sign comes first — a negative amount orders below zero and `-0` is the
 * same amount as `0`, so it ties — and only amounts of one sign are compared
 * by magnitude, at the wider of the two scales and as integers, which is where
 * a `Number()` comparison would lose digits or call two amounts equal. Text
 * that is not a decimal string (`""`, `.`) is not an amount: it orders below
 * every number and ties with its own kind, so a sort stays a total order.
 */
export function compareDecimals(left: string, right: string): number {
	const a = parseDecimal(left);
	const b = parseDecimal(right);
	if (!a || !b) return a ? 1 : b ? -1 : 0;
	const scale = Math.max(a.fraction.length, b.fraction.length);
	const scaledLeft = scaledAt(a, scale);
	const scaledRight = scaledAt(b, scale);
	if (scaledLeft === scaledRight) return 0;
	return scaledLeft < scaledRight ? -1 : 1;
}

/**
 * CLDR's ISO 4217 list (307 codes), when the engine has `Intl.supportedValuesOf`
 * — WebKit 615+/Safari 15.4+. Without it, fall back to ICU's own rule that a
 * currency code is three letters.
 */
const isoCurrencies =
	typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf("currency") : null;

/**
 * Reads style off `Intl` (CLDR): the currency part of a formatted sample sits
 * before the integer ("prefix"), after the fraction ("suffix"), or between
 * them ("middle"), and the literal next to it tells us whether the locale
 * separates it with a space. When the locale prints the bare code — `en-US`
 * has no lira symbol — the narrow symbol (CLDR's own short form) is used
 * instead, so the currency still shows "₺" rather than "TRY".
 *
 * A commodity that isn't an ISO code is a quantity rather than money — a share
 * count (`2.68 VTI`), matching how the ledger writes it. That test has to be
 * CLDR's, not ICU's formatter's: ICU happily takes any 3-letter code as a
 * currency, so `VTI` would otherwise come out prefixed while the 4-letter
 * tickers next to it (which ICU rejects) came out suffixed.
 */
function detectStyle(currency: string): CurrencyStyle {
	const known = isoCurrencies ? isoCurrencies.includes(currency) : currency.length === 3;
	if (!known) return { symbol: currency, placement: "suffix", spaced: currency !== "" };
	const options = {
		style: "currency",
		currency,
		minimumFractionDigits: 2,
		maximumFractionDigits: 2,
	} as const;
	try {
		let parts = new Intl.NumberFormat(locale, { ...options, currencyDisplay: "symbol" }).formatToParts(
			1234.5
		);
		let symbolIndex = parts.findIndex((part) => part.type === "currency");
		if (parts[symbolIndex].value === currency) {
			parts = new Intl.NumberFormat(locale, {
				...options,
				currencyDisplay: "narrowSymbol",
			}).formatToParts(1234.5);
			symbolIndex = parts.findIndex((part) => part.type === "currency");
		}
		const integerIndex = parts.findIndex((part) => part.type === "integer");
		const fractionIndex = parts.findIndex((part) => part.type === "fraction");
		const placement: CurrencyPlacement =
			symbolIndex < integerIndex ? "prefix" : symbolIndex < fractionIndex ? "middle" : "suffix";
		const neighbour = parts[symbolIndex + (placement === "prefix" ? 1 : -1)]?.value ?? "";
		return { symbol: parts[symbolIndex].value, placement, spaced: /\s/.test(neighbour) };
	} catch {
		return { symbol: currency, placement: "suffix", spaced: currency !== "" };
	}
}

// Prototype-less so a commodity can never collide with an inherited key.
const styleCache: Record<string, CurrencyStyle> = Object.create(null);

/** Style for `currency`, derived from CLDR once and then cached. */
function currencyStyle(currency: string): CurrencyStyle {
	const cached = styleCache[currency];
	if (cached) return cached;
	const style = detectStyle(currency);
	styleCache[currency] = style;
	return style;
}

/**
 * Exact sum of decimal strings such as `-123.45`, at the widest scale among
 * them: values are scaled to a common power of ten and added as integers, so
 * money never passes through a float. `[]` sums to `0`.
 */
export function sumDecimals(values: string[]): string {
	let scale = 0;
	for (const value of values) {
		const point = value.indexOf(".");
		if (point !== -1 && value.length - point - 1 > scale) scale = value.length - point - 1;
	}
	let total = 0n;
	for (const value of values) {
		const negative = value.startsWith("-");
		const [whole, fraction = ""] = (negative ? value.slice(1) : value).split(".");
		const scaled = BigInt(`${whole}${fraction.padEnd(scale, "0")}`);
		total += negative ? -scaled : scaled;
	}
	const digits = (total < 0n ? -total : total).toString().padStart(scale + 1, "0");
	const text = scale === 0 ? digits : `${digits.slice(0, -scale)}.${digits.slice(-scale)}`;
	return total < 0n ? `-${text}` : text;
}

type MoneySegment = {
	text: string;
	/** `symbol` is the currency symbol or commodity code; `amount` is sign and digits. */
	kind: "symbol" | "amount";
};

/**
 * `units` (exact decimal string, or null for a node without a balance) as
 * ordered runs to render: the sign and digits are `amount`, the currency symbol
 * or commodity code is `symbol`, and this module — not the caller — decides the
 * order from the currency's placement. A no-break space (some currencies write
 * one) travels with the symbol run, so styling that run cannot detach it.
 * `signed: false` drops the +/- run, for a column whose heading already says
 * the direction.
 */
export function moneySegments(
	units: string | null,
	currency: string,
	{ signed = true }: { signed?: boolean } = {}
): MoneySegment[] {
	if (units === null) return [];
	const text = units.trim();
	const sign = signOf(text);
	const [whole = "0", fraction = ""] = (text.startsWith("-") ? text.slice(1) : text).split(".");
	// Never round: keep the ledger's own scale, padded to at least two places.
	const fractionText = fraction.length >= 2 ? fraction : fraction.padEnd(2, "0");
	const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, GROUP_SEPARATOR);
	const digits = `${grouped}${DECIMAL_SEPARATOR}${fractionText}`;
	const { symbol, placement, spaced } = currencyStyle(currency);
	const gap = spaced ? "\u00a0" : "";
	const signText = signed && sign !== 0 ? (sign < 0 ? "-" : "+") : "";
	const segments =
		placement === "prefix"
			? [
					{ text: signText, kind: "amount" as const },
					{ text: symbol + gap, kind: "symbol" as const },
					{ text: digits, kind: "amount" as const },
				]
			: placement === "middle"
				? [
						{ text: signText + grouped, kind: "amount" as const },
						{ text: gap + symbol, kind: "symbol" as const },
						{ text: DECIMAL_SEPARATOR + fractionText, kind: "amount" as const },
					]
				: [
						{ text: signText + digits, kind: "amount" as const },
						{ text: gap + symbol, kind: "symbol" as const },
					];
	return segments.filter((segment) => segment.text !== "");
}
