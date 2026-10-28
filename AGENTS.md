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
- `session.rs` — `AppState`, the loaded `Ledger` plus its per-file baseline, ledger path resolution.
- `accounts.rs` — account tree balances and the register (`balance_view` walks, envelope filtering).
- `source.rs` — `balance_view` ↔ source-directive mapping, transaction span lookup, line/column helpers.
- `editing.rs` — the edit pipeline: write, reload, roll back on new loader errors, stage.
- `folders.rs` — the in-app folder picker's directory listing and root-journal detection.
- `rewrite.rs` — pure byte-range source surgery (header and posting rewrites).
- `git.rs` — working-tree change detection, diff rendering, committing the ledger's own files.
- `model.rs` — serde/specta types exported to `src/bindings.ts`.
- `testing.rs` — shared test fixtures (temp dirs, throwaway repos, loaded `AppState`); tests sit beside their module.

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
