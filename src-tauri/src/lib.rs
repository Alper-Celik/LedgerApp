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

/// Fully-qualified Beancount account name, e.g. "Expenses:Food:Groceries".
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct AccountBalance {
    pub name: String,
    /// Balance of the account in its primary commodity.
    /// TODO(alper): compute from posted transactions; only leaf balances are
    /// returned here, parent totals are summed on the frontend.
    pub balance: f64,
}

/// Leaf account balances. TODO(alper): replace this sample list with balances
/// collected from the loaded directives; keep the `Vec<AccountBalance>`
/// contract (sidebar derives the tree and parent sums).
#[tauri::command]
#[specta::specta]
fn accounts() -> Vec<AccountBalance> {
    vec![
        AccountBalance { name: "Assets:Bank:Checking".into(), balance: 1500.0 },
        AccountBalance { name: "Assets:Bank:Savings".into(), balance: 10000.0 },
        AccountBalance { name: "Assets:Cash".into(), balance: 120.5 },
        AccountBalance { name: "Expenses:Food:Groceries".into(), balance: 345.2 },
        AccountBalance { name: "Expenses:Food:Restaurants".into(), balance: 89.9 },
        AccountBalance { name: "Expenses:Home:Rent".into(), balance: 950.0 },
        AccountBalance { name: "Income:Salary".into(), balance: -3000.0 },
        AccountBalance { name: "Liabilities:CreditCard".into(), balance: -520.75 },
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
