// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ledger app backend: the Tauri command surface (`commands`), the loaded
//! ledger session (`session`), the ledger-derived views (`accounts`, `source`),
//! the file edits behind the editors (`editing`, `rewrite`), the in-app folder
//! picker (`folders`), the git integration (`git`), and the shapes the frontend
//! sends and receives (`model`).

mod accounts;
pub mod commands;
mod editing;
mod folders;
mod git;
mod model;
mod rewrite;
mod session;
mod source;

#[cfg(test)]
mod testing;

use specta_typescript::Typescript;
use tauri_specta::{Builder, collect_commands};

use commands::{
    account_transactions, accounts, commit_ledger, greet, ledger_diff, ledger_summary,
    list_folders, load_ledger, transaction_file, transaction_source, update_file_source,
    update_transaction, update_transaction_source,
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
        update_transaction,
        transaction_source,
        update_transaction_source,
        transaction_file,
        update_file_source,
        ledger_diff,
        commit_ledger,
    ]);

    // Only export bindings on non-release builds
    #[cfg(debug_assertions)]
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    let builder = tauri::Builder::<AppRuntime>::default()
        .manage(AppState::new())
        // and finally tell Tauri how to invoke them
        .invoke_handler(builder.invoke_handler());

    // QtWebEngine never hands over custom-protocol POST bodies, so the Qt
    // runtime carries the invoke payload in a header instead. Without this,
    // every `invoke()` from the frontend silently fails to reach Rust.
    #[cfg(all(target_os = "linux", not(feature = "wry")))]
    let builder = builder.invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
