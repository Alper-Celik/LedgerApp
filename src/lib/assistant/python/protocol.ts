// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The messages the Python sandbox and its worker exchange.
 *
 * A run starts on the main thread and streams stdout back; a `ledger.*` call is
 * an RPC the worker posts to the main thread and awaits, so the executor stays
 * on the side that owns the Tauri bridge. Every message is plain data — only
 * strings, numbers and booleans cross the boundary — so nothing here needs a
 * structured-clone-compatible class.
 */
import type { ChartSpec, TableArtifact } from "../artifacts/types.js";

/** The answer to one RPC: the executor's `ok`, and its JSON text. */
export type PythonRpcAnswer = { ok: boolean; text: string };

/** Main thread → worker: load Pyodide from `indexURL`, then answer `ready`. */
export type InitMessage = { type: "init"; indexURL: string };

/** Main thread → worker: run one script, streaming stdout until `done`. */
export type RunMessage = { type: "run"; id: number; code: string; maxOutput: number };

/** Main thread → worker: the answer to one `ledger.*` call. */
export type RpcResultMessage = { type: "rpc-result"; id: number; ok: boolean; text: string };

/** Worker → main thread: one `ledger.*` call to run through the tool executor. */
export type RpcMessage = { type: "rpc"; id: number; name: string; args: string };

/** Worker → main thread: a fragment of the run's stdout (or stderr). */
export type StdoutMessage = { type: "stdout"; text: string };

/** Worker → main thread: the run is over, with the artifacts it collected. */
export type DoneMessage = {
	type: "done";
	id: number;
	stdout: string;
	error: string;
	tables: TableArtifact[];
	chart: ChartSpec | null;
};

/** Worker → main thread: Pyodide is loaded and the `ledger` module installed. */
export type ReadyMessage = { type: "ready" };

/** Worker → main thread: the sandbox could not start; this worker is spent. */
export type FatalMessage = { type: "fatal"; error: string };

/** Everything the worker may send. */
export type WorkerMessage = RpcMessage | StdoutMessage | DoneMessage | ReadyMessage | FatalMessage;

/** Everything the main thread may send. */
export type SandboxMessage = InitMessage | RunMessage | RpcResultMessage;
