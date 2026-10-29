// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * What a Python run brings back into the transcript.
 *
 * A run answers with prose on stdout, the traceback when the script raised,
 * the tables it built with `ledger.table`, and at most one chart drawn with
 * `ledger.chart`. Amounts stay exact decimal strings here, exactly as the
 * sandbox wrote them: the chart wrappers read those strings and never a float.
 */

/** The chart shapes `$lib/charts/*` can draw. */
export type ChartKind = "line" | "bar" | "area" | "donut";

/** One point on a chart's category axis, e.g. a month and an amount. */
export type ChartPoint = { label: string; value: string };

/** One named series of points; a donut draws the first series as its slices. */
export type ChartSeriesSpec = { name: string; points: ChartPoint[] };

/**
 * One chart a run asked for, before the currency is folded into each point.
 *
 * This is the wire shape `ledger.chart({...})` takes and the transcript keeps;
 * [`$lib/components/assistant/chart-artifact.svelte`] turns it into the props
 * `$lib/charts/*` expect.
 */
export type ChartSpec = {
	title?: string;
	kind: ChartKind;
	/** ISO 4217 code or commodity every amount in the chart is written in. */
	currency: string;
	series: ChartSeriesSpec[];
};

/** One table a run built: a name, its columns, and every cell as written. */
export type TableArtifact = {
	name: string;
	columns: string[];
	rows: string[][];
	/** True when the sandbox capped the rows or columns it kept. */
	truncated?: boolean;
};

/** What one `run_python` call answers — the sandbox's own result contract. */
export type PythonRun = {
	stdout: string;
	/** The traceback when the script raised, `""` when it finished. */
	error: string;
	tables: TableArtifact[];
	chart: ChartSpec | null;
};

const KINDS: readonly string[] = ["line", "bar", "area", "donut"];

/**
 * A chart spec as it crossed the worker boundary, or `null` when it is not
 * one. The worker answers JSON that the panel did not compute, so the shape is
 * checked rather than trusted — a malformed spec is dropped, never thrown.
 */
export function chartSpecOf(value: unknown): ChartSpec | null {
	if (!value || typeof value !== "object") return null;
	const record = value as Record<string, unknown>;
	if (typeof record.kind !== "string" || !KINDS.includes(record.kind)) return null;
	if (typeof record.currency !== "string" || record.currency.trim() === "") return null;
	if (!Array.isArray(record.series)) return null;

	const series: ChartSeriesSpec[] = [];
	for (const entry of record.series) {
		if (!entry || typeof entry !== "object") return null;
		const one = entry as Record<string, unknown>;
		if (typeof one.name !== "string" || !Array.isArray(one.points)) return null;
		const points: ChartPoint[] = [];
		for (const point of one.points) {
			if (!point || typeof point !== "object") return null;
			const pair = point as Record<string, unknown>;
			if (typeof pair.label !== "string" || typeof pair.value !== "string") return null;
			points.push({ label: pair.label, value: pair.value });
		}
		series.push({ name: one.name, points });
	}

	const spec: ChartSpec = {
		kind: record.kind as ChartKind,
		currency: record.currency,
		series,
	};
	if (typeof record.title === "string" && record.title.trim() !== "") {
		spec.title = record.title;
	}
	return spec;
}

/** One table as it crossed the worker boundary, or `null` when it is not one. */
export function tableArtifactOf(value: unknown): TableArtifact | null {
	if (!value || typeof value !== "object") return null;
	const record = value as Record<string, unknown>;
	if (typeof record.name !== "string") return null;
	if (!Array.isArray(record.columns) || !Array.isArray(record.rows)) return null;
	if (!record.columns.every((column) => typeof column === "string")) return null;

	const rows: string[][] = [];
	for (const row of record.rows) {
		if (!Array.isArray(row) || !row.every((cell) => typeof cell === "string")) return null;
		rows.push(row as string[]);
	}

	const table: TableArtifact = {
		name: record.name,
		columns: record.columns as string[],
		rows,
	};
	if (record.truncated === true) table.truncated = true;
	return table;
}

/** A finished run as it crossed the worker boundary, or `null` when it is not one. */
export function pythonRunOf(value: unknown): PythonRun | null {
	if (!value || typeof value !== "object") return null;
	const record = value as Record<string, unknown>;
	if (typeof record.stdout !== "string" || typeof record.error !== "string") return null;
	if (!Array.isArray(record.tables)) return null;

	const tables: TableArtifact[] = [];
	for (const table of record.tables) {
		const parsed = tableArtifactOf(table);
		if (!parsed) return null;
		tables.push(parsed);
	}
	return {
		stdout: record.stdout,
		error: record.error,
		tables,
		chart: chartSpecOf(record.chart),
	};
}
