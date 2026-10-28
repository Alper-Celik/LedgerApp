// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The edit pipeline: work out the file text an edit implies, write it, reload
//! the journal and refuse — rolling the file back — if the loader complains
//! about something the journal did not already say.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rustledger_loader::LedgerError;

use crate::git::stage_file;
use crate::model::{TransactionEdit, TransactionFile};
use crate::rewrite::rewrite_transaction;
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

    apply_file_edit(state, &path, &previous, rewritten, reported)
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

    apply_file_edit(state, &path, &previous, rewritten, reported)
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

    apply_file_edit(state, &path, &previous, text.to_string(), reported)
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
    rewritten: String,
    reported: Vec<String>,
) -> Result<(), String> {
    if rewritten == previous {
        // The edit describes the file exactly as it already stands.
        return Ok(());
    }

    // The offsets came from the loaded copy; a file rewritten under us (an
    // editor write, a checkout) would be clobbered by spans that no longer line
    // up with it.
    let previous_bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if std::str::from_utf8(&previous_bytes).ok() != Some(previous) {
        return Err(format!(
            "{} changed on disk since it was loaded; reload the ledger",
            path.display()
        ));
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

    fs::write(path, rewritten.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    let reloaded = match load_journal(&root) {
        Ok(ledger) => ledger,
        Err(e) => {
            let _ = fs::write(path, &previous_bytes);
            return Err(format!("{}: {e}", root.display()));
        }
    };
    let raised = new_errors(&reported, error_signatures(&reloaded.errors));
    if !raised.is_empty() {
        let _ = fs::write(path, &previous_bytes);
        return Err(raised.join("\n"));
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
    stage_file(path)
}

/// The ledger's errors in comparable form: code and message are what separate an
/// edit's mistakes from the errors the file already had.
fn error_signatures(errors: &[LedgerError]) -> Vec<String> {
    errors
        .iter()
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
    use crate::model::TransactionEdit;
    use crate::testing::*;

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
}
