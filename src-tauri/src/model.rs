// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shapes the frontend receives and sends: account tree rows, register
//! rows, and the fields the editors may change.

/// Fully-qualified Beancount account name, e.g. "Expenses:Food:Groceries".
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct AccountBalance {
    pub name: String,
    /// Exact subtree balance in the account's primary commodity (the first
    /// `option "operating_currency"` it holds, else the commodity with the
    /// largest absolute position), as a decimal string — beancount `bal`
    /// semantics: leaves report their own postings, parents the sum of their
    /// whole subtree. The frontend never sums.
    pub balance: String,
    /// Beancount commodity of `balance`, e.g. `USD` or `TRY` — the code the
    /// frontend needs to place a symbol or code next to the amount.
    pub currency: String,
    /// True when at least one transaction in this account's subtree is
    /// uncommitted — the same flag [`TransactionView::modified`] carries, rolled
    /// up the account tree so the sidebar can mark a branch.
    pub modified: bool,
}

/// One posting as shown on an account page.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct PostingView {
    pub account: String,
    /// Exact units as a decimal string.
    pub units: String,
    pub currency: String,
}

/// A transaction that touches an account, with every posting it contains.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct TransactionView {
    /// Position of the transaction in the ledger's transaction stream; stable id for the UI.
    pub id: u32,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Transaction flag, `*` or `!`.
    pub flag: String,
    pub payee: Option<String>,
    pub narration: String,
    pub postings: Vec<PostingView>,
    /// Whether the transaction's own lines differ from the ledger's last commit,
    /// staged or not. False everywhere when the ledger is not in a git
    /// repository or nothing changed — the register never fails over git.
    pub modified: bool,
}

/// Fields the register may change about one transaction. `id` is the
/// `TransactionView.id` the frontend already has: the transaction's index in the
/// ledger's transaction stream, which this command re-derives the same way.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct TransactionEdit {
    pub id: u32,
    pub date: String, // YYYY-MM-DD
    pub flag: String, // "*" or "!"
    pub payee: Option<String>,
    pub narration: String,
    /// Posting amount edits, by the posting's index inside the transaction.
    pub postings: Vec<PostingEdit>,
}

/// One posting's new units figure. Everything else on the posting's line — its
/// currency, `@`/`@@` price, `{cost}` spec and comment — stays as written.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct PostingEdit {
    pub index: u32,
    pub units: String,
}

/// A whole ledger file as the source editor sees it: the text the editor is
/// handed, the path it came from, and the transaction's position in it. Line and
/// column are 1-based and in Monaco's coordinates (`column` counts UTF-16 code
/// units, which is how a Monaco model measures a line), so the editor can reveal
/// the entry without re-deriving anything from byte offsets.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct TransactionFile {
    pub path: String,
    pub text: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
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

/// One folder in the in-app folder picker.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct FolderEntry {
    pub name: String,
    /// Absolute path, the argument that opens it.
    pub path: String,
    /// A root journal sits directly inside it (`main.bean` and friends), so
    /// opening this folder loads a ledger rather than failing.
    pub ledger: bool,
}

/// One directory as the folder picker lists it: its own path, the way back up,
/// and the folders inside it.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct FolderListing {
    /// Absolute path of the directory being listed.
    pub path: String,
    /// Its parent, or `None` at the filesystem root.
    pub parent: Option<String>,
    /// A root journal sits directly in *this* directory, so it can be opened as
    /// it stands.
    pub ledger: bool,
    /// The folders inside it, hidden ones left out, name-sorted.
    pub folders: Vec<FolderEntry>,
}
