// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ledger app backend: the Tauri command surface (`commands`), the loaded
//! ledger session (`session`), the ledger-derived views (`accounts`, `source`,
//! `files`, `budget`), the file edits behind the editors (`editing`, `rewrite`),
//! the in-app folder picker (`folders`), the git integration (`git`), the
//! embedded language server (`lsp`), and the shapes the frontend sends and
//! receives (`model`).

mod accounts;
mod ai;
mod ai_cmd;
pub mod attachments;
mod budget;
pub mod commands;
mod conflict;
mod conflict_cmd;
mod conflict_names;
mod conversations;
mod dashboards;
mod editing;
mod files;
mod folders;
mod git;
mod git_ops;
mod history;
mod index;
mod index_providers;
mod index_store;
mod lsp;
mod model;
mod prices;
mod repo;
mod reports;
mod rewrite;
mod session;
mod source;
mod storage;
mod tools;

#[cfg(test)]
mod testing;

use specta_typescript::Typescript;
use tauri_specta::{Builder, collect_commands};

use commands::{
    account_transactions, accounts, add_transaction, budget, commit_ledger, file_text,
    file_transactions, greet, journal_files, ledger_diff, ledger_summary, list_folders,
    load_ledger, lsp_file_path, lsp_send, lsp_start, pick_ledger_folder, suggest_transaction_file,
    transaction_file, transaction_source, update_file_source, update_file_text,
    update_transaction, update_transaction_source,
};
use commands::{
    ai_chat, ai_configure, ai_models, ai_settings, ai_tools, budget_range, conflict_compare,
    conflict_resolve, conversation, conversation_append, conversation_delete, conversation_rename,
    conversations, dashboards, delete_dashboard, fetch_prices, git_init, git_set_identity,
    git_state, history, history_undo, index_import, index_provider_list, index_refresh,
    index_series,
    lifestyle_report, price_sources, price_targets, save_dashboard, sync_conflicts, write_prices,
};
use session::AppState;

/// The runtime this app runs on: the Qt runtime (Qt windowing + QtWebEngine)
/// on Linux, wry everywhere else — or on Linux too when built with
/// `--features wry`.
#[cfg(all(target_os = "linux", not(feature = "wry")))]
pub type AppRuntime = tauri_runtime_qt::QtRuntime;
#[cfg(any(not(target_os = "linux"), feature = "wry"))]
pub type AppRuntime = tauri::Wry;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<AppRuntime>::new().commands(collect_commands![
        greet,
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
        dashboards,
        save_dashboard,
        delete_dashboard,
        git_state,
        git_init,
        git_set_identity,
        price_sources,
        price_targets,
        fetch_prices,
        write_prices,
        ai_settings,
        ai_configure,
        ai_models,
        ai_tools,
        ai_chat,
        index_provider_list,
        index_series,
        index_import,
        index_refresh,
        lifestyle_report,
        conversations,
        conversation,
        conversation_append,
        conversation_rename,
        conversation_delete,
        history,
        history_undo,
        sync_conflicts,
        conflict_compare,
        conflict_resolve,
        ledger_diff,
        commit_ledger,
    ]);

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

    // QtWebEngine never hands over custom-protocol POST bodies, so the Qt
    // runtime carries the invoke payload in a header instead. Without this,
    // every `invoke()` from the frontend silently fails to reach Rust.
    #[cfg(all(target_os = "linux", not(feature = "wry")))]
    let builder = builder.invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
