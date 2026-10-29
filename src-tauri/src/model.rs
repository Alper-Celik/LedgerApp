// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shapes the frontend receives and sends: account tree rows, register
//! rows, journal files, budget months, and the fields the editors may change.

/// How a file, an entry or an account stands against the ledger's last commit —
/// the one letter a git client would put beside it.
///
/// The reference is HEAD's blob where git has one, and the text the session
/// started with otherwise (see [`Loaded::baseline`](crate::session::Loaded)):
/// a ledger that is not committed yet is not a change in itself, only what was
/// written to it is. Whether that change is *staged* is not distinguished —
/// every edit this app makes stages its file on the way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// As committed, or a ledger with no commit to be measured against.
    Unchanged,
    /// New to git: the file is not in the last commit, or the entry's lines
    /// were not there before.
    Added,
    /// There and rewritten: the file is committed and differs from it, or the
    /// entry's lines stand where committed lines were.
    Modified,
}

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
/// ledger's transaction stream, which `update_transaction` re-derives the same
/// way.
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

/// A transaction that is not in the ledger yet, as the new-entry form fills it
/// out: the shape of a [`TransactionEdit`] plus the file to write it into, and
/// with the amounts optional — a posting left without a figure is the leg
/// beancount interpolates.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct NewTransaction {
    /// Path of the journal file to append to, as `journal_files` reported it.
    pub file: String,
    pub date: String, // YYYY-MM-DD
    pub flag: String, // "*" or "!"
    pub payee: Option<String>,
    pub narration: String,
    pub postings: Vec<NewPosting>,
}

/// One posting of a new transaction: the account, and what it is worth.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct NewPosting {
    pub account: String,
    /// The units figure as typed. `None` (or empty) writes the account alone and
    /// leaves the amount to be interpolated at load time.
    pub units: Option<String>,
    /// The commodity. `None` (or empty) writes the ledger's first
    /// `option "operating_currency"`.
    pub currency: Option<String>,
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

/// One file of the loaded journal — the journal the session was opened with and
/// every file it pulls in with `include`. These are the files a new transaction
/// may be written into, and what the file explorer lists.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct JournalFile {
    /// The path as loaded, which is what a new entry sends back to say where to
    /// write. The frontend only ever echoes a path it was handed; it never
    /// builds one.
    pub path: String,
    /// Name to show: the path relative to the journal's own directory when the
    /// file lives under it, else the path as it stands.
    pub label: String,
    /// How many transactions the file itself writes — every entry in it, the
    /// ones the register hides included, since this is a fact about the file
    /// rather than about a view of the ledger.
    pub transactions: u32,
    /// How the file stands in git: [`Change::Modified`] when the file is in the
    /// last commit and differs from it, [`Change::Added`] when git does not
    /// have it and it has been written to. The same measure the entries inside
    /// it carry, read at file level.
    pub change: Change,
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

/// One envelope of a budget month, as the envelope plugin's own bookkeeping
/// reports it.
///
/// Every figure is an exact decimal string in [`BudgetMonth::currency`], the
/// app's convention: nothing is a float and nothing is rounded here.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetEnvelope {
    /// The envelope's label inside its group — the account's path below the
    /// group, e.g. `Bread` for `Equity:Envelope:Food:Bread` in group `Food`.
    pub name: String,
    /// The account the plugin writes this envelope's postings to, which is what
    /// names it in the ledger.
    pub full_name: String,
    /// The ledger account this envelope mirrors — `Expenses:Food` for an envelope
    /// over `Expenses:Food`.
    ///
    /// The register deliberately hides every posting inside the envelope
    /// namespace (they are the plugin's bookkeeping, not the reader's entries), so
    /// `full_name` opens an empty page; this is the account the reader's own
    /// spending actually reaches.
    pub account: String,
    /// What the month put into the envelope, net: inherited funding, one-off
    /// `adjust`/`zero`/`pad` deltas, and the rollover sweep's own correction,
    /// which takes money back out and so reads negative.
    pub budgeted: String,
    /// What the month spent out of it, the plugin's mirrored activity, reported
    /// as a positive figure.
    pub spent: String,
    /// What the envelope held at the month's end, every earlier month carried
    /// into it — the subtree total the plugin's own `balance` assertions check.
    pub balance: String,
}

/// One top-level branch of the plugin's envelope tree — a group in the budget
/// screen's sense — and its totals over its envelopes.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetGroup {
    /// The branch's own name, the first account component below the namespace.
    pub name: String,
    /// Its envelopes' figures summed, exact decimal strings like theirs.
    pub budgeted: String,
    pub spent: String,
    pub balance: String,
    /// The envelopes below it, in the ledger's tree order.
    pub envelopes: Vec<BudgetEnvelope>,
}

/// One month of the ledger's envelope budget, as the budget screen reads it.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetMonth {
    /// The month the figures belong to, `YYYY-MM`.
    pub month: String,
    /// Every month the plugin budgeted, oldest first — the screen's navigation.
    pub months: Vec<String>,
    /// The month's groups, alphabetically.
    pub groups: Vec<BudgetGroup>,
    /// `Equity:Envelope:To-Be-Budgeted` at the month's end: what the
    /// configuration has not assigned to an envelope, per the plugin's rule
    /// `To-Be-Budgeted = on-budget money − Σ categories`.
    pub to_budget: String,
    /// The commodity every figure is in: the ledger's operating currency.
    pub currency: String,
}
