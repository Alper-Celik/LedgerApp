<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# LedgerApp

A ledger for Beancount journals — desktop on Linux/Windows/macOS and Android. The account tree
and register are drawn the way GnuCash draws them; edits go into the `.bean` file itself and are
undoable whether or not the journal is a git repository. Around that: an envelope budget, saved
dashboards and charts, prices for what the ledger holds, inflation and lifestyle comparisons, a
sync-conflict resolver for journals living in a cloud folder, and an assistant that answers from
the ledger through typed tools.

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
without restarting; the chooser is the desktop's own (xdg-desktop-portal on Linux).

For the phone, the same Rust code builds an APK:

```sh
scripts/android-build.sh                 # debug APK for aarch64
scripts/android-build.sh --apk            # release APK (release is the default)
```

which runs `npm run tauri android build` inside `nix develop .#android` (SDK, NDK, JDK 17 and a
rustc with the Android std targets). No device or emulator is needed to build it.

A release build is unsigned until a keystore is configured:

```sh
scripts/android-keystore.sh    # writes app/upload.jks + app/keystore.properties (git-ignored)
```

(or pass `STORE_FILE`, `STORE_PASSWORD`, `KEY_ALIAS`, `KEY_PASSWORD` to the build). Keep that
keystore: the same key has to sign every update.

## Command line

`src-tauri` also builds a `ledger` binary over the same backend — no Tauri runtime, no second
loader: every subcommand calls `api.rs`, the Tauri-free façade the commands wrap.

```sh
cargo build --bin ledger            # inside src-tauri
./target/debug/ledger check books/main.bean
./target/debug/ledger query 'SELECT account, sum(number) GROUP BY account' --limit 5
./target/debug/ledger budget --months 3
./target/debug/ledger report net-worth --months 6
./target/debug/ledger chart spec.json --width 80
```

The journal is `--ledger PATH` (a file or a folder holding one), else `LEDGER_PATH`, else
`./main.bean`. The app's config directory (indexes, assistant settings) is `--config-dir DIR`, else
`LEDGER_CONFIG_DIR`, else the platform's app-config path for `dev.alper-celik.ledger-app`.
Every read command takes `--json`; money is always the backend's exact decimal string.
Exit codes: `0` answered, `1` failed (reason on stderr), `2` misuse.

## Layout

| Path | What lives there |
|---|---|
| `src-tauri/src/commands.rs` | the Tauri/specta command surface |
| `src-tauri/src/api.rs` | the Tauri-free façade over the same modules, plain data out; the `ledger` binary's whole backend |
| `src-tauri/src/bin/ledger.rs` | the `ledger` CLI (`check`, `query`, `budget`, `report`, `prices`, `index`, `ask`, `chart`) |
| `src-tauri/src/` | one module per concern: ledger views (`accounts`, `source`, `files`, `budget`, `reports`), edits (`editing`, `rewrite`, `conflict*`), storage and history (`session`, `storage`, `repo`, `git`, `git_ops`, `history`), fetchers (`prices`, `index*`), assistant (`ai`, `ai_cmd`, `attachments`, `tools`, `conversations`), `lsp` (see `AGENTS.md`) |
| `src/lib/components/` | register, sidebar, source editor, diff, folder picker, assistant, budget, dashboards, inflation, prices, conflicts |
| `src/lib/*.svelte.ts` | shared stores (filters, editor scope, ledger, git state, sidebar width) |
| `crates/chart-render` | chart spec → PNG / terminal (kitty graphics or Unicode) |
| `vendor/TauriQt` | git submodule: the Qt runtime, taken by path |
| `plugins/rustledger-envelope` | the envelope-budgeting WASM plugin (own crate, own flake output) |
| `src-tauri/gen/android` | the generated Gradle project `tauri android init` emits |
| `books/`, `main.bean` | ignored/private: the actual ledger lives outside this repo |

## Checks

```sh
cd src-tauri && cargo test --lib        # backend unit tests
cd crates/chart-render && cargo test    # the chart crate's own tests
npx svelte-check --tsconfig ./tsconfig.json
npx playwright test                     # e2e against a stubbed Tauri IPC
reuse lint                              # SPDX on every file
```

Runtime work has its own gate inside the submodule: `scripts/smoke.sh` drives a headless demo
that grades itself (`DEMO_OK`) — see `vendor/TauriQt/AGENTS.md`.

## Licence

AGPL-3.0-or-later. Every file carries SPDX tags; `reuse lint` is the gate.
