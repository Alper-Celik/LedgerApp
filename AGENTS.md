<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

## Project Configuration

- **Language**: TypeScript
- **Package Manager**: npm
- **Add-ons**: paraglide, ai-tools

## Rust backend (`src-tauri/src`)

- `lib.rs` — crate root: runtime alias, module list, `run()` (specta registration + bindings export).
- `commands.rs` — the Tauri/specta command surface; frontend-facing names live here, keep them thin.
- **Ledger views** — `session.rs` (`AppState`, the loaded `Ledger` + baseline, path resolution), `accounts.rs`
  (tree balances, register rows), `source.rs` (`balance_view` ↔ source spans), `files.rs` (journal files,
  per-file transactions, the new-entry file suggestion), `budget.rs` (one month and a range of envelope
  budget), `reports.rs` (net worth, cash flow, category months, lifestyle inflation).
- **Edits** — `editing.rs` (write → reload → roll back on a *new* loader error → stage), `rewrite.rs` (pure
  byte-range surgery), `conflict.rs` (directive-level three-way merge), `conflict_cmd.rs` (compare/resolve
  over the operation log), `conflict_names.rs` (every sync client's conflict-copy naming).
- **Storage, git and history** — `storage.rs` (local vs Android SAF tree: `pick_ledger_folder` picks a
  tree on Android, mirrors it privately, and `sync_ledger_tree` writes changes back — the mirror is what
  the loader and git open, so git is off in tree mode), `repo.rs` (repo state/init/identity), `git.rs` (working-tree
  detection, diff, commit), `git_ops.rs` (branches, log, stage, merge, revert; not yet exposed as commands),
  `history.rs` (the operation log inside the ledger: per-device jsonl + pre-image blobs + `state.json`,
  which serves undo *and* merge bases without git).
- **Fetchers** — `prices.rs` + `prices/tefas.rs` (quotes via rustledger's `PriceSource`, TEFAS
  downstream, written to the journal through the edit pipeline), `index.rs` (index-as-price maths),
  `index_store.rs`, `index_providers.rs` (FRED, Eurostat, TÜİK, ENAG, ECB), `dashboards.rs` (the saved
  dashboard store).
- **Assistant** — `ai.rs` (OpenAI-compatible transport, settings that never leak the key), `ai_cmd.rs`
  (IPC surface, system prompt, tool table), `ai_stream.rs` (streaming reads forwarded as `ai://delta`),
  `query.rs` (BQL → text cells, the assistant's one aggregate tool), `suggest.rs` (quick-add's last pair
  and ranked categories), `tools/` (hashline read/edit, grep, glob, one registry rendered twice: OpenAI
  tools and a Python stub), `attachments.rs` (MuPDF pages, calamine sheets, CSV),
  `conversations.rs` (markdown conversations inside the ledger, so they sync with it).
- **CLI** — `api.rs` (a `pub`, Tauri-free façade over the same modules) and `src/bin/ledger.rs` (the
  `ledger` binary: check/query/budget/report/prices/index/ask/chart); `sync.rs` reports files a synced
  folder changed behind the app.
- `lsp.rs` — the embedded language server; `model.rs` — the original register/session payload types
  (newer payload types live beside their module); `folders.rs` — the in-app picker's listing;
  `testing.rs` — shared fixtures (temp dirs, throwaway repos, loaded `AppState`); tests sit beside their module.

## Qt runtime (`vendor/TauriQt`, git submodule)

`src-tauri/Cargo.toml` takes `tauri-runtime-qt` from that checkout by path, so runtime fixes are made there first and the app builds them immediately. That repo has its own AGENTS.md (commit as you go; Qt-bearing crates cannot link a test binary, so behaviour is verified by its demo + `scripts/smoke.sh`). The demo grades itself into the window title and the smoke script asserts `DEMO_OK`.

---

# Svelte MCP server

You are able to use the Svelte MCP server (`svelte`), where you have access to comprehensive Svelte 5 and SvelteKit documentation. Here's how to use the available tools effectively:

## Available Svelte MCP tools

### 1. list-sections

Use this FIRST to discover all available documentation sections. Returns a structured list with titles, use_cases, and paths.
When asked about Svelte or SvelteKit topics, ALWAYS use this tool at the start of the chat to find relevant sections.

### 2. get-documentation

Retrieves full documentation content for specific sections. Accepts single or multiple sections.
After calling the list-sections tool, analyze the returned documentation sections (especially the use_cases field) and then use the get-documentation tool to fetch ALL documentation sections that are relevant for the user's task.

### 3. svelte-autofixer

Analyzes Svelte code and returns issues and suggestions.
You MUST use this tool whenever writing Svelte code before sending it to the user. Keep calling it until no issues or suggestions are returned.

### 4. playground-link

Generates a Svelte Playground link with the provided code.
After completing the code, ask the user if they want a playground link. Only call this tool after user confirmation and NEVER if code was written to files in their project.
