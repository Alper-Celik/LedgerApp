<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# LedgerApp

A desktop ledger for Beancount journals: the account tree and register the way GnuCash draws
them, in-place and source-text editing that writes the `.bean` file itself, and a git-aware
diff/commit flow for the journal.

- **Backend** — Rust on [rustledger](https://github.com/rustledger/rustledger) (parse, book,
  validate, query), behind Tauri's [Qt runtime](vendor/TauriQt) (Qt windowing + QtWebEngine)
  on Linux and stock wry elsewhere.
- **Frontend** — SvelteKit + Svelte 5 runes, Tailwind and shadcn-svelte, Monaco for the source
  editor. Commands are typed end to end: `src-tauri/src/commands.rs` is exported to
  `src/bindings.ts` by specta.

## Running it

```sh
nix develop          # Qt merged prefix, Node, Rust, the wasm linker
npm install
npm run tauri dev    # builds the Rust side and serves the frontend on :1420
```

The journal to open is, in order: the last CLI argument, `LEDGER_PATH` (per-machine, e.g. via a
git-ignored `.envrc.local`), `main.bean` in the working directory. A directory is resolved to
the root journal inside it, and **Open folder…** (sidebar footer or Settings) switches journals
without restarting.

## Layout

| Path | What lives there |
|---|---|
| `src-tauri/src/commands.rs` | the Tauri/specta command surface |
| `src-tauri/src/{session,accounts,source,editing,rewrite,git,folders,model}.rs` | backend, one module per concern (see `AGENTS.md`) |
| `src/lib/components/` | register, sidebar, source editor, diff, folder picker |
| `src/lib/*.svelte.ts` | shared stores (filters, editor scope, ledger, sidebar width) |
| `vendor/TauriQt` | git submodule: the Qt runtime, taken by path |
| `plugins/rustledger-envelope` | the envelope-budgeting WASM plugin (own crate, own flake output) |
| `books/`, `main.bean` | ignored/private: the actual ledger lives outside this repo |

## Checks

```sh
cd src-tauri && cargo test --lib        # backend unit tests
npx svelte-check --tsconfig ./tsconfig.json
npx playwright test                     # e2e against a stubbed Tauri IPC
```

Runtime work has its own gate inside the submodule: `scripts/smoke.sh` drives a headless demo
that grades itself (`DEMO_OK`) — see `vendor/TauriQt/AGENTS.md`.

## Licence

AGPL-3.0-or-later. Every file carries SPDX tags; `reuse lint` is the gate.
