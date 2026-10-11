// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use specta_typescript::Typescript;
use tauri_specta::{Builder, collect_commands};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use rustledger_core::{Decimal, Directive, IncompleteAmount};
use rustledger_loader::{Ledger, LoadOptions};
use tauri::State;

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
    /// Exact balance in the account's primary commodity (the first
    /// `option "operating_currency"` it holds, else the commodity with the
    /// largest absolute position), as a decimal string. Parents carry their
    /// own postings' balance only; the frontend never sums.
    pub balance: String,
}

/// Shared app state: the currently loaded ledger, if any.
#[derive(Default)]
struct AppState {
    ledger: RwLock<Option<Ledger>>,
}

impl AppState {
    /// Open the ledger at startup: the last CLI argument if one was given,
    /// else `LEDGER_PATH` from the environment (set per-machine via a
    /// git-ignored `.envrc.local`), else `main.bean` in the working
    /// directory. A missing or broken file just leaves the app running with
    /// no ledger loaded.
    fn new() -> Self {
        let path = resolve_ledger_path();
        let ledger = match rustledger_loader::load(Path::new(&path), &LoadOptions::default()) {
            Ok(ledger) => Some(ledger),
            Err(e) => {
                eprintln!("not loading {}: {e}", path.display());
                None
            }
        };
        Self {
            ledger: RwLock::new(ledger),
        }
    }
}

/// Resolve the ledger to open: last CLI argument, else `LEDGER_PATH`, else
/// `main.bean`. A directory is resolved to the root journal inside it.
fn resolve_ledger_path() -> PathBuf {
    let raw = std::env::args()
        .skip(1) // drop the program name
        .last()
        .or_else(|| std::env::var("LEDGER_PATH").ok())
        .unwrap_or_else(|| "main.bean".to_string());
    let path = PathBuf::from(raw);
    if path.is_dir() {
        rustledger_loader::discover_journal_file(&path).unwrap_or(path)
    } else {
        path
    }
}

/// Summary of a successful ledger load.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct LoadSummary {
    /// Path of the loaded root journal.
    pub path: String,
    /// Number of loaded directives (after booking and plugins).
    pub directive_count: u32,
    /// Number of validation/processing errors collected; the ledger is still
    /// loaded and usable.
    pub error_count: u32,
}

/// Parse, book, and validate a beancount file, then keep it as app state.
/// A failed load leaves the previously loaded ledger untouched.
#[tauri::command]
#[specta::specta]
fn load_ledger(path: String, state: State<'_, AppState>) -> Result<LoadSummary, String> {
    let ledger = rustledger_loader::load(Path::new(&path), &LoadOptions::default())
        .map_err(|e| e.to_string())?;
    let summary = LoadSummary {
        path,
        directive_count: u32::try_from(ledger.directives.len()).unwrap_or(u32::MAX),
        error_count: u32::try_from(ledger.errors.len()).unwrap_or(u32::MAX),
    };
    // A poisoned lock only means a previous command panicked mid-write; the
    // value is still a valid `Option<Ledger>`, so recover it rather than
    // failing every later command.
    *state.ledger.write().unwrap_or_else(|e| e.into_inner()) = Some(ledger);
    Ok(summary)
}

/// One entry per account that has postings; parents are included and carry
/// only their own postings' balance.
#[tauri::command]
#[specta::specta]
fn accounts(state: State<'_, AppState>) -> Vec<AccountBalance> {
    let guard = state.ledger.read().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(account_balances).unwrap_or_default()
}

/// Accumulate per-(account, currency) units over the pad-merged transaction
/// stream; one entry per account with postings.
fn account_balances(ledger: &Ledger) -> Vec<AccountBalance> {
    let mut totals: HashMap<String, HashMap<String, Decimal>> = HashMap::new();
    for directive in ledger.balance_view() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        for posting in &tx.postings {
            let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                // Residual/interpolated postings never occur post-booking.
                continue;
            };
            *totals
                .entry(posting.value.account.as_str().to_owned())
                .or_default()
                .entry(amount.currency.as_str().to_owned())
                .or_default() += amount.number;
        }
    }
    let mut balances: Vec<AccountBalance> = totals
        .iter()
        .map(|(name, held)| AccountBalance {
            name: name.clone(),
            balance: primary_units(held, &ledger.options.operating_currency).to_string(),
        })
        .collect();
    balances.sort_by(|a, b| a.name.cmp(&b.name));
    balances
}

/// Units of an account's primary commodity: the first operating currency the
/// account holds, else the commodity with the largest absolute position.
fn primary_units(held: &HashMap<String, Decimal>, operating: &[String]) -> Decimal {
    for currency in operating {
        if let Some(units) = held.get(currency) {
            return *units;
        }
    }
    held.values()
        .max_by_key(|units| units.abs())
        .copied()
        .unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<tauri::Wry>::new()
        // Then register them (separated by a comma)
        .commands(collect_commands![greet, accounts, load_ledger,]);

    // Only export bindings on non-release builds
    #[cfg(debug_assertions)]
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .manage(AppState::new())
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
            .commands(collect_commands![greet, accounts, load_ledger,])
            .export(Typescript::default(), "../src/bindings.ts")
            .unwrap();
    }

    #[test]
    fn computes_account_balances_with_pads_and_multi_currency() {
        let mut fs = rustledger_loader::VirtualFileSystem::new();
        fs.add_file(
            "main.beancount",
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Assets:Bank:Emergency
2026-01-01 open Assets:Cash
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary
2026-01-01 open Equity:Opening-Balances

2026-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
2026-01-02 balance Assets:Bank:Checking 100.00 USD

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD

2026-01-11 * \"Salary\"
  Assets:Bank:Checking            200.00 USD
  Income:Salary                  -200.00 USD

2026-01-12 * \"Cash\"
  Assets:Cash                      10.00 EUR @ 1.10 USD
  Assets:Bank:Checking            -11.00 USD

2026-01-13 * \"Transfer\"
  Assets:Bank                     -30.00 USD
  Assets:Bank:Emergency            30.00 USD
",
        );
        let ledger = rustledger_loader::load_with_fs(
            Path::new("main.beancount"),
            &LoadOptions::default(),
            Box::new(fs),
        )
        .unwrap();
        assert!(ledger.errors.is_empty(), "errors: {:?}", ledger.errors);

        let balances = account_balances(&ledger);
        let get = |name: &str| {
            balances
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.balance.as_str())
                .unwrap_or_else(|| panic!("missing {name} in {balances:?}"))
        };

        // Checking: 100 (pad) - 50 + 200 - 11 (bought EUR cash).
        assert_eq!(get("Assets:Bank:Checking"), "239.00");
        assert_eq!(get("Assets:Bank:Emergency"), "30.00");
        // Assets:Bank has postings AND a sub-account: included with its own
        // postings' balance, never summed with children.
        assert_eq!(get("Assets:Bank"), "-30.00");
        // USD held + USD is an operating currency → direct match.
        assert_eq!(get("Expenses:Food"), "50.00");
        assert_eq!(get("Income:Salary"), "-200.00");
        // Fallback: EUR only, not an operating currency → largest |units|.
        assert_eq!(get("Assets:Cash"), "10.00");
        assert_eq!(get("Equity:Opening-Balances"), "-100.00");
        // Exact scale is preserved end-to-end.
        assert!(balances.iter().all(|b| b.balance.contains('.')));
    }
}
