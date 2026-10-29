// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error Node's types are not part of the app's dependencies.
import { createReadStream, readdirSync, readFileSync } from "node:fs";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

/**
 * The files Pyodide's loader fetches from its `indexURL` directory: the
 * loader itself, the wasm interpreter, the standard library and the lockfile.
 * The wheels the lockfile names for packages the runtime does not bundle
 * (`pandas`, `numpy` and their dependencies) are vendored separately, below.
 */
const PYODIDE_FILES = [
  "pyodide.mjs",
  "pyodide.asm.mjs",
  "pyodide.asm.wasm",
  "python_stdlib.zip",
  "pyodide-lock.json",
];

/**
 * The vendored wheels, relative to the project root. They are served under
 * `/pyodide/packages/`, next to the runtime files, which is the directory
 * `loadPyodide` gets as `packageBaseUrl`. Names are the lockfile's own: Pyodide
 * looks each package up by `file_name`, so they cannot be hashed.
 */
const PYODIDE_PACKAGES = "src/lib/assistant/python/packages";

/**
 * The extension of a file name, including its dot.
 * @param {string} file
 */
function extensionOf(file) {
  const dot = file.lastIndexOf(".");
  return dot < 0 ? "" : file.slice(dot);
}

/** @type {Record<string, string>} */
const PYODIDE_TYPES = {
  ".mjs": "text/javascript; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".zip": "application/zip",
  ".json": "application/json; charset=utf-8",
  ".whl": "application/octet-stream",
};

/**
 * Serve the installed `pyodide` package's runtime, and the wheels vendored
 * under `PYODIDE_PACKAGES`, at `/pyodide/`, in dev and in the built app, so
 * the Python sandbox loads offline from the app's own files instead of a CDN.
 *
 * The files keep their published names — Pyodide resolves the runtime files
 * relative to `indexURL` and the wheels relative to `packageBaseUrl`, so they
 * cannot be hashed like other assets. Dev and preview get a middleware; the
 * client build emits the same files with `emitFile`, which adapter-static then
 * copies into the output directory.
 *
 * @returns {import("vite").Plugin}
 */
function pyodideAssets() {
  /** @type {string} */
  let root = "";
  /** @type {string[]} */
  let packages = [];

  /**
   * The file a request names, or `undefined` when it names nothing this plugin
   * serves. `/pyodide/packages/<wheel>` is the wheel directory; anything else
   * directly under `/pyodide/` is a runtime file.
   *
   * @param {string} path
   * @returns {{ file: string; name: string } | undefined}
   */
  const resolve = (path) => {
    const match = /^\/pyodide\/(packages\/)?([^/]+)$/.exec(path);
    if (!match) return undefined;
    const name = match[2];
    if (match[1]) {
      return packages.includes(name)
        ? { file: `${root}/${PYODIDE_PACKAGES}/${name}`, name }
        : undefined;
    }
    return PYODIDE_FILES.includes(name)
      ? { file: `${root}/node_modules/pyodide/${name}`, name }
      : undefined;
  };

  /**
   * @param {any} req
   * @param {any} res
   * @param {() => void} next
   */
  const serve = (req, res, next) => {
    const target = resolve((req.url ?? "").split("?")[0]);
    if (!target) {
      next();
      return;
    }
    res.setHeader(
      "Content-Type",
      PYODIDE_TYPES[extensionOf(target.name)] ?? "application/octet-stream"
    );
    res.setHeader("Cache-Control", "public, max-age=3600");
    createReadStream(target.file)
      .on("error", () => next())
      .pipe(res);
  };

  return {
    name: "ledger-pyodide-assets",

    configResolved(config) {
      root = config.root;
      /** @type {string[]} */
      const files = readdirSync(`${root}/${PYODIDE_PACKAGES}`);
      packages = files.filter((file) => file.endsWith(".whl"));
    },

    // Before SvelteKit's own dev middleware, which would answer 404 for a path
    // no route matches.
    configureServer(server) {
      server.middlewares.use(serve);
    },

    configurePreviewServer(server) {
      server.middlewares.use(serve);
    },

    buildStart() {
      // The runtime is for the browser; the SSR build has no page to serve it to.
      if (this.environment?.name === "ssr") return;
      for (const file of PYODIDE_FILES) {
        this.emitFile({
          type: "asset",
          fileName: `pyodide/${file}`,
          source: readFileSync(`${root}/node_modules/pyodide/${file}`),
        });
      }
      for (const file of packages) {
        this.emitFile({
          type: "asset",
          fileName: `pyodide/packages/${file}`,
          source: readFileSync(`${root}/${PYODIDE_PACKAGES}/${file}`),
        });
      }
    },
  };
}

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [pyodideAssets(), tailwindcss(), sveltekit()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
