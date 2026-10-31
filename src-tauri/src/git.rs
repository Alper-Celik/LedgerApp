// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Git integration: which lines of the journal are uncommitted, the diff view,
//! and committing the ledger's own staged files.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustledger_loader::Ledger;

use crate::session::AppState;

/// The body of [`ledger_diff`](crate::commands::ledger_diff): the ledger's files
/// against HEAD.
pub(crate) fn ledger_diff_text(state: &AppState) -> Result<String, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let ledger = &loaded.ledger;
    let (repo, _) = ledger_repo(ledger).ok_or("the ledger is not inside a git repository")?;
    let mut options = git2::DiffOptions::new();
    // Only the ledger's own files: the repository around it may hold anything.
    for relative in ledger_files_in(&repo, ledger) {
        if let Some(spec) = relative.to_str() {
            options.pathspec(spec);
        }
    }
    // A ledger file the repository does not track yet is part of what the commit
    // screen reviews: without its content a new include would be listed as a
    // delta with no lines (libgit2 omits untracked bodies by default), and the
    // commit would then carry a file the reader never saw.
    options.include_untracked(true).show_untracked_content(true);
    let head = head_tree(&repo);
    let diff = repo
        .diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut options))
        .map_err(|e| format!("git diff: {e}"))?;
    render_diff(&diff)
}

/// The body of [`commit_ledger`](crate::commands::commit_ledger): commit what the
/// frontend staged of the ledger's files, on top of HEAD (an empty repository
/// has no parent).
///
/// Only the ledger's own paths go into the commit. A repository can hold
/// unrelated work — staged or not — and the frontend asked for the ledger's
/// changes, so the commit is built from a private index seeded with HEAD's tree
/// and overlaid with the ledger's staged entries. Everything else stays staged,
/// exactly as `git commit <paths>` leaves it.
/// The index mode a working-tree file is committed with: the executable bit is
/// the only one git records.
fn workdir_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0) {
            return 0o100755;
        }
    }
    let _ = path;
    0o100644
}

pub(crate) fn commit_staged(state: &AppState, message: &str) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("commit message must not be empty".to_string());
    }
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let ledger = &loaded.ledger;
    let (repo, _) = ledger_repo(ledger).ok_or("the ledger is not inside a git repository")?;
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let head = parent.as_ref().and_then(|commit| commit.tree().ok());

    let index = repo.index().map_err(|e| format!("git index: {e}"))?;
    let mut staged = git2::Index::new().map_err(|e| format!("git index: {e}"))?;
    if let Some(head) = &head {
        staged
            .read_tree(head)
            .map_err(|e| format!("git index: {e}"))?;
    }
    // The commit is built from the working tree, not from the repository index:
    // the screen reviewed the working tree's diff, and the index may still hold
    // what an earlier app edit staged — a sync client's newer bytes would then
    // be reviewed but not committed.
    let mut tracked = 0;
    let mut committed: Vec<PathBuf> = Vec::new();
    for relative in ledger_files_in(&repo, ledger) {
        // Any stage above 0 is an unresolved conflict: an add/add conflict has
        // no common ancestor, so checking stage 1 alone would let it through and
        // the commit would silently omit the merged file.
        if (1..=3).any(|stage| index.get_path(&relative, stage).is_some()) {
            return Err(format!(
                "{} has an unresolved merge; resolve it before committing",
                relative.display()
            ));
        }
        // A ledger file the folder keeps out of git — a private include named in
        // `.gitignore` — is not in the diff the screen reviewed (that diff shows
        // untracked, not ignored, files), so committing it would publish a file
        // the reader never saw.
        if repo.status_should_ignore(&relative).unwrap_or(false) {
            continue;
        }
        // A ledger file the repository does not track yet — an include created
        // outside the app, or a journal made before the repository — is part of
        // what the diff screen showed, so it is committed from the working tree
        // like any other ledger path. Skipping it left a reviewed change out of
        // the commit, and the diff never cleared.
        let disk = repo.workdir().map(|dir| dir.join(&relative));
        match disk.filter(|path| path.is_file()) {
            Some(path) => {
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let blob = repo
                    .blob(&bytes)
                    .map_err(|e| format!("git blob for {}: {e}", relative.display()))?;
                // The private index is not backed by a repository, so the entry
                // is built here rather than read off the workdir by git.
                let mode = workdir_mode(&path);
                let entry = git2::IndexEntry {
                    ctime: git2::IndexTime::new(0, 0),
                    mtime: git2::IndexTime::new(0, 0),
                    dev: 0,
                    ino: 0,
                    mode,
                    uid: 0,
                    gid: 0,
                    file_size: bytes.len() as u32,
                    id: blob,
                    flags: 0,
                    flags_extended: 0,
                    path: relative.to_string_lossy().replace('\\', "/").into_bytes(),
                };
                staged.add(&entry).map_err(|e| format!("git index: {e}"))?;
            }
            None => {
                // Deleted on disk: the commit records the deletion — when the
                // index had it. An untracked path the folder no longer holds has
                // nothing to record.
                if index.get_path(&relative, 0).is_none() {
                    continue;
                }
                staged
                    .remove_path(&relative)
                    .map_err(|e| format!("git index: {e}"))?;
            }
        }
        committed.push(relative);
        tracked += 1;
    }
    // With no ledger file in it the tree is HEAD's own, message and all.
    if tracked == 0 {
        return Err("nothing staged to commit".to_string());
    }
    let tree_id = staged
        .write_tree_to(&repo)
        .map_err(|e| format!("git index: {e}"))?;
    if head.as_ref().is_some_and(|head| head.id() == tree_id) {
        return Err("nothing staged to commit".to_string());
    }
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("git tree: {e}"))?;
    // The user's own identity, exactly as git would use it.
    let signature = repo
        .signature()
        .map_err(|e| format!("git identity: {e} (set user.name and user.email)"))?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let commit = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .map_err(|e| format!("git commit: {e}"))?;
    // The commit took the working tree; the repository index still holds what an
    // earlier stage put there, so every path whose staged bytes differed is left
    // modified-in-index against the new HEAD — which the branch and merge guards
    // read as uncommitted work and refuse over. Only the committed paths are
    // reset, so a conflicted path's stages are untouched.
    let written = repo
        .find_commit(commit)
        .map_err(|e| format!("git commit: {e}"))?;
    repo.reset_default(Some(written.as_object()), committed.iter())
        .map_err(|e| format!("git index: {e}"))?;
    Ok(commit.to_string().chars().take(7).collect())
}

/// The repository the ledger lives in, found by walking up from its files, so
/// the ledger directory itself does not have to sit at the repository's root.
fn ledger_repo(ledger: &Ledger) -> Option<(git2::Repository, PathBuf)> {
    ledger
        .source_map
        .files()
        .iter()
        .find_map(|file| repo_and_relative(&file.path))
}

/// A file's repository, found by walking up from the file, together with the
/// file's path relative to that repository's working directory (git2 only takes
/// repository-relative paths). `None` for a file outside any repository.
fn repo_and_relative(path: &Path) -> Option<(git2::Repository, PathBuf)> {
    let absolute = fs::canonicalize(path).ok()?;
    let repo = git2::Repository::discover(&absolute).ok()?;
    let workdir = fs::canonicalize(repo.workdir()?).ok()?;
    let relative = absolute.strip_prefix(&workdir).ok()?.to_path_buf();
    Some((repo, relative))
}

/// The ledger's files that live in `repo`, as paths relative to it — the guard
/// that keeps unrelated files of the same repository out of the ledger's diff
/// and commit.
fn ledger_files_in(repo: &git2::Repository, ledger: &Ledger) -> Vec<PathBuf> {
    ledger
        .source_map
        .files()
        .iter()
        .filter_map(|file| repo_and_relative(&file.path))
        .filter(|(owner, _)| owner.path() == repo.path())
        .map(|(_, relative)| relative)
        .collect()
}

/// The repository's HEAD tree, or `None` while HEAD has no commit yet.
fn head_tree(repo: &git2::Repository) -> Option<git2::Tree<'_>> {
    repo.head().ok()?.peel_to_tree().ok()
}

/// The file's content at HEAD; `None` when HEAD does not have it — an untracked
/// file, or a repository whose first commit is still to come.
fn head_blob(repo: &git2::Repository, relative: &Path) -> Option<String> {
    let tree = head_tree(repo)?;
    let entry = tree.get_path(relative).ok()?;
    let blob = repo.find_blob(entry.id()).ok()?;
    String::from_utf8(blob.content().to_vec()).ok()
}

/// Stage a freshly written ledger file so a later commit picks it up. A ledger
/// outside version control is not a failure: the write already happened and
/// there is simply nothing to stage.
pub(crate) fn stage_file(path: &Path) -> Result<(), String> {
    let Some((repo, relative)) = repo_and_relative(path) else {
        return Ok(());
    };
    let mut index = repo.index().map_err(|e| format!("git index: {e}"))?;
    index
        .add_path(&relative)
        .map_err(|e| format!("git add {}: {e}", relative.display()))?;
    index.write().map_err(|e| format!("git index: {e}"))
}

/// Render a diff as one `=== path ===` block per changed file: git's own unified
/// patch text, which already carries the hunk headers, under a header the
/// frontend can split on.
fn render_diff(diff: &git2::Diff<'_>) -> Result<String, String> {
    let mut text = String::new();
    let mut current: Option<String> = None;
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let name = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        if current.as_deref() != Some(name.as_str()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("=== {name} ===\n"));
            current = Some(name);
        }
        // The callback is handed each line's text without its diff marker, so
        // put the marker back and leave the block reading as a patch the frontend
        // can colour line by line.
        match line.origin() {
            '+' | '-' | ' ' => text.push(line.origin()),
            _ => {}
        }
        text.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| format!("git diff: {e}"))?;
    Ok(text)
}

/// Which lines of the ledger's files differ from what they are measured
/// against, keyed by source-map file id — the measure behind
/// [`TransactionView::modified`](crate::model::TransactionView::modified).
///
/// A file's reference is its HEAD blob when git has one, and otherwise the text
/// the session started with (see [`Loaded::baseline`](crate::session::Loaded)):
/// a ledger that is not committed yet is not a change in itself. Empty when the
/// ledger is not in a repository or nothing differs.
pub(crate) fn working_tree_changes(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
) -> HashMap<u16, Vec<(u32, u32)>> {
    let mut changed: HashMap<u16, Vec<(u32, u32)>> = HashMap::new();
    for file in ledger.source_map.files() {
        // `.ledger/` is the app's own state (conversations, the operation log),
        // not ledger content: it must neither light up as a change nor be
        // counted as one if a file in it ever gets loaded by accident.
        if crate::conversations::is_session_path(&file.path) {
            continue;
        }
        let Some(id) = u16::try_from(file.id).ok() else {
            continue;
        };
        let Some((repo, relative)) = repo_and_relative(&file.path) else {
            continue;
        };
        let lines = match head_blob(&repo, &relative) {
            // Committed: the working tree is compared against HEAD.
            Some(head) => {
                if head == *file.source {
                    continue;
                }
                changed_lines(&head, &file.source)
            }
            // Not committed: compared against the session's starting point, so
            // an untracked ledger lights up only where it was edited.
            None => {
                let Some(start) = baseline.get(&file.path) else {
                    continue;
                };
                if start.as_ref() == file.source.as_ref() {
                    continue;
                }
                changed_lines(start, &file.source)
            }
        };
        if !lines.is_empty() {
            changed.insert(id, lines);
        }
    }
    changed
}

/// The 1-based lines of `new` that `old` does not have, merged into inclusive
/// ranges. libgit2's own line diff drives this, so rewrites, insertions and
/// deletions are all covered; a deleted line has no number of its own in the new
/// file, so the line that takes its place is the one reported.
fn changed_lines(old: &str, new: &str) -> Vec<(u32, u32)> {
    let Ok(patch) = git2::Patch::from_buffers(
        old.as_bytes(),
        Some(Path::new("head")),
        new.as_bytes(),
        Some(Path::new("worktree")),
        None,
    ) else {
        return Vec::new();
    };
    let mut lines: BTreeSet<u32> = BTreeSet::new();
    // A deletion at the end of the file points past its last line; clamping
    // keeps the reported change on a line the file actually has.
    let last_line = u32::try_from(new.lines().count().max(1)).unwrap_or(u32::MAX);
    for hunk_index in 0..patch.num_hunks() {
        let Ok((hunk, count)) = patch.hunk(hunk_index) else {
            continue;
        };
        let mut line = hunk.new_start();
        for line_index in 0..count {
            let Ok(diff_line) = patch.line_in_hunk(hunk_index, line_index) else {
                continue;
            };
            match diff_line.origin() {
                '+' => {
                    lines.insert(line.min(last_line));
                    line += 1;
                }
                '-' => {
                    lines.insert(line.min(last_line));
                }
                ' ' => line += 1,
                // `\ No newline at end of file` and friends occupy no line.
                _ => {}
            }
        }
    }
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for line in lines {
        match ranges.last_mut() {
            Some(last) if line <= last.1.saturating_add(1) => last.1 = line,
            _ => ranges.push((line, line)),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::edit_transaction;
    use crate::model::TransactionEdit;
    use crate::testing::*;

    /// The commit takes the working tree even when the repository index still
    /// holds an older staged version, and it leaves the index agreeing with the
    /// new HEAD — otherwise every such path reads as uncommitted work and the
    /// branch and merge guards refuse over it.
    #[test]
    fn a_commit_built_from_the_worktree_leaves_the_index_clean() {
        let dir = TempDir::new("commit-workdir");
        let (repo, path) = committed_fixture(
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
        let head = repo.head().unwrap().target().unwrap();

        // Stage one version through the app, then let a sync client put another
        // on disk: the screen's diff showed the disk version.
        let staged = fs::read_to_string(&path)
            .unwrap()
            .replace("-20.00", "-21.00");
        fs::write(&path, &staged).unwrap();
        crate::editing::edit_transaction(
            &state,
            crate::model::TransactionEdit {
                id: view_id(&state, "Groceries"),
                date: "2026-01-10".to_string(),
                flag: "*".to_string(),
                payee: None,
                narration: "Groceries".to_string(),
                postings: Vec::new(),
            },
        )
        .ok();
        let on_disk = fs::read_to_string(&path).unwrap();
        let newer = on_disk.replace("-21.00", "-22.00");
        fs::write(&path, &newer).unwrap();

        let commit = commit_staged(&state, "from the worktree").unwrap();

        assert_eq!(commit.len(), 7);
        let head_after = repo.head().unwrap().target().unwrap();
        assert_ne!(head_after, head);
        // The file is clean: index and HEAD agree, and the working tree matches.
        let statuses = repo.statuses(None).unwrap();
        let dirty: Vec<String> = statuses
            .iter()
            .map(|entry| entry.path().unwrap_or("?").to_string())
            .collect();
        assert!(dirty.is_empty(), "{}", dirty.join(", "));
        // And the commit holds what the screen reviewed.
        let commit = repo.find_commit(head_after).unwrap();
        let tree = commit.tree().unwrap();
        let blob = tree
            .get_path(Path::new(path.file_name().unwrap()))
            .unwrap()
            .to_object(&repo)
            .unwrap()
            .peel_to_blob()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(blob.content()), newer);
    }

    /// A ledger file the folder holds but the repository does not track — an
    /// include the app never wrote, in a repository made before it existed — is
    /// part of the diff the commit screen showed. Leaving it out of the commit
    /// dropped a reviewed change and left the diff uncleared.
    #[test]
    fn an_untracked_include_is_committed_with_the_ledger() {
        let dir = TempDir::new("commit-untracked");
        dir.seed(
            "main.bean",
            "include \"expenses.bean\"\n\n2026-01-01 open Assets:Bank:Checking\n2026-01-01 open Expenses:Food\n",
        );
        dir.seed(
            "expenses.bean",
            "2026-01-10 * \"Groceries\"\n  Assets:Bank:Checking            -20.00 USD\n  Expenses:Food                    20.00 USD\n",
        );
        let repo = crate::testing::repo_with_commit(&dir, &["main.bean"]);
        let state = loaded_state(&dir.file("main.bean"));
        let diff = ledger_diff_text(&state).unwrap();
        assert!(
            diff.contains("=== expenses.bean ===\n"),
            "the commit screen shows the untracked include, body included: {diff}"
        );
        assert!(
            diff.contains("+2026-01-10 * \"Groceries\"\n"),
            "the untracked file's lines read as additions: {diff}"
        );

        commit_staged(&state, "add the folder's own include").unwrap();

        let tree = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        assert!(
            tree.get_name("expenses.bean").is_some(),
            "the include the diff showed is in the commit"
        );
        assert!(tree.get_name("main.bean").is_some());
    }

    /// A ledger file the folder keeps out of git is not part of the review
    /// screen — `ledger_diff` shows untracked files, not ignored ones — so the
    /// commit must leave it alone. A private accounts include named in
    /// `.gitignore` must never be published by a commit made in the app.
    #[test]
    fn an_ignored_include_is_left_out_of_the_commit() {
        let dir = TempDir::new("commit-ignored");
        dir.seed(
            "main.bean",
            "include \"private.bean\"\n\n2026-01-01 open Assets:Bank:Checking\n",
        );
        dir.seed("private.bean", "2026-01-01 open Liabilities:Card\n");
        dir.seed(".gitignore", "private.bean\n");
        let repo = crate::testing::repo_with_commit(&dir, &["main.bean"]);
        let state = loaded_state(&dir.file("main.bean"));

        let diff = ledger_diff_text(&state).unwrap();
        assert!(
            !diff.contains("private.bean"),
            "the ignored include is not on the review screen: {diff}"
        );

        // Something the reader did review, so there is a commit to inspect.
        fs::write(
            dir.file("main.bean"),
            "include \"private.bean\"\n\n2026-01-01 open Assets:Bank:Checking\n2026-01-01 open Assets:Savings\n",
        )
        .unwrap();
        commit_staged(&state, "another account").unwrap();

        let tree = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        assert!(
            tree.get_name("private.bean").is_none(),
            "an ignored ledger file is never committed"
        );
        assert!(tree.get_name("main.bean").is_some());
    }

    #[test]
    fn reports_the_diff_and_commits_what_was_staged() {
        let dir = TempDir::new("commit");
        let (repo, path) = committed_fixture(
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
        // Unrelated work in the same repository must stay out of a ledger commit.
        fs::write(dir.file("notes.txt"), "shopping list\n").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("notes.txt")).unwrap();
            index.write().unwrap();
        }
        edit_transaction(
            &state,
            TransactionEdit {
                flag: "!".to_string(),
                payee: Some("Corner Shop".to_string()),
                ..edit_of(
                    view_id(&state, "Groceries"),
                    "2026-01-10",
                    "Groceries",
                    vec![posting(0, "-25.00"), posting(1, "25.00")],
                )
            },
        )
        .unwrap();

        // The diff view sees the staged change: one block per file, with the
        // old and the new header and units lines in it.
        let diff = ledger_diff_text(&state).unwrap();
        assert!(diff.contains("=== main.bean ===\n"), "diff: {diff}");
        assert!(
            diff.contains("-2026-01-10 * \"Groceries\"\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("+2026-01-10 ! \"Corner Shop\" \"Groceries\"\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("+  Assets:Bank:Checking            -25.00 USD\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("-  Expenses:Food                    20.00 USD\n"),
            "diff: {diff}"
        );

        // An empty message is not a commit.
        assert!(commit_staged(&state, "").is_err());
        assert!(commit_staged(&state, "   ").is_err());

        let short = commit_staged(&state, "groceries went up").unwrap();
        assert_eq!(short.len(), 7, "short id: {short}");
        assert!(
            short.chars().all(|character| character.is_ascii_hexdigit()),
            "short id: {short}"
        );

        // HEAD is that commit, and its tree carries what the file says.
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert!(head.id().to_string().starts_with(&short));
        assert_eq!(head.message().unwrap(), "groceries went up");
        assert_eq!(head.parent_count(), 1);
        let entry = head
            .tree()
            .unwrap()
            .get_path(Path::new("main.bean"))
            .unwrap();
        let blob = repo.find_blob(entry.id()).unwrap();
        assert_eq!(blob.content(), fs::read(&path).unwrap().as_slice());
        // The unrelated staged file stayed out of the commit, and out of HEAD —
        // it is still staged, waiting for a commit of its own.
        assert!(
            head.tree()
                .unwrap()
                .get_path(Path::new("notes.txt"))
                .is_err()
        );
        assert!(
            repo.index()
                .unwrap()
                .get_path(Path::new("notes.txt"), 0)
                .is_some()
        );

        // The ledger has nothing left to commit.
        assert!(commit_staged(&state, "again").is_err());
    }

    /// The ledger the untracked-file tests work on: two transactions, one of
    /// them with a leg whose amount is interpolated at load time.
    const UNTRACKED_LEDGER: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
";

    /// A repository that has commits, but not of the ledger — the workspace's
    /// shape while the journal is still untracked.
    fn untracked_fixture(dir: &TempDir) -> (git2::Repository, PathBuf) {
        let path = dir.file("main.bean");
        fs::write(&path, UNTRACKED_LEDGER).unwrap();
        fs::write(dir.file("README.md"), "notes\n").unwrap();
        let repo = repo_with_commit(dir, &["README.md"]);
        (repo, path)
    }

    #[test]
    fn an_untracked_ledger_reports_only_its_own_edits() {
        let dir = TempDir::new("untracked");
        let (_, path) = untracked_fixture(&dir);
        let state = loaded_state(&path);

        // Loading a ledger git has never seen is not a change in itself: the
        // register starts quiet instead of lighting up whole file.
        assert_eq!(modified_narrations(&state), Vec::<String>::new());

        let id = view_id(&state, "Groceries");
        edit_transaction(
            &state,
            edit_of(id, "2026-01-10", "Groceries", vec![posting(0, "-25.00")]),
        )
        .unwrap();

        // The edit — not the file's untracked state — is what shows up.
        assert_eq!(modified_narrations(&state), ["Groceries"]);
        assert_eq!(view_id(&state, "Groceries"), id);
    }

    #[test]
    fn an_untracked_ledger_commits_as_a_new_file() {
        let dir = TempDir::new("untracked-commit");
        let (repo, path) = untracked_fixture(&dir);
        let state = loaded_state(&path);
        edit_transaction(
            &state,
            edit_of(
                view_id(&state, "Groceries"),
                "2026-01-10",
                "Groceries",
                vec![posting(0, "-25.00")],
            ),
        )
        .unwrap();

        // The staged ledger — untracked, so new to HEAD — is in the diff, and
        // its lines read as additions.
        let diff = ledger_diff_text(&state).unwrap();
        assert!(diff.contains("=== main.bean ===\n"), "diff: {diff}");
        assert!(diff.contains("new file mode"), "diff: {diff}");
        assert!(
            diff.contains("+2026-01-01 open Assets:Bank:Checking\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("+  Assets:Bank:Checking            -25.00 USD\n"),
            "diff: {diff}"
        );

        let short = commit_staged(&state, "add the journal").unwrap();
        assert_eq!(short.len(), 7, "short id: {short}");
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert!(head.id().to_string().starts_with(&short));
        let tree = head.tree().unwrap();
        // The ledger landed in the commit, and the commit did not drop what was
        // already there.
        let entry = tree.get_path(Path::new("main.bean")).unwrap();
        let blob = repo.find_blob(entry.id()).unwrap();
        assert_eq!(blob.content(), fs::read(&path).unwrap().as_slice());
        assert!(tree.get_path(Path::new("README.md")).is_ok());
    }
}
