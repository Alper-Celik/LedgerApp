// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ledger app backend: the same concerns the window and the `ledger` binary
//! drive (see `AGENTS.md` for the module map — it is the one place that lists
//! them, so a new module is named there and not here).
//!
//! Two rules hold for every module: one that touches Tauri lives here or in
//! `commands`, and one that must run headless (the CLI's façade, the mirror, the
//! loader's views) does not — which is why the backend splits into a Tauri-bound
//! command layer over the same functions `api` exposes.

mod accounts;
mod ai;
mod ai_cmd;
mod ai_stream;
// The Tauri-free façade the `ledger` binary (and anything else headless)
// drives: the same backend the commands wrap, returning plain data.
pub mod api;
pub mod attachments;
mod budget;
mod budget_write;
pub mod commands;
mod conflict;
mod conflict_cmd;
mod conflict_names;
mod conversations;
mod dashboards;
mod editing;
mod files;
mod folders;
mod fsutil;
mod git;
mod git_ops;
mod history;
mod index;
mod index_price;
mod index_providers;
mod index_store;
mod lsp;
mod model;
mod price_history;
mod prices;
mod query;
mod repo;
mod reports;
mod rewrite;
mod session;
mod source;
mod storage;
mod suggest;
mod sync;
mod tools;

#[cfg(test)]
mod testing;

#[cfg(all(debug_assertions, not(mobile)))]
use specta_typescript::Typescript;
use tauri_specta::{Builder, collect_commands};

use commands::*;
use session::AppState;

/// The runtime this app runs on: the Qt runtime (Qt windowing + QtWebEngine)
/// on Linux, wry everywhere else — or on Linux too when built with
/// `--features wry`.
#[cfg(all(target_os = "linux", not(feature = "wry")))]
pub type AppRuntime = tauri_runtime_qt::QtRuntime;
#[cfg(any(not(target_os = "linux"), feature = "wry"))]
pub type AppRuntime = tauri::Wry;

/// The one command registry: what `run()` installs and what `bindings.ts` is
/// exported from.
///
/// One list, not one in `run()` and another in the export test — a command added
/// to a copy that the other one lacks is a method the frontend can call and the
/// backend does not know, which is invisible until something tries it.
pub fn command_builder() -> Builder<AppRuntime> {
    Builder::<AppRuntime>::new().commands(collect_commands![
        accounts,
        account_transactions,
        load_ledger,
        ledger_summary,
        list_folders,
        pick_ledger_folder,
        lsp_start,
        lsp_send,
        lsp_file_path,
        update_transaction,
        transaction_source,
        update_transaction_source,
        transaction_file,
        update_file_source,
        journal_files,
        file_transactions,
        file_text,
        update_file_text,
        suggest_transaction_file,
        add_transaction,
        budget,
        budget_range,
        budget_settings,
        budget_init,
        budget_fund_set,
        budget_rollover_set,
        budget_config_set,
        budget_plugin_status,
        budget_plugin_enable,
        pick_plugin_file,
        budget_plugin_install,
        dashboards,
        save_dashboard,
        delete_dashboard,
        git_state,
        git_init,
        git_set_identity,
        price_sources,
        price_targets,
        price_history,
        fetch_prices,
        write_prices,
        ai_settings,
        ai_configure,
        ai_models,
        ai_tools,
        ai_chat,
        query,
        ai_stream,
        quick_add_suggestions,
        storage_kind,
        storage_error,
        sync_ledger_tree,
        reload_ledger_tree,
        resolve_tree_conflicts,
        index_provider_list,
        index_series,
        index_import,
        index_refresh,
        index_write_prices,
        lifestyle_report,
        conversations,
        conversation,
        conversation_append,
        conversation_rename,
        conversation_delete,
        history,
        history_undo,
        sync_changes,
        sync_conflicts,
        conflict_compare,
        conflict_resolve,
        report_net_worth,
        report_cashflow,
        report_categories,
        attachment_describe,
        attachment_pdf_pages,
        attachment_table,
        attachment_text,
        tool_schemas,
        tool_invoke,
        git_branches,
        git_create_branch,
        git_checkout,
        git_log,
        git_commit_diff,
        git_stage,
        git_unstage,
        git_discard,
        git_merge,
        git_conflict_texts,
        git_resolve,
        git_merge_continue,
        git_merge_abort,
        git_revert_commit,
        ledger_diff,
        commit_ledger,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = command_builder();

    // Only export bindings on non-release desktop builds: a phone's debug build
    // must not try to write `../src/bindings.ts` (and panic when it cannot).
    #[cfg(all(debug_assertions, not(mobile)))]
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    let builder = tauri::Builder::<AppRuntime>::default()
        .manage(AppState::new())
        // and finally tell Tauri how to invoke them
        .invoke_handler(builder.invoke_handler());

    // The desktop's own choosers (`pick_ledger_folder`): Tauri's dialog plugin
    // on Windows/macOS, and on Linux the same plugin through the
    // xdg-desktop-portal backend (see Cargo.toml for why not GTK).
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    let builder = builder.plugin(tauri_plugin_dialog::init());

    // Android has no folder dialog: a directory is a SAF tree behind a content
    // URI, and this plugin is the JNI bridge to it (`storage` mirrors the tree
    // into app-private files, which is what the loader and git can open).
    #[cfg(target_os = "android")]
    let builder = builder
        .plugin(tauri_plugin_android_fs::init())
        // A phone has no folder path to open at launch: the tree the user picked
        // last is mirrored back and loaded before the window appears.
        .setup(|app| {
            commands::restore_tree(app.handle());
            Ok(())
        });

    // QtWebEngine never hands over custom-protocol POST bodies, so the Qt
    // runtime carries the invoke payload in a header instead. Without this,
    // every `invoke()` from the frontend silently fails to reach Rust.
    #[cfg(all(target_os = "linux", not(feature = "wry")))]
    let builder = builder.invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
