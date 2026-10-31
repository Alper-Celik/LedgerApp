// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a synced folder changed behind the app's back.
//!
//! A ledger can live in a folder a cloud client syncs (Drive, Syncthing,
//! Nextcloud…). When another device writes a file, the folder has it before the
//! app does, and the app would keep serving the ledger it loaded. The operation
//! log already records, per device, the hash of every file this device last saw
//! ([`history::last_seen`]); comparing that with the disk is what tells "the
//! folder moved on" apart from "this is the file we wrote".
//!
//! This module only reports. Reloading is the caller's decision (the UI asks),
//! because a reload replaces the ledger under any unsaved editor draft.
//!
//! # When the folder is not the tracker
//!
//! The log is a second baseline for one filesystem tree, and a second baseline
//! is a liability unless it is the only one. A ledger in a git repository is
//! tracked by git: its commits, branches and checkouts are the record of what
//! the tree held, and they move the files without ever going through this
//! module's log. The two never agree for long — after a commit, a checkout or a
//! branch switch the log's last-seen map describes a version git has already
//! replaced, so a comparison reports the app's own repository work as "changed
//! in this folder outside the app", the false banner a reader complains about.
//! [`watches`] is the one gate the commands consult: outside a repository (and
//! outside a SAF tree, where the mirror is not the ledger the reader edits) the
//! folder is the only tracker and this module does its work; inside one git is
//! authoritative and every answer here is empty.

use std::fs;
use std::path::{Path, PathBuf};

use blake3::Hasher;
use serde::Serialize;

use crate::history;
use crate::session::Loaded;
use crate::storage::StorageKind;

/// Whether this ledger's folder is the thing that tracks its versions.
///
/// True for a ledger the app reads and writes directly in a folder something
/// else may change (a cloud client, another device). False in a git repository,
/// where git owns the tree's history — a second baseline behind git's back is
/// exactly what produced the false "changed outside the app" report — and in a
/// SAF tree, where the mirror is the app's private copy and the reconciliation
/// (`sync_ledger_tree`) is the other side.
///
/// Every folder-tracking answer the commands return goes through this one
/// predicate ([`changes_for`], [`conflicts_for`]), so the two commands cannot
/// disagree about which mode the ledger is in.
pub fn watches(ledger_dir: &Path) -> bool {
    crate::storage::kind() != StorageKind::Tree && !crate::repo::state(ledger_dir).repo
}

/// Adopt what a load just read as this device's last-seen baseline.
///
/// The watch exists to notice a folder that moved *before* the app read it.
/// Once the load has read the disk, the old baseline describes a version nobody
/// is looking at any more, and [`changed`] would keep reporting that very reload
/// as an outside change for ever — the banner does not clear itself, because
/// nothing else rewrites those files. A file a peer *deleted* is the same
/// story: the load already reflected the deletion by not reading it, so its old
/// entry is recorded gone rather than left to report a change nobody can act
/// on. The app calls this after every successful load of a ledger whose folder
/// it watches; a git-owned or tree-owned ledger leaves the log untouched
/// ([`watches`]).
pub fn adopt_loaded(loaded: &Loaded) {
    let Some(directory) = loaded.root.parent() else {
        return;
    };
    if !watches(directory) {
        return;
    }
    let mut files: Vec<PathBuf> = loaded
        .ledger
        .source_map
        .files()
        .iter()
        .map(|file| file.path.clone())
        .collect();
    let log = log_dir(directory);
    // Only this device's own record: another device's entries name paths in its
    // mirror, and adopting those would write spellings this device never used.
    // A recorded path that is still on disk but outside the load (another
    // journal in the folder) is left alone — nothing showed it to the reader.
    if let Ok(seen) = history::last_seen(&log) {
        for (recorded, _) in seen {
            let path = history::local_path(&log, &recorded).unwrap_or(recorded);
            if !path.is_file() {
                files.push(path);
            }
        }
    }
    // Adopting is a repair of the watch, never part of the load: a log that
    // cannot be written must not fail an open that already parsed.
    if let Err(error) = history::note_seen(&log, &files) {
        eprintln!("history: loaded files not noted as seen: {error}");
    }
}

/// One file whose bytes on disk are not the bytes this device last recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ExternalChange {
    /// The path as the log names it (absolute, as the ledger knows it).
    pub path: String,
    /// The hash recorded the last time this device wrote or loaded the file.
    pub seen: String,
    /// The hash on disk now.
    pub disk: String,
}

/// Every file the log knows about whose bytes moved since this device last saw
/// them, sorted by path. A file the log does not mention is not reported: the
/// app never wrote it, so nothing about it changed behind a write of ours.
pub fn changed(log_dir: &Path) -> Result<Vec<ExternalChange>, String> {
    let seen = history::last_seen(log_dir)?;
    let mut changes = Vec::new();
    for (path, recorded) in seen {
        let Some(recorded) = recorded else {
            // The operation that created it was a deletion of a file this device
            // no longer has; nothing to compare.
            continue;
        };
        // A recorded path is the writer's spelling — another device's private
        // mirror, or this device before the folder moved. Resolving it against
        // this ledger first is what keeps a moved folder from reading as "every
        // file was deleted by a peer".
        let path = history::local_path(log_dir, &path).unwrap_or(path);
        let disk = match fs::read(&path) {
            Ok(bytes) => hash(&bytes),
            // Gone since we wrote it: a peer deleted it. That is a change too,
            // and the empty hash says which kind.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if disk != recorded {
            changes.push(ExternalChange {
                path: path.display().to_string(),
                seen: recorded,
                disk,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// The answer `sync_changes` gives: the files the folder changed behind the
/// app, or nothing when this ledger is not folder-tracked ([`watches`]).
///
/// Both commands go through one function per report rather than each calling
/// [`watches`] on its own, so a git-owned ledger cannot show one kind of report
/// while hiding the other: a second baseline behind git's back is exactly what
/// produced the false "changed outside the app" banner, and an empty answer is
/// how that mode stays off.
pub fn changes_for(ledger_dir: &Path) -> Result<Vec<ExternalChange>, String> {
    if !watches(ledger_dir) {
        return Ok(Vec::new());
    }
    changed(&log_dir(ledger_dir))
}

/// The answer `sync_conflicts` gives: the sync client's conflicted copies under
/// the ledger folder, or an empty report when this ledger is not folder-tracked
/// ([`watches`]).
///
/// In a repository git owns the versions, and its merge conflicts are surfaced
/// by the merge view; a name-shaped second copy there is an ordinary neighbour
/// of something else, not a difference the app may offer to settle.
pub fn conflicts_for(ledger_dir: &Path) -> Result<crate::conflict_names::Report, String> {
    if !watches(ledger_dir) {
        return Ok(crate::conflict_names::Report::default());
    }
    crate::conflict_names::scan_report(ledger_dir)
}

/// The hash the log uses for a file's bytes: BLAKE3 hex, the same digest
/// `history` names its blobs by, so a comparison never needs the blob.
pub fn hash(bytes: &[u8]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(bytes);
    hasher.finalize().to_hex().to_string()
}

/// Where the log lives for a ledger directory: `<ledger>/.ledger/log`, the same
/// placement [`crate::conversations`] uses for its sessions.
pub fn log_dir(ledger_dir: &Path) -> PathBuf {
    ledger_dir.join(crate::conversations::DIRECTORY).join("log")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Origin, record};
    use crate::testing::TempDir;

    #[test]
    fn a_file_the_folder_changed_is_reported() {
        let dir = TempDir::new("sync-changed");
        let ledger = dir.file("main.bean");
        fs::write(&ledger, "2026-01-01 open Assets:Cash\n").unwrap();
        let log = log_dir(dir.dir());
        record(
            &log,
            Origin::User,
            "write",
            &[history::Change::new(
                &ledger,
                None,
                Some(&fs::read(&ledger).unwrap()),
            )],
        )
        .unwrap();

        // Another device rewrote it.
        fs::write(&ledger, "2026-01-02 open Assets:Cash\n").unwrap();

        let changes = changed(&log).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, ledger.display().to_string());
        assert_eq!(changes[0].disk, hash(&fs::read(&ledger).unwrap()));
    }

    #[test]
    fn our_own_write_and_a_deletion_are_told_apart() {
        let dir = TempDir::new("sync-deleted");
        let ledger = dir.file("main.bean");
        fs::write(&ledger, "2026-01-01 open Assets:Cash\n").unwrap();
        let log = log_dir(dir.dir());
        let bytes = fs::read(&ledger).unwrap();
        record(
            &log,
            Origin::User,
            "write",
            &[history::Change::new(&ledger, None, Some(&bytes))],
        )
        .unwrap();

        // Nothing moved: nothing to report.
        assert!(changed(&log).unwrap().is_empty());

        fs::remove_file(&ledger).unwrap();
        let changes = changed(&log).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(
            changes[0].disk.is_empty(),
            "a deleted file compares as empty"
        );
    }

    #[test]
    fn a_file_the_log_never_mentions_is_ignored() {
        let dir = TempDir::new("sync-unknown");
        fs::write(dir.file("other.bean"), "not ours\n").unwrap();
        let log = log_dir(dir.dir());

        assert!(changed(&log).unwrap().is_empty());
    }

    /// The ledger every test below writes and edits: a parsing, balancing
    /// journal, so the edit funnel accepts what the tests do to it.
    const LEDGER: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
";

    /// A repository is git's to track: the operation log's baseline would go
    /// stale on the first commit and report the app's own work as an outside
    /// change. A plain folder keeps the watch.
    #[test]
    fn git_owns_a_repository_but_not_a_plain_folder() {
        let repository = TempDir::new("sync-mode-repository");
        repository.seed("main.bean", LEDGER);
        crate::testing::repo_with_commit(&repository, &["main.bean"]);
        assert!(
            !watches(repository.dir()),
            "git tracks this ledger; the folder watch must stay off"
        );

        let plain = TempDir::new("sync-mode-plain");
        plain.seed("main.bean", LEDGER);
        assert!(
            watches(plain.dir()),
            "nothing else tracks this ledger; the folder watch applies"
        );
    }

    /// A write the app makes and then commits is the app's own work on both
    /// records: neither the edit nor the commit may surface as "changed outside
    /// the app".
    #[test]
    fn an_app_write_and_a_commit_are_not_outside_changes() {
        let dir = TempDir::new("sync-commit");
        let (_repo, ledger) = crate::testing::committed_fixture(&dir, LEDGER);
        let state = crate::testing::loaded_state(&ledger);
        let log = log_dir(dir.dir());

        // The app's own edit: the funnel writes the file, records the operation
        // in the log and stages the result.
        let edited = LEDGER
            .replace("-20.00", "-21.00")
            .replace("20.00 USD", "21.00 USD");
        crate::editing::edit_named_file_source(
            &state,
            &ledger.display().to_string(),
            &edited,
            None,
        )
        .unwrap();
        assert!(
            changed(&log).unwrap().is_empty(),
            "the app's own write is not an outside change"
        );

        crate::git::commit_staged(&state, "edit").unwrap();
        assert!(
            changed(&log).unwrap().is_empty(),
            "the commit moved git's index, not the file on disk"
        );
    }

    /// The bar offers a reload; after it, the disk the app serves is the
    /// baseline. Without adopting it, the bar would keep reporting the reload
    /// it just performed, because nothing else rewrites those files.
    #[test]
    fn a_reload_adopts_the_folder_as_the_baseline() {
        let dir = TempDir::new("sync-reload");
        let ledger = dir.seed("main.bean", LEDGER);
        let log = log_dir(dir.dir());
        record(
            &log,
            Origin::User,
            "write",
            &[history::Change::new(
                &ledger,
                None,
                Some(LEDGER.as_bytes()),
            )],
        )
        .unwrap();

        // Another device writes the file while this session is open.
        let moved_on = LEDGER
            .replace("-20.00", "-22.00")
            .replace("20.00 USD", "22.00 USD");
        fs::write(&ledger, &moved_on).unwrap();
        assert_eq!(changed(&log).unwrap().len(), 1, "the folder moved on");

        // The reader takes the reload: the app opens the journal again.
        let loaded = crate::session::load_baselined(&ledger).unwrap();
        adopt_loaded(&loaded);
        assert!(
            changed(&log).unwrap().is_empty(),
            "the reload adopted the disk as what this device has seen"
        );
    }

    /// A peer's deletion is already reflected in the reload — the load does not
    /// read the file — so its old entry must be adopted as gone rather than
    /// reported for ever as a change the reader cannot act on.
    #[test]
    fn a_reload_retires_a_file_the_folder_deleted() {
        let dir = TempDir::new("sync-reload-deleted");
        let ledger = dir.seed("main.bean", LEDGER);
        const EXTRA: &str = "2026-01-05 open Assets:Old\n";
        let extra = dir.seed("old.bean", EXTRA);
        let log = log_dir(dir.dir());
        record(
            &log,
            Origin::User,
            "write",
            &[
                history::Change::new(&ledger, None, Some(LEDGER.as_bytes())),
                history::Change::new(&extra, None, Some(EXTRA.as_bytes())),
            ],
        )
        .unwrap();

        // The peer removed the include and the file; nothing on disk
        // references `old.bean` any more, so the reload cannot read it.
        fs::remove_file(&extra).unwrap();
        assert_eq!(changed(&log).unwrap().len(), 1, "the deletion is real");

        let loaded = crate::session::load_baselined(&ledger).unwrap();
        adopt_loaded(&loaded);
        assert!(
            changed(&log).unwrap().is_empty(),
            "the reload already reflected the deletion"
        );
    }

    /// The gate is also what keeps a git-owned ledger's folder free of `.ledger`
    /// writes on open: there is no baseline to adopt when git is the tracker.
    #[test]
    fn opening_a_repository_ledger_leaves_the_log_unwritten() {
        let dir = TempDir::new("sync-adopt-repository");
        let (_repo, ledger) = crate::testing::committed_fixture(&dir, LEDGER);
        let loaded = crate::session::load_baselined(&ledger).unwrap();

        adopt_loaded(&loaded);
        assert!(
            !log_dir(dir.dir()).exists(),
            "git owns the ledger; opening it must not create the operation log"
        );
    }

    /// The reported shape, in order: open a repository's journal, edit it in the
    /// app (the funnel writes and records it), commit the edit, then switch back
    /// to the first branch. The switch writes the other branch's bytes to the
    /// same file, so the log's last-seen hash describes a version git already
    /// replaced — and the app used to report its own branch switch as a file
    /// "changed in this folder outside the app".
    #[test]
    fn a_branch_switch_is_not_an_outside_change() {
        let dir = TempDir::new("sync-checkout");
        let (repo, ledger) = crate::testing::committed_fixture(&dir, LEDGER);
        let base = repo.head().unwrap().shorthand().unwrap().to_string();
        drop(repo);
        let state = crate::testing::loaded_state(&ledger);
        let root = dir.dir();

        // A branch with its own version of the journal, committed from the app.
        crate::git_ops::create_branch(root, "monthly", true).unwrap();
        let edited = LEDGER
            .replace("-20.00", "-21.00")
            .replace("20.00 USD", "21.00 USD");
        crate::editing::edit_named_file_source(
            &state,
            &ledger.display().to_string(),
            &edited,
            None,
        )
        .unwrap();
        crate::git::commit_staged(&state, "edit").unwrap();
        assert!(changes_for(root).unwrap().is_empty(), "the repository is silent");

        // Switching back rewrites the file behind the app.
        crate::git_ops::checkout(root, &base).unwrap();
        assert_eq!(
            changed(&log_dir(root)).unwrap().len(),
            1,
            "the log's baseline is stale after a checkout"
        );
        assert!(
            changes_for(root).unwrap().is_empty(),
            "a repository's rewrites are not outside changes"
        );
    }

    /// The reported false conflict: this project's own ledger folder is the
    /// repository root, and it holds `package-lock.json` beside `package.json`,
    /// `budget-grid.ts` beside `budget.ts`, and a dated statement beside its
    /// suffixed sibling. The old OneDrive suffix rule paired all of them — 63
    /// pairs in that one folder — and the Conflicts screen listed them as
    /// copies. In a repository the answer is empty even though the names are
    /// there, and the detector itself is quiet on those names too.
    #[test]
    fn a_repository_reports_no_conflict_copies() {
        let dir = TempDir::new("sync-conflicts-repository");
        dir.seed("main.bean", LEDGER);
        for name in [
            "package.json",
            "package-lock.json",
            "budget.ts",
            "budget-grid.ts",
            "2026-10-04.xls",
            "2026-10-04-yatirim.xls",
        ] {
            dir.seed(name, "");
        }
        crate::testing::repo_with_commit(&dir, &["main.bean"]);
        let root = dir.dir();

        assert!(!watches(root), "git owns this folder");
        assert_eq!(
            crate::conflict_names::scan_report(root).unwrap(),
            crate::conflict_names::Report::default(),
            "the names are ordinary neighbours, not copies"
        );
        assert_eq!(
            conflicts_for(root).unwrap(),
            crate::conflict_names::Report::default()
        );
        assert!(changes_for(root).unwrap().is_empty());
    }
}
