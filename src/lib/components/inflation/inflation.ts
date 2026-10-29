// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import type { IndexSeries, LifestyleReport } from "../../../bindings";

/**
 * The month ranges the lifestyle screen offers, in the order they are shown.
 * Each is a count of months the backend's report walks back from the newest
 * month it can see, not a date range the frontend picks.
 */
export const RANGES = [6, 12, 24] as const;

/** The ranges as menu labels: `6 months`, `12 months`, `24 months`. */
export function rangeLabel(months: number): string {
	return `${months} months`;
}

/** `1 point` / `0 points`, so a card never reads `1 points`. */
export function pointsLabel(count: number): string {
	return count === 1 ? "1 point" : `${count} points`;
}

/** The newest point of a series, or `null` for one that holds none. */
export function lastPoint(series: IndexSeries): IndexSeries["points"][number] | null {
	return series.points.at(-1) ?? null;
}

type Decimal = { negative: boolean; whole: string; fraction: string };

/** A plain decimal string (`-1234.50`, `125`, `0.00`) split into its digits. */
function parseDecimal(text: string): Decimal | null {
	const match = /^([+-]?)(\d+)(?:\.(\d*))?$/.exec(text.trim());
	if (!match) return null;
	return { negative: match[1] === "-", whole: match[2] ?? "0", fraction: match[3] ?? "" };
}

/** The value at `scale` decimal places, as an integer. */
function scaledAt(value: Decimal, scale: number): bigint {
	const magnitude = BigInt(`${value.whole}${value.fraction.padEnd(scale, "0")}`);
	return value.negative ? -magnitude : magnitude;
}

/** Integer division rounded to the nearest integer, midpoint away from zero. */
function divideRounded(numerator: bigint, denominator: bigint): bigint {
	const negative = (numerator < 0n) !== (denominator < 0n);
	const top = numerator < 0n ? -numerator : numerator;
	const bottom = denominator < 0n ? -denominator : denominator;
	const quotient = top / bottom;
	const rounded = (top % bottom) * 2n >= bottom ? quotient + 1n : quotient;
	return negative ? -rounded : rounded;
}

/**
 * `latest` against `previous` in percent, two decimals: `-50.00`, `25.00`.
 *
 * The same arithmetic the backend's `percentage_change` does — exact decimal
 * integers, one rounding at the end, midpoint away from zero — so a figure
 * worked out here agrees with one the report already returned. `null` when
 * either side is not a decimal string or `previous` is zero: there is no
 * honest percentage against nothing.
 */
export function percentChange(previous: string, latest: string): string | null {
	const before = parseDecimal(previous);
	const after = parseDecimal(latest);
	if (!before || !after) return null;
	const scale = Math.max(before.fraction.length, after.fraction.length);
	const start = scaledAt(before, scale);
	const end = scaledAt(after, scale);
	if (start === 0n) return null;
	// `* 10000` keeps two decimals of a percentage: hundredths of a percent.
	const hundredths = divideRounded((end - start) * 10000n, start);
	const magnitude = hundredths < 0n ? -hundredths : hundredths;
	const whole = magnitude / 100n;
	const fraction = (magnitude % 100n).toString().padStart(2, "0");
	return `${hundredths < 0n ? "-" : ""}${whole}.${fraction}`;
}

/**
 * A backend change (`"3.81"`, `"-1.20"`) as it is read: `+3.81%`, `-1.20%`,
 * `0.00%`, and an em dash for a change the report could not compute.
 */
export function percentText(change: string | null): string {
	if (change === null) return "—";
	if (Number(change) === 0) return "0.00%";
	return `${change.startsWith("-") ? "" : "+"}${change}%`;
}

/**
 * Whether a spending change beat the index's own movement — the one comparison
 * the table marks red. Real spending growing faster than prices means the same
 * money bought more (or better) than in the month it is measured from, which is
 * the lifestyle inflation this screen is named after.
 */
export function outpaces(change: string | null, index: string | null): boolean {
	if (change === null || index === null) return false;
	return Number(change) > Number(index);
}

/** The real change from the first month of a row of months to the last. */
export function realChange(months: readonly { real: string }[]): string | null {
	if (months.length < 2) return null;
	const first = months[0];
	const last = months[months.length - 1];
	return first && last ? percentChange(first.real, last.real) : null;
}

/** The nominal change from the first month of a row of months to the last. */
export function nominalChange(months: readonly { nominal: string }[]): string | null {
	if (months.length < 2) return null;
	const first = months[0];
	const last = months[months.length - 1];
	return first && last ? percentChange(first.nominal, last.nominal) : null;
}

/**
 * The range a report covers, in its own months: `2025-07 → 2026-06`, or the
 * single month when the report holds one.
 */
export function rangeText(report: LifestyleReport): string {
	const months = report.months.map((one) => one.month);
	const first = months[0];
	const last = months[months.length - 1];
	if (!first || !last) return "no months";
	return first === last ? first : `${first} → ${last}`;
}

/** What the whole range's spending did in real terms, against its first month. */
export function spendingRealChange(report: LifestyleReport): string | null {
	return realChange(report.totals);
}
