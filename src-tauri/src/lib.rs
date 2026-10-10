// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use specta_typescript::Typescript;
use tauri_specta::{Builder, collect_commands};

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
#[specta::specta]
fn greet(name: String) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Fully-qualified Beancount account names, e.g. "Expenses:Food:Groceries".
// TODO(alper): replace this sample list with the accounts collected from the
// loaded directives; keep the `Vec<String>` contract (sidebar derives the tree).
#[tauri::command]
#[specta::specta]
fn accounts() -> Vec<String> {
    vec![
        "Assets:Bank:Checking".into(),
        "Assets:Bank:Savings".into(),
        "Assets:Cash".into(),
        "Expenses:Food:Groceries".into(),
        "Expenses:Food:Restaurants".into(),
        "Expenses:Home:Rent".into(),
        "Income:Salary".into(),
        "Liabilities:CreditCard".into(),
    ]
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<tauri::Wry>::new()
        // Then register them (separated by a comma)
        .commands(collect_commands![greet, accounts,]);

    // Only export bindings on non-release builds
    #[cfg(debug_assertions)]
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // and finally tell Tauri how to invoke them
        .invoke_handler(builder.invoke_handler())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings() {
        Builder::<tauri::Wry>::new()
            .commands(collect_commands![greet, accounts,])
            .export(Typescript::default(), "../src/bindings.ts")
            .unwrap();
    }
}
