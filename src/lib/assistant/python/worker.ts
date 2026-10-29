// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The Python sandbox worker.
 *
 * Pyodide is loaded here, not on the page: a script can spin for as long as it
 * likes without freezing the app, and the only way out of a runaway loop —
 * `terminate()` — costs nothing but a fresh worker. The runtime is served from
 * the app's own `/pyodide/` assets, which the Vite plugin in `vite.config.js`
 * copies from the installed `pyodide` package; only when those are absent does
 * this fall back to the jsDelivr CDN, and it says so on the console.
 *
 * Everything needed to start — the loader, the wasm interpreter, the standard
 * library and the lockfile — is in those assets, so the sandbox runs offline.
 * The CSP's one remote origin, jsDelivr, exists solely for wheels the npm
 * distribution does not ship (pandas and its dependencies); vendoring those
 * wheels beside the runtime would remove the last network use.
 *
 * The `ledger` module is installed once from `ledger.py`. Its functions are
 * coroutines that post an RPC to the main thread and await the answer, because
 * the Tauri bridge lives there; the executor is the same one the model's tools
 * run through, so a sandbox figure and a tool figure cannot disagree. The
 * interpreter has no real filesystem and no subprocess: `os` and `pathlib` see
 * Pyodide's in-memory filesystem only, `subprocess` and socket access do not
 * exist, and nothing here can reach the app's process or its ledger files — the
 * RPC is the only way to a figure.
 */
import pyodideManifest from "pyodide/package.json";
import type { PyodideAPI } from "pyodide";
import type { PyProxy } from "pyodide/ffi";
import type {
	DoneMessage,
	PythonRpcAnswer,
	SandboxMessage,
	WorkerMessage,
} from "./protocol.js";
import ledgerSource from "./ledger.py?raw";

/** The little of the worker global scope this file uses. */
type WorkerScope = {
	postMessage(message: WorkerMessage): void;
	onmessage: ((event: MessageEvent) => void) | null;
	onmessageerror: ((event: MessageEvent) => void) | null;
};

/** The loader module as the package's `pyodide.mjs` exports it. */
type Loader = {
	loadPyodide(options: { indexURL: string; packageBaseUrl: string }): Promise<PyodideAPI>;
};

const scope = globalThis as unknown as WorkerScope;

/** How much of one traceback the worker keeps. */
const MAX_ERROR = 20_000;

/** The code that installs `ledger.py` as an importable module, once. */
const INSTALL = [
	"import sys, types",
	"_module = types.ModuleType('ledger')",
	"_module.__doc__ = 'The ledger read API of this sandbox; every function is a coroutine.'",
	"exec(compile(__ledger_source__, '<ledger>', 'exec'), _module.__dict__)",
	"sys.modules['ledger'] = _module",
	"del __ledger_source__",
].join("\n");

let runtime: PyodideAPI | null = null;
let booting: Promise<PyodideAPI> | null = null;

/** One run's stdout buffer, while a run is in progress. */
type Capture = { text: string; truncated: boolean };

let capture: Capture | null = null;
let maxOutput = 200_000;

/** RPCs waiting for the main thread's answer, by id. */
const waiting = new Map<number, (answer: PythonRpcAnswer) => void>();
let nextRpc = 1;

/**
 * Output arrives as bytes, not lines: Pyodide's line-batching drops the
 * newline and cannot tell a `print` from a partial `sys.stdout.write`, so the
 * writer below decodes the exact stream instead.
 */
const decoder = new TextDecoder();

/** Write one stdout/stderr buffer to the run's capture. */
function write(buffer: Uint8Array): number {
	emit(decoder.decode(buffer, { stream: true }));
	return buffer.length;
}

scope.onmessage = (event: MessageEvent) => {
	const message = event.data as SandboxMessage;
	if (!message || typeof message !== "object") return;
	switch (message.type) {
		case "init":
			void start(message.indexURL);
			return;
		case "run":
			void execute(message.id, message.code, message.maxOutput);
			return;
		case "rpc-result": {
			const settle = waiting.get(message.id);
			if (!settle) return;
			waiting.delete(message.id);
			settle({ ok: message.ok, text: message.text });
			return;
		}
	}
};

scope.onmessageerror = () => {
	post({ type: "fatal", error: "the sandbox could not read a message from the app" });
};

/** Send one message to the main thread. */
function post(message: WorkerMessage): void {
	scope.postMessage(message);
}

/** A thrown value as the text a traceback should show. */
function textOf(cause: unknown): string {
	const text = cause instanceof Error ? cause.message : String(cause);
	return text.length > MAX_ERROR ? `${text.slice(0, MAX_ERROR)}\n… the error was truncated` : text;
}

/** One `ledger.*` call, answered by the main thread's tool executor. */
function rpc(name: string, args: string): Promise<PythonRpcAnswer> {
	const { promise, resolve } = Promise.withResolvers<PythonRpcAnswer>();
	const id = nextRpc++;
	waiting.set(id, resolve);
	post({ type: "rpc", id, name, args });
	return promise;
}

/** Append one stdout/stderr fragment, capped at the run's output limit. */
function emit(text: string): void {
	if (!capture || capture.truncated) return;
	const room = maxOutput - capture.text.length;
	if (room <= 0) {
		capture.truncated = true;
		return;
	}
	const piece = text.length <= room ? text : text.slice(0, room);
	capture.text += piece;
	if (piece.length < text.length) capture.truncated = true;
	post({ type: "stdout", text: piece });
}

/**
 * Load the runtime from `indexURL`, falling back to the CDN when it is absent.
 *
 * The specifier is chosen at runtime — the app's own asset directory, or the
 * CDN — so a static import cannot name it; that is the whole point of the
 * fallback.
 */
async function load(indexURL: string): Promise<{ loader: Loader; indexURL: string; cdn: string }> {
	try {
		const loader = (await import(/* @vite-ignore */ `${indexURL}pyodide.mjs`)) as Loader;
		return { loader, indexURL, cdn: cdnURL() };
	} catch (cause) {
		const cdn = cdnURL();
		console.warn(
			`Pyodide was not found at ${indexURL}; loading it from ${cdn} instead, which needs a network connection.`,
			cause
		);
		const loader = (await import(/* @vite-ignore */ `${cdn}pyodide.mjs`)) as Loader;
		return { loader, indexURL: cdn, cdn };
	}
}

/** The jsDelivr directory matching this package's version. */
function cdnURL(): string {
	return `https://cdn.jsdelivr.net/pyodide/v${pyodideManifest.version}/full/`;
}

/** Boot Pyodide once and install the `ledger` module. */
function boot(indexURL: string): Promise<PyodideAPI> {
	if (runtime) return Promise.resolve(runtime);
	if (booting) return booting;
	booting = (async () => {
		const { loader, indexURL: resolved, cdn } = await load(indexURL);
		// The runtime and the standard library come from the app's assets; the
		// optional wheels (`pandas`) are only in the distribution, so packages
		// are fetched from the same CDN the runtime would have come from.
		const py = await loader.loadPyodide({ indexURL: resolved, packageBaseUrl: cdn });
		py.setStdout({ write });
		py.setStderr({ write });
		py.registerJsModule("ledger_rpc", { call: rpc });
		py.globals.set("__ledger_source__", ledgerSource);
		py.runPython(INSTALL);
		runtime = py;
		return py;
	})();
	booting.catch(() => {
		booting = null;
	});
	return booting;
}

/** Answer `init`: load the runtime and say when the sandbox is usable. */
async function start(indexURL: string): Promise<void> {
	try {
		await boot(indexURL);
		post({ type: "ready" });
	} catch (cause) {
		post({ type: "fatal", error: textOf(cause) });
	}
}

/** Answer `run`: one script, then its stdout, error and artifacts. */
async function execute(id: number, code: string, limit: number): Promise<void> {
	const py = runtime;
	if (!py) {
		post({ ...emptyRun(id), error: "the Python sandbox is not ready" });
		return;
	}
	if (capture) {
		post({ ...emptyRun(id), error: "another run is still in progress" });
		return;
	}

	maxOutput = Math.max(1, Math.min(limit, 2_000_000));
	capture = { text: "", truncated: false };

	let error = "";
	let tables: DoneMessage["tables"] = [];
	let chart: DoneMessage["chart"] = null;
	let globals: PyProxy | null = null;

	try {
		// A fresh globals dict per run: one script's names cannot leak into the
		// next, while imports and loaded packages stay warm in the interpreter.
		globals = py.runPython("{'__name__': '__main__'}") as PyProxy;
		await py.runPythonAsync(code, { globals, filename: "assistant.py" });
	} catch (cause) {
		error = textOf(cause);
	} finally {
		try {
			const drained = JSON.parse(
				String(py.runPython("import ledger; ledger._drain()"))
			) as {
				tables?: DoneMessage["tables"];
				chart?: DoneMessage["chart"];
			};
			tables = Array.isArray(drained.tables) ? drained.tables : [];
			chart = drained.chart ?? null;
		} catch {
			// A drain failure must not hide the run's own error.
		}
		globals?.destroy();
	}

	// A run that ended mid-codepoint still owes the tail of its output.
	const flushed = decoder.decode();
	if (flushed) emit(flushed);

	const stdout = capture.truncated
		? `${capture.text}\n… stdout truncated at ${maxOutput} characters`
		: capture.text;
	capture = null;
	post({ type: "done", id, stdout, error, tables, chart });
}

/** The shape of a run that never started. */
function emptyRun(id: number): DoneMessage {
	return { type: "done", id, stdout: "", error: "", tables: [], chart: null };
}
