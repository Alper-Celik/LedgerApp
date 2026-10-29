// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The journal's files: what each one holds, how it stands in git, and the
//! transactions one of them writes.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use rustledger_core::Directive;
use rustledger_loader::{Ledger, SourceFile};

use crate::accounts::account_transaction_views;
use crate::git::working_tree_changes;
use crate::model::{Change, JournalFile, TransactionView};
use crate::session::Loaded;
use crate::source::source_directive_index;

/// The journal's files, as the explorer and the new-entry picker show them: in
/// the loader's own order, which is the root journal first, then what it
/// includes.
pub(crate) fn journal_file_views(loaded: &Loaded) -> Vec<JournalFile> {
    let ledger = &loaded.ledger;
    let files = ledger.source_map.files();
    // The loader resolves everything a journal mentions relative to the root
    // journal's directory (its own `base_dir` rule, shared by `include` and
    // `option "documents"`), so that is the directory a label is relative to.
    let base = files.first().and_then(|file| file.path.parent());
    // Count straight off the parsed directives: each transaction already carries
    // the file it came from, so this is one pass with no ledger clone and no
    // source index. An entry the loader synthesized has no file, so a `pad`'s
    // transaction or a plugin's entries are no file's — which is also why they
    // cannot inflate a count here.
    let mut counts = vec![0u32; files.len()];
    for directive in &ledger.directives {
        // `Spanned<Directive>` carries the file; the transaction inside does not.
        if !matches!(directive.value, Directive::Transaction(_)) {
            continue;
        }
        if let Some(count) = counts.get_mut(usize::from(directive.file_id)) {
            *count = count.saturating_add(1);
        }
    }
    let changed: HashMap<u16, Vec<(u32, u32)>> = working_tree_changes(ledger, &loaded.baseline);
    files
        .iter()
        .enumerate()
        .map(|(id, file)| JournalFile {
            path: file.path.display().to_string(),
            label: label_of(base, &file.path),
            transactions: counts[id],
            change: file_change(&changed, id, &file.path),
        })
        .collect()
}

/// The name a file is listed under: relative to the journal's own directory
/// when it lives under it, else the path as it stands. A journal that is itself
/// the file at the base answers with its path rather than an empty label.
fn label_of(base: Option<&Path>, path: &Path) -> String {
    base.and_then(|base| path.strip_prefix(base).ok())
        .filter(|rest| !rest.as_os_str().is_empty())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// How one file stands: nothing, new to git, or committed and rewritten.
///
/// The shared measure in [`crate::git`] answers which lines moved but not which
/// reference they were measured against, so the HEAD lookup is repeated here —
/// a file git does not have is `Added` however it was edited.
fn file_change(changed: &HashMap<u16, Vec<(u32, u32)>>, id: usize, path: &Path) -> Change {
    let moved = u16::try_from(id)
        .ok()
        .is_some_and(|id| changed.contains_key(&id));
    if !moved {
        Change::Unchanged
    } else if committed_at_head(path) {
        Change::Modified
    } else {
        Change::Added
    }
}

/// Whether the last commit holds `path`; `false` outside a repository, when
/// HEAD has no commit yet, or when git does not track the file.
fn committed_at_head(path: &Path) -> bool {
    let Some((repo, relative)) = repository_of(path) else {
        return false;
    };
    let tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    tree.is_some_and(|tree| tree.get_path(&relative).is_ok())
}

/// A file's repository, found by walking up from it, together with the file's
/// path relative to the working directory (git2 only takes repository-relative
/// paths). `None` for a file outside any repository.
fn repository_of(path: &Path) -> Option<(git2::Repository, std::path::PathBuf)> {
    let absolute = fs::canonicalize(path).ok()?;
    let repo = git2::Repository::discover(&absolute).ok()?;
    let workdir = fs::canonicalize(repo.workdir()?).ok()?;
    let relative = absolute.strip_prefix(&workdir).ok()?.to_path_buf();
    Some((repo, relative))
}

/// Every transaction the file `file` itself writes, newest first, each carrying
/// all of its postings — the file view's transaction mode.
///
/// The rows are the register's own, ids included, so an entry opened from here
/// edits and stages exactly as it does from an account page. An entry belongs to
/// a file only through the directive it came from; synthesized entries — a
/// `pad`'s transaction, a plugin's — belong to no file.
pub(crate) fn file_transaction_views(
    ledger: &Ledger,
    baseline: &HashMap<std::path::PathBuf, Arc<str>>,
    file: &str,
) -> Result<Vec<TransactionView>, String> {
    let found = source_of_file(ledger, file)?;
    let view = ledger.balance_view();
    let sources = source_directive_index(ledger, &view);
    let mut in_file: HashSet<u32> = HashSet::new();
    for (id, directive) in view.iter().enumerate() {
        if !matches!(directive, Directive::Transaction(_)) {
            continue;
        }
        let Some(index) = sources.get(id).copied().flatten() else {
            continue;
        };
        if usize::from(ledger.directives[index].file_id) == found.id {
            in_file.insert(u32::try_from(id).unwrap_or(u32::MAX));
        }
    }
    Ok(account_transaction_views(ledger, baseline, "")
        .into_iter()
        .filter(|row| in_file.contains(&row.id))
        .collect())
}

/// The journal file the loader holds under the path `file` — the one lookup
/// every command that names a file starts with, so an unknown path fails the
/// same way whichever one asked.
pub(crate) fn source_of_file<'a>(
    ledger: &'a Ledger,
    file: &str,
) -> Result<&'a SourceFile, String> {
    ledger
        .source_map
        .get_by_path(Path::new(file))
        .ok_or_else(|| format!("`{file}` is not a file of this journal"))
}

/// The journal file a new transaction should go into, given the accounts it
/// mentions — the new-entry form's default, so the form can preselect a file
/// before the user has to think about one.
///
/// Entries live next to their neighbours: the answer is the file of the newest
/// entry that already touches one of `accounts`, on the account itself or
/// anywhere on its branch of the tree. A ledger that says nothing about them —
/// no entry yet, an account the journal has never seen — falls back to the root
/// journal, so a new entry always has somewhere to go. `None` only when no
/// ledger is loaded or the journal has no files at all.
pub(crate) fn suggested_transaction_file(ledger: &Ledger, accounts: &[String]) -> Option<String> {
    let files = ledger.source_map.files();
    let index = recent_account_file(ledger, accounts)
        .filter(|index| *index < files.len())
        .unwrap_or(0);
    Some(files.get(index)?.path.display().to_string())
}

/// The file of the newest entry that touches one of `accounts`, or `None` when
/// no entry does. `balance_view` is in date order, so the last match on the walk
/// is the newest entry; one the loader synthesized — a `pad`'s transaction, a
/// plugin's generated entries — has no file of its own and cannot answer.
fn recent_account_file(ledger: &Ledger, accounts: &[String]) -> Option<usize> {
    if accounts.is_empty() {
        return None;
    }
    let view = ledger.balance_view();
    let sources = source_directive_index(ledger, &view);
    let mut suggested = None;
    for (id, directive) in view.iter().enumerate() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        let touches = tx.postings.iter().any(|posting| {
            accounts
                .iter()
                .any(|account| same_branch(posting.value.account.as_str(), account))
        });
        if !touches {
            continue;
        }
        let Some(index) = sources.get(id).copied().flatten() else {
            continue;
        };
        suggested = Some(ledger.directives[index].file_id as usize);
    }
    suggested
}

/// Whether `name` and `account` lie on the same branch of the account tree:
/// either is the other, or one sits under the other. Which file an entry belongs
/// in is read that way round on both sides — an entry on `Expenses:Food` says
/// where a new entry on `Expenses:Food:Groceries` goes, even though the
/// register's own one-way filter would not have listed it there.
fn same_branch(name: &str, account: &str) -> bool {
    is_under(name, account) || is_under(account, name)
}

/// Whether `name` is `account` itself or an account below it.
fn is_under(name: &str, account: &str) -> bool {
    name == account
        || (name.len() > account.len()
            && name.starts_with(account)
            && name.as_bytes()[account.len()] == b':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::AppState;
    use crate::testing::*;
    use std::fs;

    /// The register id of the entry narrated `narration`.
    fn row_id(state: &AppState, narration: &str) -> u32 {
        views(state)
            .into_iter()
            .find(|row| row.narration == narration)
            .unwrap_or_else(|| panic!("no entry narrated `{narration}`"))
            .id
    }

    /// A journal split in two files, both committed: the root holds the `open`
    /// directives and no entry, the included file one entry.
    fn split_fixture(dir: &TempDir) -> (git2::Repository, std::path::PathBuf, std::path::PathBuf) {
        let root = dir.file("main.bean");
        let entries = dir.file("2026.bean");
        fs::write(
            &root,
            "\
option \"operating_currency\" \"USD\"

include \"2026.bean\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Expenses:Coffee
",
        )
        .unwrap();
        fs::write(
            &entries,
            "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        )
        .unwrap();
        let repo = repo_with_commit(dir, &["main.bean", "2026.bean"]);
        (repo, root, entries)
    }

    /// A clean journal lists both files in loader order, with each file's own
    /// transaction count and no change.
    #[test]
    fn lists_the_journal_files_and_their_entries() {
        let dir = TempDir::new("journal-files");
        let (_, root, _) = split_fixture(&dir);
        let state = loaded_state(&root);
        let listed = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            journal_file_views(guard.as_ref().unwrap())
        };
        assert_eq!(
            listed
                .iter()
                .map(|file| (file.label.as_str(), file.transactions, file.change))
                .collect::<Vec<_>>(),
            [
                ("main.bean", 0, Change::Unchanged),
                ("2026.bean", 1, Change::Unchanged)
            ]
        );
    }

    /// A committed file that was edited reads as modified; a file git does not
    /// track that was edited reads as added.
    #[test]
    fn a_files_change_says_which_reference_it_moved_against() {
        let dir = TempDir::new("journal-change");
        let (_, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);

        // An edit to the committed file is a modification of it.
        let text = fs::read_to_string(&entries).unwrap();
        let rewritten = format!(
            "{text}\n2026-01-12 * \"Coffee\"\n  Assets:Bank:Checking             -3.00 USD\n  Expenses:Coffee                    3.00 USD\n"
        );
        fs::write(&entries, &rewritten).unwrap();
        let reloaded = crate::session::load_journal(&root).unwrap();
        {
            let mut guard = state.loaded.write().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_mut().unwrap();
            loaded.ledger = reloaded;
            loaded.remember_files();
        }
        let listed = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            journal_file_views(guard.as_ref().unwrap())
        };
        assert_eq!(
            listed
                .iter()
                .map(|file| (file.label.as_str(), file.transactions, file.change))
                .collect::<Vec<_>>(),
            [
                ("main.bean", 0, Change::Unchanged),
                ("2026.bean", 2, Change::Modified)
            ]
        );

        // A file the commit does not hold is new to git the moment it is
        // written to — measured against the session's baseline, not HEAD.
        let extra = dir.file("2027.bean");
        fs::write(&extra, "2027-01-01 open Expenses:Coffee\n").unwrap();
        let state = loaded_state(&root);
        fs::write(
            &root,
            format!(
                "{}\ninclude \"2027.bean\"\n",
                fs::read_to_string(&root).unwrap()
            ),
        )
        .unwrap();
        let reloaded = crate::session::load_journal(&root).unwrap();
        {
            let mut guard = state.loaded.write().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_mut().unwrap();
            loaded.ledger = reloaded;
            loaded.remember_files();
        }
        fs::write(&extra, "2027-01-01 open Expenses:Coffee\n\n2027-01-05 * \"Espresso\"\n  Assets:Bank:Checking  -2.00 USD\n  Expenses:Coffee        2.00 USD\n").unwrap();
        let reloaded = crate::session::load_journal(&root).unwrap();
        {
            let mut guard = state.loaded.write().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_mut().unwrap();
            loaded.ledger = reloaded;
            loaded.remember_files();
        }
        let listed = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            journal_file_views(guard.as_ref().unwrap())
        };
        let extra_view = listed
            .iter()
            .find(|file| file.label == "2027.bean")
            .expect("the included file is listed");
        assert_eq!(extra_view.change, Change::Added);
        assert_eq!(extra_view.transactions, 1);
    }

    /// A file's own transactions, in the register's own row shape: the ids are
    /// the register's, so an entry opened from here edits and stages like any
    /// other. The root journal writes none, and an unknown path is refused
    /// rather than answered with an empty list.
    #[test]
    fn one_files_transactions_are_the_registers_rows() {
        let dir = TempDir::new("file-rows");
        let (_, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);
        let rows = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            file_transaction_views(
                &loaded.ledger,
                &loaded.baseline,
                &entries.display().to_string(),
            )
            .unwrap()
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].narration, "Groceries");
        assert_eq!(rows[0].postings.len(), 2);
        assert_eq!(rows[0].id, row_id(&state, "Groceries"));

        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            assert!(
                file_transaction_views(
                    &loaded.ledger,
                    &loaded.baseline,
                    &root.display().to_string()
                )
                .unwrap()
                .is_empty()
            );
            assert!(
                file_transaction_views(&loaded.ledger, &loaded.baseline, "/tmp/elsewhere.bean")
                    .unwrap_err()
                    .contains("not a file of this journal")
            );
        }
    }

    /// The default file for a new entry follows the accounts: the file of the
    /// newest entry that already touches one of them, whichever side of the tree
    /// it is on — and the root journal when nothing does.
    #[test]
    fn the_suggested_file_follows_the_accounts() {
        let dir = TempDir::new("suggest");
        let (_, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let ledger = &guard.as_ref().unwrap().ledger;
        let suggest = |accounts: &[&str]| {
            let accounts: Vec<String> = accounts.iter().map(|name| name.to_string()).collect();
            suggested_transaction_file(ledger, &accounts)
        };

        // The account itself, its child and its parent are all the same branch.
        assert_eq!(
            suggest(&["Expenses:Food"]),
            Some(entries.display().to_string())
        );
        assert_eq!(
            suggest(&["Expenses:Food:Groceries"]),
            Some(entries.display().to_string())
        );
        assert_eq!(suggest(&["Expenses"]), Some(entries.display().to_string()));
        // Nothing touches an account the ledger has never posted to, and no
        // account at all says nothing either: a new entry still has the root
        // journal to go into.
        assert_eq!(
            suggest(&["Expenses:Coffee"]),
            Some(root.display().to_string())
        );
        assert_eq!(suggest(&[]), Some(root.display().to_string()));
    }
}
