// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The Python sandbox's main-thread half.
 *
 * The worker is started lazily by the first `run_python` call and kept warm
 * afterwards: loading Pyodide costs seconds, and the panel may run several
 * scripts in one conversation. One run is live at a time — [`runPython`] chains
 * calls, so a second script waits for the first instead of interleaving its
 * stdout.
 *
 * Pyodide cannot be interrupted (there is no signal a wasm interpreter checks),
 * so the timeout is the hard way: `terminate()` the worker, answer the run with
 * the output collected so far, and let the next call boot a fresh worker. A
 * crash is handled the same way. Every failure is a `PythonRun` with `error`
 * set — never a rejection — because the result goes back to the model as text.
 *
 * A `ledger.*` call arrives as an RPC from the worker and is answered by the
 * executor passed in by `tools.ts`; that keeps the Tauri bridge on this side
 * and the sandbox unable to reach anything but those reads.
 */
import { base } from "$app/paths";
import { pythonRunOf, type PythonRun } from "../artifacts/types.js";
import { messageOf } from "../../errors.js";
import type {
	DoneMessage,
	InitMessage,
	PythonRpcAnswer,
	RpcResultMessage,
	RunMessage,
	WorkerMessage,
} from "./protocol.js";

/** One `ledger.*` call, run through the panel's tool executor. */
export type LedgerRpc = (name: string, argsJson: string) => Promise<PythonRpcAnswer>;

/** The caps a run gets; the model cannot raise them from its script. */
export type RunOptions = {
	/** Wall-clock budget for the script itself, after the sandbox is warm. */
	timeoutMs?: number;
	/** How many characters of stdout the run keeps. */
	maxOutput?: number;
};

/** A run's defaults: twenty seconds and 200 000 characters. */
export const DEFAULT_TIMEOUT_MS = 20_000;
export const DEFAULT_MAX_OUTPUT = 200_000;

/** How long the first run may wait for Pyodide to load before giving up. */
const INIT_TIMEOUT_MS = 60_000;

/** A timer handle: `number` in the browser, a `Timeout` under Node's typings. */
type TimerHandle = ReturnType<typeof setTimeout>;

/** One run the worker is executing, and where its answer goes. */
type ActiveRun = {
	id: number;
	rpc: LedgerRpc;
	partial: { text: string };
	maxOutput: number;
	settle: (run: PythonRun) => void;
	timer: TimerHandle;
};

/** The state of the one sandbox the panel keeps. */
class PythonSandbox {
	private worker: Worker | null = null;
	private ready: Promise<void> | null = null;
	private readySettle: { resolve: () => void; reject: (cause: Error) => void } | null = null;
	private active: ActiveRun | null = null;
	private nextRun = 1;

	/** Run one script, resolving with its result whatever happens. */
	async run(code: string, options: RunOptions, rpc: LedgerRpc): Promise<PythonRun> {
		const timeoutMs = Math.max(1, options.timeoutMs ?? DEFAULT_TIMEOUT_MS);
		const maxOutput = Math.max(1, options.maxOutput ?? DEFAULT_MAX_OUTPUT);

		try {
			await this.ensure();
		} catch (cause) {
			return failed(`the Python sandbox could not start: ${messageOf(cause)}`);
		}
		const worker = this.worker;
		if (!worker || this.worker !== worker) return failed("the Python sandbox is not running");

		const id = this.nextRun++;
		const partial = { text: "" };
		const { promise, resolve } = Promise.withResolvers<PythonRun>();
		const timer = setTimeout(() => {
			if (this.active?.id !== id) return;
			this.active = null;
			this.stop();
			resolve({
				stdout: partial.text,
				error: `the run was stopped after ${timeoutMs} ms; the sandbox was restarted`,
				tables: [],
				chart: null,
			});
		}, timeoutMs);

		this.active = { id, rpc, partial, maxOutput, settle: resolve, timer };
		worker.postMessage({ type: "run", id, code, maxOutput } satisfies RunMessage);
		return promise;
	}

	/** The worker, booted once and kept for the next run. */
	private ensure(): Promise<void> {
		if (this.ready) return this.ready;
		const ready = this.boot();
		this.ready = ready;
		// A boot that never finished must not leave a dead worker behind.
		ready.catch(() => this.stop());
		return ready;
	}

	private boot(): Promise<void> {
		const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
		this.worker = worker;
		worker.onmessage = (event: MessageEvent) => this.receive(event.data as WorkerMessage);
		worker.onerror = (event: ErrorEvent) =>
			this.crashed(event.message || "the Python sandbox stopped");
		worker.onmessageerror = () =>
			this.crashed("a message from the Python sandbox could not be read");

		const { promise, resolve, reject } = Promise.withResolvers<void>();
		const timer = setTimeout(
			() => reject(new Error(`Pyodide did not load within ${INIT_TIMEOUT_MS / 1000} s`)),
			INIT_TIMEOUT_MS
		);
		this.readySettle = {
			resolve: () => {
				clearTimeout(timer);
				resolve();
			},
			reject: (cause) => {
				clearTimeout(timer);
				reject(cause);
			},
		};
		worker.postMessage({ type: "init", indexURL: pyodideIndexURL() } satisfies InitMessage);
		return promise;
	}

	private receive(message: WorkerMessage): void {
		switch (message.type) {
			case "ready":
				this.readySettle?.resolve();
				this.readySettle = null;
				return;

			case "fatal":
				this.readySettle?.reject(new Error(message.error));
				this.readySettle = null;
				this.stop();
				return;

			case "stdout":
				if (this.active) appendCapped(this.active.partial, message.text, this.active.maxOutput);
				return;

			case "rpc":
				this.answerRpc(message.id, message.name, message.args);
				return;

			case "done":
				this.finish(message);
				return;
		}
	}

	/** Hand one `ledger.*` call to the executor and post its answer back. */
	private answerRpc(id: number, name: string, args: string): void {
		const origin = this.worker;
		const active = this.active;
		if (!origin) return;
		if (!active) {
			origin.postMessage({
				type: "rpc-result",
				id,
				ok: false,
				text: JSON.stringify({ error: "no run is active" }),
			} satisfies RpcResultMessage);
			return;
		}

		const reply = (answer: PythonRpcAnswer) => {
			// Only the worker that asked gets the answer: a timed-out run's
			// executor may settle after a fresh worker has started, and its ids
			// would collide with the new worker's.
			if (this.worker !== origin) return;
			origin.postMessage({
				type: "rpc-result",
				id,
				ok: answer.ok,
				text: answer.text,
			} satisfies RpcResultMessage);
		};

		void active.rpc(name, args).then(reply, (cause: unknown) =>
			reply({ ok: false, text: JSON.stringify({ error: messageOf(cause) }) })
		);
	}

	/** A finished run: its result is the worker's own answer. */
	private finish(message: DoneMessage): void {
		const active = this.active;
		if (!active || active.id !== message.id) return;
		this.active = null;
		clearTimeout(active.timer);
		active.settle(pythonRunOf(message) ?? failed("the sandbox answered with an unreadable result"));
	}

	/** A worker that died: the run it was serving fails, the next one reboots. */
	private crashed(message: string): void {
		const active = this.active;
		this.stop();
		if (!active) return;
		clearTimeout(active.timer);
		active.settle(failed(`the Python sandbox crashed: ${message}; it restarts on the next run`));
	}

	/** Drop the worker; the next run boots a new one. */
	private stop(): void {
		const worker = this.worker;
		const settle = this.readySettle;
		this.worker = null;
		this.ready = null;
		this.readySettle = null;
		this.active = null;
		// A boot nobody will finish must fail its waiter now rather than at the
		// init timeout.
		settle?.reject(new Error("the Python sandbox stopped before it was ready"));
		worker?.terminate();
	}
}

/**
 * Where the runtime's files are served from: the Vite plugin copies the
 * `pyodide` package's runtime, and the vendored wheels under `packages/`, next
 * to the app's assets, under `/pyodide/`. The path is absolute, so
 * `location.href` only supplies scheme and host — which also works under
 * Tauri's custom protocol.
 */
function pyodideIndexURL(): string {
	const root = base.endsWith("/") ? base.slice(0, -1) : base;
	return new URL(`${root}/pyodide/`, location.href).href;
}

/** Append a streamed fragment, keeping at most `limit` characters. */
function appendCapped(partial: { text: string }, text: string, limit: number): void {
	const room = limit - partial.text.length;
	if (room <= 0) return;
	partial.text += text.length <= room ? text : text.slice(0, room);
}

/** A run that never produced a result, as the text the model reads. */
function failed(error: string): PythonRun {
	return { stdout: "", error, tables: [], chart: null };
}

const sandbox = new PythonSandbox();

/**
 * The queue that keeps one run live at a time. Each call waits for the one
 * before it, so two scripts cannot interleave stdout or fight over the worker.
 */
let tail: Promise<unknown> = Promise.resolve();

/** Run one script in the sandbox; resolves with the run's result, never rejects. */
export function runPython(code: string, options: RunOptions, rpc: LedgerRpc): Promise<PythonRun> {
	const job = tail
		.then(() => sandbox.run(code, options, rpc))
		.catch((cause: unknown) => failed(messageOf(cause)));
	tail = job.then(
		() => undefined,
		() => undefined
	);
	return job;
}
