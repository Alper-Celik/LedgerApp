// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The edit pipeline: work out the file text an edit implies, write it, reload
//! the journal and refuse — rolling the file back — if the loader complains
//! about something the journal did not already say.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::str::FromStr;

use rust_decimal::Decimal;
use rustledger_loader::{ErrorSeverity, Ledger, LedgerError};

use crate::files::source_of_file;
use crate::git::stage_file;
use crate::history::Origin;
use crate::model::{NewTransaction, TransactionEdit, TransactionFile};
use crate::rewrite::{checked_date, checked_flag, quote_beancount, rewrite_transaction};
use crate::session::{AppState, Loaded, load_journal};
use crate::source::{line_column, source_of_transaction};

/// The body of [`update_transaction`](crate::commands::update_transaction), split
/// out so it can be driven without a running Tauri app.
pub(crate) fn edit_transaction(state: &AppState, edit: TransactionEdit) -> Result<(), String> {
    // Read-only pass: work out the new file text from the loaded copy.
    let (path, previous, rewritten, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        let (source, tx, file) = source_of_transaction(ledger, edit.id)?;
        let previous = file.source.to_string();
        let rewritten = rewrite_transaction(&previous, source.span, tx, &edit)?;
        (
            file.path.clone(),
            previous,
            rewritten,
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(
        state,
        &path,
        &previous,
        None,
        rewritten,
        reported,
        Origin::User,
    )
}

/// The body of [`transaction_source`](crate::commands::transaction_source), split
/// out like [`edit_transaction`]: the transaction's own lines exactly as they
/// stand in the loaded copy of its file.
pub(crate) fn transaction_source_text(state: &AppState, id: u32) -> Result<String, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let (source, _, file) = source_of_transaction(&loaded.ledger, id)?;
    file.source
        .get(source.span.start..source.span.end)
        .map(str::to_string)
        .ok_or_else(|| format!("transaction {id} is outside its file"))
}

/// The body of
/// [`update_transaction_source`](crate::commands::update_transaction_source):
/// the transaction's span has `text` spliced over it, and the result goes
/// through the same pipeline as [`edit_transaction`] — the loader, not this
/// function, is the authority on whether arbitrary text is a legal transaction.
pub(crate) fn edit_transaction_source(state: &AppState, id: u32, text: &str) -> Result<(), String> {
    let (path, previous, rewritten, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        let (source, _, file) = source_of_transaction(ledger, id)?;
        let previous = file.source.to_string();
        let (Some(head), Some(tail)) = (
            previous.get(..source.span.start),
            previous.get(source.span.end..),
        ) else {
            return Err(format!("transaction {id} is outside its file"));
        };
        let mut rewritten = String::with_capacity(previous.len() + text.len());
        rewritten.push_str(head);
        rewritten.push_str(text);
        rewritten.push_str(tail);
        (
            file.path.clone(),
            previous,
            rewritten,
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(
        state,
        &path,
        &previous,
        None,
        rewritten,
        reported,
        Origin::User,
    )
}

/// The body of
/// [`transaction_file`](crate::commands::transaction_file): the file the entry
/// came from, its whole text, and the entry's own position in it (1-based line,
/// UTF-16 column).
pub(crate) fn transaction_file_view(state: &AppState, id: u32) -> Result<TransactionFile, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let (source, _, file) = source_of_transaction(&loaded.ledger, id)?;
    let (start_line, start_column) = line_column(&file.source, source.span.start);
    let (end_line, end_column) = line_column(&file.source, source.span.end);
    Ok(TransactionFile {
        path: file.path.display().to_string(),
        text: file.source.to_string(),
        start_line,
        start_column,
        end_line,
        end_column,
    })
}

/// The body of
/// [`update_file_source`](crate::commands::update_file_source): the whole file
/// text replaces the loaded copy, and the result goes through the same pipeline
/// as [`edit_transaction_source`] — [`apply_file_edit`] reloads, reverts on a
/// new loader error, and stages what survives.
pub(crate) fn edit_file_source(state: &AppState, id: u32, text: &str) -> Result<(), String> {
    let (path, previous, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        // The entry is only how the editor knows which file it is editing; it
        // need not still exist in `text`, the file is the unit being written.
        let (_, _, file) = source_of_transaction(ledger, id)?;
        (
            file.path.clone(),
            file.source.to_string(),
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(
        state,
        &path,
        &previous,
        None,
        text.to_string(),
        reported,
        Origin::User,
    )
}

/// The body of [`update_file_text`](crate::commands::update_file_text): a journal
/// file named by path — not by an entry inside it — has its whole text replaced,
/// then takes the same reload, refusal and staging path as every other edit.
///
/// The file is looked up in the journal, so a stale picker or a hand-written
/// payload can only ever write a file this session loaded; the loader, not this
/// function, is the authority on whether the text is a legal ledger.
pub(crate) fn edit_named_file_source(
    state: &AppState,
    file: &str,
    text: &str,
    base: Option<&str>,
) -> Result<(), String> {
    let (path, previous, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let found = source_of_file(&loaded.ledger, file)?;
        (
            found.path.clone(),
            found.source.to_string(),
            error_signatures(&loaded.ledger.errors),
        )
    };

    apply_file_edit(
        state,
        &path,
        &previous,
        base,
        text.to_string(),
        reported,
        Origin::User,
    )
}

/// The body of [`add_transaction`](crate::commands::add_transaction), split out
/// like [`edit_file_source`] so it can be driven without a running Tauri app:
/// work out the file's new text from the loaded copy, then take the same
/// reload-and-stage path as every edit.
///
/// The entry names the file it goes into; one the journal does not hold is
/// refused rather than written to, so this can only touch the ledger's own
/// files. Everything the entry says is rendered as ledger tokens first: a date,
/// flag or account the lexer could split is rejected before any byte is written,
/// and a booked entry that does not balance is caught by the reload like any
/// other edit.
pub(crate) fn create_transaction(state: &AppState, entry: NewTransaction) -> Result<(), String> {
    let (path, previous, rewritten, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        let file = source_of_file(ledger, &entry.file)?;
        let previous = file.source.to_string();
        let rewritten = append_transaction(&previous, ledger, &entry)?;
        (
            file.path.clone(),
            previous,
            rewritten,
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(
        state,
        &path,
        &previous,
        None,
        rewritten,
        reported,
        Origin::User,
    )
}

/// `text` with `entry` appended to the end of the file as a new transaction.
fn append_transaction(
    text: &str,
    ledger: &Ledger,
    entry: &NewTransaction,
) -> Result<String, String> {
    let rendered = render_transaction(text, ledger, entry)?;
    Ok(append_directive(text, &rendered))
}

/// `text` with `rendered` appended to the end of the file.
///
/// Only bytes are added: one blank line separates the directive from whatever
/// the file already ends with, and not a byte of what was there is rewritten, so
/// the change the diff view shows is the directive itself.
fn append_directive(text: &str, rendered: &str) -> String {
    let mut rewritten = String::with_capacity(text.len() + rendered.len() + 2);
    rewritten.push_str(text);
    if !text.is_empty() {
        if !text.ends_with('\n') {
            rewritten.push_str("\n\n");
        } else if !text.ends_with("\n\n") {
            rewritten.push('\n');
        }
    }
    rewritten.push_str(rendered);
    rewritten
}

/// The text of `entry`, laid out the way the file it is going into lays its own
/// postings out: the header, then one line per posting, each account padded to
/// the column the file's postings put their amounts at.
///
/// Only the tokens beancount reads are written, from values that are validated
/// or quoted on the way in, so nothing typed into the form can land in the file
/// as syntax of its own.
fn render_transaction(
    text: &str,
    ledger: &Ledger,
    entry: &NewTransaction,
) -> Result<String, String> {
    let date = checked_date(&entry.date)?;
    let flag = checked_flag(&entry.flag)?;
    let payee = entry
        .payee
        .as_deref()
        .map(str::trim)
        .filter(|payee| !payee.is_empty());
    let narration = entry.narration.trim();
    let mut rendered = String::new();
    rendered.push_str(&date);
    rendered.push(' ');
    rendered.push_str(flag);
    // A payee that only repeats the narration is not written down twice — the
    // same rule the register's header rewrite follows.
    if let Some(payee) = payee.filter(|payee| *payee != narration) {
        rendered.push(' ');
        rendered.push_str(&quote_beancount(payee));
    }
    rendered.push(' ');
    rendered.push_str(&quote_beancount(narration));

    let column = amount_column(text)
        // A file with no posting to take a column from — a root journal of
        // `open`s and `include`s, say — still gets postings that line up with
        // each other: the entry is its own convention then.
        .unwrap_or_else(|| 2 + longest_account(entry) + 1);
    for (position, posting) in entry.postings.iter().enumerate() {
        let account = posting.account.trim();
        if account.is_empty() {
            return Err(format!("posting {} has no account", position + 1));
        }
        if account
            .chars()
            .any(|character| character.is_whitespace() || character == '"' || character == ';')
        {
            return Err(format!("`{account}` is not an account name"));
        }
        rendered.push('\n');
        rendered.push_str("  ");
        rendered.push_str(account);
        let figure = match posting
            .units
            .as_deref()
            .map(str::trim)
            .filter(|units| !units.is_empty())
        {
            // An amount left blank is the leg beancount interpolates: the
            // account stands alone, the way a hand-written ledger leaves it.
            None => continue,
            Some(units) => Decimal::from_str(units)
                .map_err(|e| format!("`{units}` is not an amount: {e}"))?
                .to_string(),
        };
        let currency = posting
            .currency
            .as_deref()
            .map(str::trim)
            .filter(|currency| !currency.is_empty())
            .or_else(|| {
                ledger
                    .options
                    .operating_currency
                    .first()
                    .map(String::as_str)
            })
            .ok_or("the ledger declares no `operating_currency` to write amounts in")?;
        // Pad out to the file's own amount column, never below one space, so an
        // account longer than the file's widest cannot touch its figure.
        rendered.extend(std::iter::repeat_n(
            ' ',
            column.saturating_sub(2 + account.len()).max(1),
        ));
        rendered.push_str(&figure);
        rendered.push(' ');
        rendered.push_str(currency);
    }
    rendered.push('\n');
    Ok(rendered)
}

/// The longest account `entry` posts to, in bytes — the width a file with no
/// postings of its own gets its new entry aligned to.
fn longest_account(entry: &NewTransaction) -> usize {
    entry
        .postings
        .iter()
        .map(|posting| posting.account.trim().len())
        .max()
        .unwrap_or(0)
}

/// The column the file's own postings put their amounts at: the one most of them
/// agree on, so a new entry lines up with the entries around it. `None` when the
/// file has no posting with a figure yet.
fn amount_column(text: &str) -> Option<usize> {
    let mut columns: BTreeMap<usize, usize> = BTreeMap::new();
    for line in text.lines() {
        if let Some(column) = posting_amount_column(line) {
            *columns.entry(column).or_default() += 1;
        }
    }
    // The most common column wins; a tie goes to the leftmost, so an entry never
    // drifts further right than the file already puts its own amounts.
    columns
        .into_iter()
        .max_by_key(|&(column, count)| (count, std::cmp::Reverse(column)))
        .map(|(column, _)| column)
}

/// The byte column a posting line's amount figure starts at, when the line is
/// one of the file's own postings: indented, an account name, then a figure.
///
/// A posting the file leaves without an amount has no figure to align to, and
/// neither has anything else on the line: an account name starts with a capital
/// and never ends with a colon, which is what keeps a metadata line's
/// `key: value` from reading as one.
fn posting_amount_column(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start().len();
    if indent == 0 {
        return None;
    }
    let rest = &line[indent..];
    let account_end = rest.find(char::is_whitespace)?;
    let account = &rest[..account_end];
    let named = account.chars().next()?.is_ascii_uppercase()
        && account.contains(':')
        && !account.ends_with(':')
        && account.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ':' | '-' | '_')
        });
    if !named {
        return None;
    }
    let value = &rest[account_end..];
    let gap = value.len() - value.trim_start().len();
    value
        .trim_start()
        .starts_with(|character: char| character.is_ascii_digit() || matches!(character, '-' | '+'))
        .then_some(indent + account_end + gap)
}

/// Write an edit's `rewritten` file text over the loaded copy at `path`, reload
/// the ledger from disk, and stage the file — the shared tail of every edit
/// path: field-by-field, transaction source, and whole file.
///
/// A refused reload puts the file back exactly as it was; a ledger outside a
/// git repository is still written, there is simply nothing to stage.
fn apply_file_edit(
    state: &AppState,
    path: &Path,
    previous: &str,
    base: Option<&str>,
    rewritten: String,
    reported: Vec<String>,
    origin: Origin,
) -> Result<(), String> {
    // The offsets came from the loaded copy; a file rewritten under us (an
    // editor write, a checkout) would be clobbered by spans that no longer line
    // up with it. This runs before the "nothing changed" short-circuit on
    // purpose: text equal to the *session's* copy says nothing about the disk,
    // and a caller that treats `Ok` as "the bytes are there" (a conflict
    // resolution deleting the copy it merged) would otherwise drop a version
    // that was never written.
    let previous_bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if std::str::from_utf8(&previous_bytes).ok() != Some(previous) {
        return Err(format!(
            "{} changed on disk since it was loaded; reload the ledger",
            path.display()
        ));
    }
    // The caller's own base is the stronger check: an editor's buffer can be
    // older than the *session's* copy — a reload replaced what the ledger holds
    // while the buffer kept the text it opened with — and the two would agree
    // with disk, so the comparison above cannot see it.
    if let Some(base) = base
        && base.as_bytes() != previous_bytes
    {
        return Err(format!(
            "{} changed since this draft was started; reopen it to edit the current text",
            path.display()
        ));
    }

    // Now the disk is known to hold what the session holds, so "the text is not
    // new" means the file already says exactly that.
    if rewritten == previous {
        return Ok(());
    }

    // Reload the journal this session was opened with, never the file just
    // written: `Loaded::root` explains why.
    let root = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map(|loaded| loaded.root.clone())
            .unwrap_or_else(|| path.to_path_buf())
    };

    crate::fsutil::write_atomic(path, rewritten.as_bytes(), None)?;
    let reloaded = match load_journal(&root) {
        Ok(ledger) => ledger,
        Err(e) => {
            return Err(rollback(
                path,
                &previous_bytes,
                format!("{}: {e}", root.display()),
            ));
        }
    };
    let raised = new_errors(&reported, error_signatures(&reloaded.errors));
    if !raised.is_empty() {
        return Err(rollback(path, &previous_bytes, raised.join("\n")));
    }

    // The reload is the ledger the app now holds; the baseline keeps what the
    // session started from, so this edit reads as a change.
    let mut guard = state.loaded.write().unwrap_or_else(|e| e.into_inner());
    match guard.as_mut() {
        Some(loaded) => {
            loaded.ledger = reloaded;
            loaded.root = root;
            loaded.remember_files();
        }
        None => *guard = Some(Loaded::fresh(reloaded, root)),
    }
    drop(guard);
    // The operation log is the undo (and merge-base) authority when there is no
    // git; a failure to record it must not turn a good edit into a failed one,
    // so it is reported and the edit stands.
    record_history(state, path, &previous_bytes, rewritten.as_bytes(), origin);
    stage_file(path)
}

/// Put the file back after a refused edit. A rollback that itself fails is the
/// one case where the caller must know the ledger on disk is now wrong: the
/// loader's message alone would read as "nothing was written".
fn rollback(path: &Path, previous_bytes: &[u8], refusal: String) -> String {
    match crate::fsutil::write_atomic(path, previous_bytes, None) {
        Ok(()) => refusal,
        Err(error) => format!(
            "{refusal}\n{} could not be restored ({error}); it holds the refused edit",
            path.display()
        ),
    }
}

/// Append one operation to the ledger's log (`<ledger>/.ledger/log`).
fn record_history(state: &AppState, path: &Path, before: &[u8], after: &[u8], origin: Origin) {
    let Some(directory) = ({
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .and_then(|loaded| loaded.root.parent().map(Path::to_path_buf))
    }) else {
        return;
    };
    let log = directory.join(crate::conversations::DIRECTORY).join("log");
    let change = crate::history::Change::new(path, Some(before), Some(after));
    if let Err(error) = crate::history::record(&log, origin, "edit", &[change]) {
        eprintln!("history: {error}");
    }
}

/// The ledger's *errors* in comparable form: code and message are what separate
/// an edit's mistakes from the errors the file already had.
///
/// Warnings are excluded on purpose. They are advice — a budget the plugin says
/// went negative, an unknown key it ignores — and the app's own soundness check
/// treats them as non-fatal; refusing an edit because it raised one would make
/// the source editor unable to save a ledger the loader accepts, and a warning
/// whose text carries a figure (an over-budget pool) would refuse every later
/// edit that changes that figure.
fn error_signatures(errors: &[LedgerError]) -> Vec<String> {
    errors
        .iter()
        .filter(|error| error.severity == ErrorSeverity::Error)
        .map(|error| format!("{}: {}", error.code, error.message))
        .collect()
}

/// The errors in `after` that `before` did not already report, counting repeats
/// so that fixing one error while raising another still reads as new.
fn new_errors(before: &[String], after: Vec<String>) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for error in before {
        *seen.entry(error.as_str()).or_default() += 1;
    }
    after
        .into_iter()
        .filter(|error| match seen.get_mut(error.as_str()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{commit_staged, ledger_diff_text};
    use crate::model::{NewPosting, TransactionEdit};
    use crate::testing::*;

    /// The amount column the fixtures below align their postings at.
    const FIXTURE_COLUMN: usize = 34;

    /// A posting line of the fixtures' own shape: indented account, spaces out to
    /// [`FIXTURE_COLUMN`], then the figure and its commodity.
    fn aligned(account: &str, units: &str) -> String {
        format!(
            "  {account}{}{units} USD",
            " ".repeat(FIXTURE_COLUMN.saturating_sub(2 + account.len()))
        )
    }

    /// A posting of a new entry, as the form sends it.
    fn new_posting(account: &str, units: Option<&str>) -> NewPosting {
        NewPosting {
            account: account.to_string(),
            units: units.map(str::to_string),
            currency: Some("USD".to_string()),
        }
    }

    /// A new entry for the journal file named `file`.
    fn new_entry(
        file: &str,
        date: &str,
        narration: &str,
        postings: Vec<NewPosting>,
    ) -> NewTransaction {
        NewTransaction {
            file: file.to_string(),
            date: date.to_string(),
            flag: "*".to_string(),
            payee: None,
            narration: narration.to_string(),
            postings,
        }
    }

    /// One committed journal of two `open` directives and one entry, with the
    /// postings aligned at [`FIXTURE_COLUMN`] — the file the whole-text and
    /// new-entry tests write into.
    fn single_file_fixture(dir: &TempDir) -> (git2::Repository, std::path::PathBuf, String) {
        let text = format!(
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
{}
{}
",
            aligned("Assets:Bank:Checking", "-20.00"),
            aligned("Expenses:Food", "20.00"),
        );
        let (repo, path) = committed_fixture(dir, &text);
        (repo, path, text)
    }

    /// A journal split across files — the root holds the `open` directives, an
    /// included file the entries — is editable like any other: the reload after
    /// a write has to follow the `include` (see `Loaded::root`).
    #[test]
    fn an_edit_to_an_included_file_reloads_the_whole_journal() {
        let dir = TempDir::new("included");
        let root = dir.file("main.bean");
        let entries = dir.file("2026.bean");
        fs::write(
            &root,
            "\
option \"operating_currency\" \"USD\"

include \"2026.bean\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
",
        )
        .unwrap();
        fs::write(
            &entries,
            "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food
",
        )
        .unwrap();
        repo_with_commit(&dir, &["main.bean", "2026.bean"]);

        let state = loaded_state(&root);
        let id = view_id(&state, "Groceries");
        edit_transaction(
            &state,
            edit_of(
                id,
                "2026-01-10",
                "Groceries and coffee",
                vec![posting(0, "-25.00")],
            ),
        )
        .unwrap();

        // Only the included file changed, and the root is untouched.
        let written = fs::read_to_string(&entries).unwrap();
        assert!(
            written.contains("Groceries and coffee"),
            "entries: {written}"
        );
        assert!(written.contains("-25.00"), "entries: {written}");
        assert!(
            fs::read_to_string(&root)
                .unwrap()
                .contains("include \"2026.bean\"")
        );

        // The ledger the app now holds is the whole journal, and it is clean.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            assert_eq!(loaded.root, root);
            assert!(
                loaded.ledger.errors.is_empty(),
                "errors: {:?}",
                loaded.ledger.errors
            );
        }
    }

    #[test]
    fn edits_a_transaction_in_place_and_stages_it() {
        let dir = TempDir::new("edit");
        let (repo, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-01-05 * \"Employer\" \"Salary\"
  Assets:Bank:Checking            100.00 USD
  Income:Salary                  -100.00 USD

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");

        edit_transaction(
            &state,
            edit_of(
                id,
                "2026-01-10",
                "Groceries and coffee",
                vec![posting(0, "-25.00")],
            ),
        )
        .unwrap();

        // Only the two lines the edit addressed changed: the header lost its
        // none-payee, the units figure was replaced in place, the interpolated
        // leg keeps its amount implied and every other byte is untouched.
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-01-05 * \"Employer\" \"Salary\"
  Assets:Bank:Checking            100.00 USD
  Income:Salary                  -100.00 USD

2026-01-10 * \"Groceries and coffee\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
"
        );

        // The reloaded ledger — the one the app now holds — is clean.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let errors = &guard.as_ref().unwrap().ledger.errors;
            assert!(errors.is_empty(), "errors: {errors:?}");
        }
        // The edited transaction is the only row that reports itself modified.
        assert_eq!(modified_narrations(&state), ["Groceries and coffee"]);

        // The file is staged: the index differs from HEAD, and only by it.
        let head = repo.head().unwrap().peel_to_tree().unwrap();
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(
            staged
                .deltas()
                .map(|delta| delta.new_file().path().unwrap().display().to_string())
                .collect::<Vec<_>>(),
            ["main.bean"]
        );
    }

    #[test]
    fn an_unbalancing_edit_is_refused_and_rolled_back() {
        let dir = TempDir::new("unbalanced");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");
        let before = fs::read(&path).unwrap();

        // Raising one leg without the other leaves 5.00 USD unaccounted for.
        let refused = edit_transaction(
            &state,
            edit_of(id, "2026-01-10", "Groceries", vec![posting(0, "-25.00")]),
        );
        assert!(refused.is_err(), "an unbalanced edit went through");

        // The file is byte-for-byte what it was, and the app still holds the
        // ledger it had before the attempt.
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(modified_narrations(&state), Vec::<String>::new());
        assert_eq!(view_id(&state, "Groceries"), id);
    }

    #[test]
    fn the_source_path_round_trips_and_stages_an_edit() {
        let dir = TempDir::new("source");
        let (repo, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");
        let before = fs::read(&path).unwrap();
        let head = repo.head().unwrap().peel_to_tree().unwrap();

        // The text is the transaction's own lines, from its date token through
        // the blank line that separates it from the next directive — the span
        // the loader computed, indentation and all.
        let source = transaction_source_text(&state, id).unwrap();
        assert_eq!(
            source,
            "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD

"
        );

        // Feeding the text straight back is a no-op: the same bytes, and
        // nothing new staged.
        edit_transaction_source(&state, id, &source).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            repo.index().unwrap().write_tree().unwrap(),
            head.id(),
            "the round trip staged something"
        );
        assert_eq!(transaction_source_text(&state, id).unwrap(), source);

        // An edit through the text path rewrites the same span in place; the
        // rest of the file stays byte-identical.
        edit_transaction_source(
            &state,
            id,
            "\
2026-01-10 * \"Groceries and coffee\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    25.00 USD

",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries and coffee\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    25.00 USD

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
"
        );

        // The reloaded ledger is clean, the edited transaction kept its id, and
        // it is the only row reporting itself modified.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let errors = &guard.as_ref().unwrap().ledger.errors;
            assert!(errors.is_empty(), "errors: {errors:?}");
        }
        assert_eq!(view_id(&state, "Groceries and coffee"), id);
        assert_eq!(modified_narrations(&state), ["Groceries and coffee"]);

        // The file is staged.
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(
            staged
                .deltas()
                .map(|delta| delta.new_file().path().unwrap().display().to_string())
                .collect::<Vec<_>>(),
            ["main.bean"]
        );
    }

    #[test]
    fn an_unbalancing_source_edit_is_refused_and_rolled_back() {
        let dir = TempDir::new("unbalanced-source");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");
        let before = fs::read(&path).unwrap();

        // Raising one leg without the other leaves 5.00 USD unaccounted for.
        let refused = edit_transaction_source(
            &state,
            id,
            "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    20.00 USD
",
        );
        assert!(refused.is_err(), "an unbalanced source edit went through");

        // The file is byte-for-byte what it was, and the app still holds the
        // ledger it had before the attempt.
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(modified_narrations(&state), Vec::<String>::new());
        assert_eq!(view_id(&state, "Groceries"), id);
    }

    #[test]
    fn an_unknown_id_is_refused_by_both_source_commands() {
        let dir = TempDir::new("unknown-id");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let before = fs::read(&path).unwrap();

        // One past the last entry of the directive stream the ids index.
        let missing = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            u32::try_from(guard.as_ref().unwrap().ledger.balance_view().len()).unwrap()
        };
        assert!(
            transaction_source_text(&state, missing)
                .unwrap_err()
                .contains(&format!("no transaction with id {missing}"))
        );
        assert!(edit_transaction_source(&state, missing, "whatever\n").is_err());
        // Neither attempt touched the file.
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_posting_keeps_its_price_and_cost_when_its_number_changes() {
        let dir = TempDir::new("annotations");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Cash
2026-01-01 open Assets:Stock
2026-01-01 open Assets:Bank:Checking

2026-01-13 * \"Cash\"
  Assets:Cash                      10.00 EUR @ 1.10 USD
  Assets:Bank:Checking            -11.00 USD

2026-01-14 * \"Stock\"
  Assets:Stock                     10.00 FOO {2.50 USD}
  Assets:Bank:Checking            -25.00 USD
",
        );
        let state = loaded_state(&path);

        // Both legs move together so the transaction stays balanced: the price
        // and the cost spec are what decide the counter-leg.
        edit_transaction(
            &state,
            edit_of(
                view_id(&state, "Cash"),
                "2026-01-13",
                "Cash",
                vec![posting(0, "12.00"), posting(1, "-13.20")],
            ),
        )
        .unwrap();
        edit_transaction(
            &state,
            edit_of(
                view_id(&state, "Stock"),
                "2026-01-14",
                "Stock",
                vec![posting(0, "12.00"), posting(1, "-30.00")],
            ),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Cash
2026-01-01 open Assets:Stock
2026-01-01 open Assets:Bank:Checking

2026-01-13 * \"Cash\"
  Assets:Cash                      12.00 EUR @ 1.10 USD
  Assets:Bank:Checking            -13.20 USD

2026-01-14 * \"Stock\"
  Assets:Stock                     12.00 FOO {2.50 USD}
  Assets:Bank:Checking            -30.00 USD
"
        );
    }

    /// A header edit touches only the token the edit changed. The line keeps its
    /// indent, its separator widths, its single-literal `payee|narration` form
    /// and its trailing comment; only the narration literal's content moves.
    #[test]
    fn a_header_edit_keeps_the_files_own_formatting() {
        let dir = TempDir::new("header-format");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = views(&state)[0].id;

        // Only the narration changes; the edit goes through the same
        // `edit_transaction` the `update_transaction` command calls.
        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Corner Shop".to_string()),
                ..edit_of(id, "2026-01-10", "Groceries and coffee", Vec::new())
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries and coffee\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
"
        );
    }

    /// A posting-only edit changes nothing about the header, so the header line
    /// comes back exactly as the file spelled it — separators, form and comment.
    #[test]
    fn a_posting_edit_leaves_the_header_byte_identical() {
        let dir = TempDir::new("header-untouched");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = views(&state)[0].id;

        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Corner Shop".to_string()),
                ..edit_of(
                    id,
                    "2026-01-10",
                    "Groceries",
                    vec![posting(0, "-25.00"), posting(1, "25.00")],
                )
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries\"  ; weekly
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    25.00 USD
"
        );
    }

    /// When the edit genuinely forces a form change — a distinct payee where the
    /// source spelled a single literal with no `|` — only the description's
    /// tokens are rewritten: the indent and the trailing comment stay put.
    #[test]
    fn a_header_form_change_keeps_the_indent_and_comment() {
        let dir = TempDir::new("header-form");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = views(&state)[0].id;

        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Corner Shop".to_string()),
                ..edit_of(id, "2026-01-10", "Groceries", Vec::new())
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop\" \"Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
"
        );

        // The rewritten header still describes what the edit asked for: the
        // payee is now explicit and the narration is unchanged.
        let view = views(&state).into_iter().next().unwrap();
        assert_eq!(view.payee.as_deref(), Some("Corner Shop"));
        assert_eq!(view.narration, "Groceries");
    }

    /// A units edit replaces only the figure's bytes: the run of spaces that
    /// aligns the amount under the account survives, as does everything else.
    #[test]
    fn a_units_edit_keeps_column_alignment() {
        let dir = TempDir::new("column-aligned");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking                    -20.00 USD
  Expenses:Food                             20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");

        edit_transaction(
            &state,
            edit_of(
                id,
                "2026-01-10",
                "Groceries",
                vec![posting(0, "-21.50"), posting(1, "21.50")],
            ),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking                    -21.50 USD
  Expenses:Food                             21.50 USD
"
        );
    }

    #[test]
    fn a_ledger_outside_git_is_still_editable() {
        let dir = TempDir::new("nogit");
        let path = dir.file("main.bean");
        fs::write(
            &path,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food
",
        )
        .unwrap();
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");

        // A payee that only repeats the narration is not written down twice.
        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Groceries".to_string()),
                ..edit_of(id, "2026-01-10", "Groceries", vec![posting(0, "-25.00")])
            },
        )
        .unwrap();
        // A quote in the narration is escaped, not left to break the header.
        edit_transaction(
            &state,
            edit_of(id, "2026-01-10", "Groceries \"to go\"", Vec::new()),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries \\\"to go\\\"\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food
"
        );
        // The escaped narration reads back as the value that was written, and
        // the transaction kept its id.
        assert_eq!(view_id(&state, "Groceries \"to go\""), id);

        // With no repository there is nothing to compare against and nothing to
        // stage — and nothing fails on that account either.
        assert!(modified_narrations(&state).is_empty());
        assert!(ledger_diff_text(&state).is_err());
        assert!(commit_staged(&state, "no repository").is_err());
    }

    /// A new *warning* is not a refused edit.
    ///
    /// The plugin's budget advisories and its unknown-key notice are warnings,
    /// and one of them embeds a figure (an over-budget pool): treating them as
    /// errors would refuse the edit that raises one, and then refuse every later
    /// edit that changes the figure the message carries.
    #[test]
    fn a_new_warning_does_not_refuse_an_edit() {
        let dir = TempDir::new("warning-is-not-an-error");
        let path = dir.file("main.bean");
        fs::write(
            &path,
            "option \"operating_currency\" \"USD\"\n\n2026-01-01 open Assets:Bank:Checking\n2026-01-01 open Expenses:Food\n",
        )
        .unwrap();
        let state = loaded_state(&path);

        let with_warning = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-02 * \"Coffee\"
  Assets:Bank:Checking  -3.00 USD
  Expenses:Food          3.00 USD

2026-01-03 custom \"unknown-plugin\" \"a key no plugin reads\"
";
        edit_named_file_source(
            &state,
            &path.display().to_string(),
            with_warning,
            Some(&fs::read_to_string(&path).unwrap()),
        )
        .expect("a warning must not refuse the write");

        assert_eq!(fs::read_to_string(&path).unwrap(), with_warning);
    }

    /// Writing exactly what the session holds still checks the disk: a caller
    /// that reads success as "those bytes are on disk" (a conflict resolution
    /// deleting the copy it merged) must not be told so about a stale session.
    #[test]
    fn a_no_op_write_still_refuses_a_stale_session() {
        let dir = TempDir::new("no-op-stale");
        let (_repo, path, text) = single_file_fixture(&dir);
        let state = loaded_state(&path);
        let name = path.display().to_string();

        // The peer's version lands; the session is not told.
        let theirs = format!("{text}\n; written elsewhere \n");
        fs::write(&path, &theirs).unwrap();

        // The text the session holds is "already there" only in its own view.
        let error = edit_named_file_source(&state, &name, &text, None).unwrap_err();

        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), theirs);
    }

    /// A draft written against text the file no longer holds is refused, and the
    /// file keeps the bytes that arrived in between.
    ///
    /// The session copy already matches disk after a reload, so the check that
    /// compares them cannot see an editor whose buffer was not reloaded; the base
    /// the editor sends is what does.
    #[test]
    fn a_draft_written_against_a_stale_copy_is_refused() {
        let dir = TempDir::new("stale-base");
        let (_repo, path, text) = single_file_fixture(&dir);
        let state = loaded_state(&path);
        let name = path.display().to_string();

        // Someone else's version lands and the ledger reloads around it.
        let theirs = format!("{text}\n; written elsewhere \n");
        fs::write(&path, &theirs).unwrap();
        {
            let mut guard = state.loaded.write().unwrap_or_else(|e| e.into_inner());
            *guard = Some(crate::session::load_baselined(&path).unwrap());
        }

        let mine = format!("{text}\n; my draft \n");
        let error = edit_named_file_source(&state, &name, &mine, Some(&text)).unwrap_err();

        assert!(error.contains("changed since this draft"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), theirs);
        // The base the file actually holds goes through.
        edit_named_file_source(&state, &name, &mine, Some(&theirs)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), mine);
    }

    /// A file's whole text is what the text editor saves: the same text is no
    /// edit at all, a changed one goes through the loader and is staged, and a
    /// text the loader refuses leaves the file byte-for-byte as it stood.
    #[test]
    fn a_files_whole_text_goes_through_the_loader_and_is_staged() {
        let dir = TempDir::new("file-text");
        let (repo, path, text) = single_file_fixture(&dir);
        let state = loaded_state(&path);
        let name = path.display().to_string();
        let head = repo.head().unwrap().peel_to_tree().unwrap();

        // What the text mode is handed is the copy this session loaded.
        let handed = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            source_of_file(&guard.as_ref().unwrap().ledger, &name)
                .unwrap()
                .source
                .to_string()
        };
        assert_eq!(handed, text);

        // The same text back is no edit at all: no bytes written, nothing staged.
        let before = fs::read(&path).unwrap();
        edit_named_file_source(&state, &name, &handed, None).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(repo.index().unwrap().write_tree().unwrap(), head.id());

        // A file written with one more entry is staged, and the app holds the
        // ledger it now describes.
        let rewritten = format!(
            "{text}\n2026-01-20 * \"Coffee\"\n{}\n{}\n",
            aligned("Assets:Bank:Checking", "-3.00"),
            aligned("Expenses:Food", "3.00"),
        );
        edit_named_file_source(&state, &name, &rewritten, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), rewritten);
        assert_eq!(
            views(&state)
                .into_iter()
                .filter(|view| view.narration == "Coffee")
                .count(),
            1
        );
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(staged.deltas().count(), 1);

        // Text the loader refuses — legs that do not balance — leaves the file
        // exactly as it was, and a path the journal does not hold is refused
        // without reading anything.
        let before = fs::read(&path).unwrap();
        let refused = format!(
            "{rewritten}2026-01-21 * \"Broken\"\n  Assets:Bank:Checking  -3.00 USD\n  Expenses:Food          2.00 USD\n"
        );
        assert!(edit_named_file_source(&state, &name, &refused, None).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(
            edit_named_file_source(&state, "/tmp/elsewhere.bean", "whatever\n", None)
                .unwrap_err()
                .contains("not a file of this journal")
        );
    }

    /// A new entry lands in the file it names, at the end, laid out like that
    /// file's own postings — through the same reload, refusal and staging path as
    /// every other edit, with the leg left without a figure booked on reload.
    #[test]
    fn a_new_entry_is_appended_to_the_named_file_and_booked() {
        let dir = TempDir::new("add");
        let (repo, path, text) = single_file_fixture(&dir);
        let state = loaded_state(&path);
        let name = path.display().to_string();
        let head = repo.head().unwrap().peel_to_tree().unwrap();

        create_transaction(
            &state,
            NewTransaction {
                payee: Some("Corner Shop".to_string()),
                ..new_entry(
                    &name,
                    "2026-01-14",
                    "Coffee",
                    vec![
                        new_posting("Assets:Bank:Checking", Some("-16.20")),
                        new_posting("Expenses:Food", None),
                    ],
                )
            },
        )
        .unwrap();

        // Appended at the end, aligned like the file's own amounts, with the
        // implied leg written as the account alone.
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{text}\n2026-01-14 * \"Corner Shop\" \"Coffee\"\n{}\n  Expenses:Food\n",
                aligned("Assets:Bank:Checking", "-16.20"),
            )
        );
        let coffee = views(&state)
            .into_iter()
            .find(|view| view.narration == "Coffee")
            .expect("the new entry is in the register");
        assert_eq!(
            coffee
                .postings
                .iter()
                .map(|posting| (posting.account.as_str(), posting.units.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Assets:Bank:Checking", "-16.20"),
                ("Expenses:Food", "16.20"),
            ]
        );
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(staged.deltas().count(), 1);

        // An entry the loader refuses, or a file the journal does not hold, is
        // not written: the file keeps every byte it had.
        let before = fs::read(&path).unwrap();
        assert!(
            create_transaction(
                &state,
                new_entry(
                    &name,
                    "2026-01-15",
                    "Unbalanced",
                    vec![
                        new_posting("Assets:Bank:Checking", Some("-3.50")),
                        new_posting("Expenses:Food", Some("3.00")),
                    ],
                ),
            )
            .is_err(),
            "an unbalanced entry went through"
        );
        assert!(
            create_transaction(
                &state,
                new_entry(
                    "/tmp/elsewhere.bean",
                    "2026-01-15",
                    "Elsewhere",
                    vec![new_posting("Expenses:Food", Some("-3.00"))],
                ),
            )
            .unwrap_err()
            .contains("not a file of this journal")
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
