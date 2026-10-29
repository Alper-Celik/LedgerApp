// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Branches, history, staging and merges on the ledger's repository.
//!
//! [`crate::git`] owns the app's own flows — which lines of the journal are
//! uncommitted, the diff view, and committing the ledger's staged files. This
//! module is the layer a branch/history screen will drive: it is implemented and
//! tested, and **no command exposes it yet** (the `/history` route reads the
//! operation log in [`crate::history`], not git). It holds no state between
//! calls (every call opens the workdir it is given), and it phrases failures for
//! a person — "cannot switch branches: books/2026.bean has uncommitted changes"
//! rather than libgit2's negative error code.
//!
//! The operations are the ones a single-file ledger needs, no more. Rebase and
//! cherry-pick are deliberately absent: both rewrite or replay commits, and a
//! ledger's history is the audit trail the journal exists for. `revert` is the
//! one history-changing operation kept, because it records the undo as a new
//! commit instead of pretending the original never happened.

use std::fs;
use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{BranchType, DiffOptions, Repository, RepositoryState, ResetType, Sort};

/// One branch of the repository, as the branch list shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct BranchInfo {
    /// The branch's short name: `main`, or `origin/main` for a remote branch.
    pub name: String,
    /// Whether HEAD is on this branch. Always false for remote branches, which
    /// cannot be the checked-out branch.
    pub is_current: bool,
    pub is_remote: bool,
    /// The remote-tracking branch this local branch follows, when one is
    /// configured. `None` for remote branches and for locals with no upstream.
    pub upstream: Option<String>,
}

/// One commit in the history, as the log rows show it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct CommitInfo {
    /// The full hex object id, the handle the other calls take.
    pub id: String,
    /// The first seven characters, for the row's id column.
    pub short: String,
    /// The message's first line, as git's own `%s` would show it.
    pub summary: String,
    pub author: String,
    pub email: String,
    /// Seconds since the Unix epoch, in the author's clock — the time git
    /// records and `git log` prints by default. The offset is not carried; a
    /// ledger app only needs the ordering and the day.
    ///
    /// Exported as `f64` because specta refuses 64-bit integers (a JS number
    /// cannot carry one); every Unix second this century is exact in a double.
    #[specta(type = f64)]
    pub time: i64,
    /// How many files the commit changed, the row's "3 files" badge. `u32`: a
    /// commit that touched four billion files is not a ledger.
    pub files: u32,
}

/// How a merge ended.
///
/// The clean non-fast-forward case is `Conflicts` with an empty list: the merge
/// result is staged and waiting for [`merge_continue`], exactly like a resolved
/// one, and reporting it that way keeps this type to the three states the
/// screen has to draw.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MergeOutcome {
    /// HEAD is on the merged commit; nothing else is left to do.
    FastForward { new_head: String },
    /// The branch already contains `theirs`; nothing happened.
    UpToDate,
    /// The merge is staged and awaiting [`merge_continue`]. Each entry is a
    /// path that still needs a resolved text; an empty list means the merge is
    /// clean and only needs finishing.
    Conflicts(Vec<ConflictFile>),
}

/// One path the merge left in conflict, with what kind of edit it was.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct ConflictFile {
    /// Repository-relative path, as git writes it.
    pub path: String,
    /// One of `both_modified`, `both_added`, `deleted_by_us` or
    /// `deleted_by_them`; `added_by_us`/`added_by_them` cover the rarer
    /// add/rename side, where only one stage exists; `deleted_by_both` and
    /// `unclassified` are the remaining stage shapes, which a merge of two
    /// text files cannot normally produce.
    pub kind: String,
    /// How many of the three index stages (base, ours, theirs) exist: 3 for a
    /// plain both-modified conflict, fewer when one side added or deleted the
    /// file, which is a hint the UI can use to pick the right editor. `u8`: git
    /// has exactly three stages, and specta refuses wider integers.
    pub stages: u8,
}

/// The three versions of a conflicted file, read from the index stages rather
/// than from the working tree's `<<<<<<<` markers, so the UI can run a real
/// three-way merge over them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct ConflictTexts {
    /// Stage 1: the merge base.
    pub base: Option<String>,
    /// Stage 2: the branch being merged into.
    pub ours: Option<String>,
    /// Stage 3: the branch merged in.
    pub theirs: Option<String>,
}

/// The branches of the repository, current first and then by name.
///
/// The order puts "where the user is" at the top of the list; the rest is one
/// alphabetical column, with remote branches sorting under their `origin/`
/// name beside the locals they track.
pub fn branches(repo: &Path) -> Result<Vec<BranchInfo>, String> {
    let repo = open(repo)?;
    let current = current_branch(&repo);
    let mut branches = Vec::new();
    for (kind, is_remote) in [(BranchType::Local, false), (BranchType::Remote, true)] {
        let found = repo
            .branches(Some(kind))
            .map_err(|e| format!("git branches: {e}"))?;
        for branch in found {
            let (branch, _) = branch.map_err(|e| format!("git branches: {e}"))?;
            let Some(name) = branch
                .name()
                .map_err(|e| format!("git branches: {e}"))?
                .map(str::to_string)
            else {
                // A branch whose name is not UTF-8 cannot be shown or passed
                // back, so it is left out rather than mangled.
                continue;
            };
            let upstream = if is_remote {
                None
            } else {
                branch
                    .upstream()
                    .ok()
                    .and_then(|upstream| upstream.name().ok().flatten().map(str::to_string))
            };
            branches.push(BranchInfo {
                is_current: !is_remote && current.as_deref() == Some(name.as_str()),
                name,
                is_remote,
                upstream,
            });
        }
    }
    branches.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(branches)
}

/// Create a local branch at HEAD, optionally switching to it.
///
/// When `checkout` is set, a dirty working tree refuses the whole call before
/// the branch exists (see [`checkout`]), so the refusal leaves no stray branch
/// behind.
pub fn create_branch(repo: &Path, name: &str, checkout: bool) -> Result<(), String> {
    let repo = open(repo)?;
    let name = branch_name(name)?;
    if repo.find_branch(&name, BranchType::Local).is_ok() {
        return Err(format!("branch {name} already exists"));
    }
    if checkout {
        guard_clean(&repo, "switch branches")?;
    }
    let head = repo
        .head()
        .map_err(|_| "cannot create a branch: the repository has no commits yet".to_string())?
        .peel_to_commit()
        .map_err(|e| format!("cannot create branch {name}: {e}"))?;
    repo.branch(&name, &head, false)
        .map_err(|e| format!("cannot create branch {name}: {e}"))?;
    if checkout {
        switch(&repo, &name)?;
    }
    Ok(())
}

/// Switch to a local branch.
///
/// Refused while the working tree has tracked changes, naming the files:
/// switching with a dirty ledger is how edits get lost or end up on the wrong
/// branch. Untracked files do not block the switch, but one that the target
/// branch's tree would overwrite does — libgit2's safe checkout would refuse
/// anyway, only with a bare conflict count instead of a file name.
pub fn checkout(repo: &Path, name: &str) -> Result<(), String> {
    let repo = open(repo)?;
    if current_branch(&repo).as_deref() == Some(name) {
        return Ok(());
    }
    if repo.find_branch(name, BranchType::Local).is_err() {
        return Err(format!(
            "no local branch named {name}: create it first, or check the name"
        ));
    }
    guard_clean(&repo, "switch branches")?;
    switch(&repo, name)
}

/// The commits reachable from HEAD, newest first, at most `limit` of them.
///
/// `path` restricts the log to the commits that touched one file — the ledger's
/// own history view. It accepts the file's absolute path as the rest of the app
/// holds it, or a repository-relative one.
pub fn log(repo: &Path, limit: usize, path: Option<&Path>) -> Result<Vec<CommitInfo>, String> {
    let repo = open(repo)?;
    if limit == 0 || head_commit(&repo).is_none() {
        // No commit yet: an empty history, not an error — the screen shows the
        // first-commit state.
        return Ok(Vec::new());
    }
    let filter = match path {
        Some(path) => Some(relative(&repo, path)?),
        None => None,
    };
    let mut walk = repo.revwalk().map_err(|e| format!("git log: {e}"))?;
    // Newest commit time first, as `git log` shows. A ledger's history is a
    // line, so there is no graph for a topological sort to rearrange.
    walk.set_sorting(Sort::TIME)
        .map_err(|e| format!("git log: {e}"))?;
    walk.push_head().map_err(|e| format!("git log: {e}"))?;

    let mut commits = Vec::new();
    for id in walk {
        if commits.len() >= limit {
            break;
        }
        let id = id.map_err(|e| format!("git log: {e}"))?;
        let commit = repo.find_commit(id).map_err(|e| format!("git log: {e}"))?;
        if let Some(filter) = &filter {
            // The filter is a predicate over the commit's own diff. The count
            // below stays the commit's whole change set, so the row's badge
            // does not shrink just because a path was asked for.
            if changed_files(&repo, &commit, Some(filter))? == 0 {
                continue;
            }
        }
        let files = u32::try_from(changed_files(&repo, &commit, None)?).unwrap_or(u32::MAX);
        let author = commit.author();
        commits.push(CommitInfo {
            id: id.to_string(),
            short: short(id),
            summary: commit
                .summary()
                .ok()
                .flatten()
                .unwrap_or_default()
                .to_string(),
            author: author.name().unwrap_or_default().to_string(),
            email: author.email().unwrap_or_default().to_string(),
            time: author.when().seconds(),
            files,
        });
    }
    Ok(commits)
}

/// The patch a commit introduced, against its first parent.
///
/// Rendered in the same `=== path ===` block shape as the ledger's diff view
/// (`git::render_diff`), so the history screen can reuse the diff component
/// line for line; that renderer is private to `git` and the ledger's own diff
/// stays its responsibility.
pub fn commit_diff(repo: &Path, id: &str) -> Result<String, String> {
    let repo = open(repo)?;
    let commit = find_commit(&repo, id)?;
    let tree = commit.tree().map_err(|e| format!("git tree: {e}"))?;
    let parent_tree = parent_tree(&commit)?;
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| format!("git diff: {e}"))?;
    render_patch(&diff)
}

/// Stage the given paths, exactly as `git add` would: a modified file stages
/// its new content, a deleted one stages the deletion.
///
/// Paths may be absolute (what the rest of the app holds) or
/// repository-relative.
pub fn stage(repo: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let repo = open(repo)?;
    let mut index = repo.index().map_err(|e| format!("git index: {e}"))?;
    let workdir = repo_workdir(&repo)?;
    for path in paths {
        let relative = relative(&repo, path)?;
        if workdir.join(&relative).exists() {
            index
                .add_path(&relative)
                .map_err(|e| format!("git add {}: {e}", relative.display()))?;
        } else {
            // A deleted file is staged as its removal, since there is no
            // content to add. A path that was never in the index is not an
            // error here: staging it is simply nothing to do.
            match index.remove_path(&relative) {
                Ok(()) => {}
                Err(e) if e.code() == git2::ErrorCode::NotFound => {}
                Err(e) => return Err(format!("git rm {}: {e}", relative.display())),
            }
        }
    }
    index.write().map_err(|e| format!("git index: {e}"))
}

/// Unstage the given paths: their index entries go back to HEAD's version,
/// leaving the working tree untouched — `git reset HEAD -- <paths>`.
///
/// In a repository with no commit yet, "HEAD's version" is absence, so the
/// paths leave the index altogether.
pub fn unstage(repo: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let repo = open(repo)?;
    let head = head_commit(&repo);
    let target = head.as_ref().map(|commit| commit.as_object());
    for path in paths {
        let relative = relative(&repo, path)?;
        repo.reset_default(target, [relative.clone()])
            .map_err(|e| format!("git unstage {}: {e}", relative.display()))?;
    }
    Ok(())
}

/// Throw away the working tree's version of the given paths and put the index's
/// version back — `git checkout -- <paths>`.
///
/// Restoring from the index (rather than from HEAD) means a staged change
/// survives, exactly as git leaves it: the user discards what was typed after
/// staging, not what was staged. A path with no index entry is skipped — that
/// covers an untracked file, which has no version to restore, and a deletion
/// already staged, whose index entry is deliberately absent. Deleting
/// uncommitted files is `git clean`, and this app never deletes bytes the user
/// has nowhere else.
pub fn discard(repo: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let repo = open(repo)?;
    let mut index = repo.index().map_err(|e| format!("git index: {e}"))?;
    let mut options = CheckoutBuilder::new();
    // Overwrite the working copy of the paths the index knows, and touch
    // nothing else.
    options.force().update_only(true);
    let mut restored = 0;
    for path in paths {
        let relative = relative(&repo, path)?;
        if index.get_path(&relative, 0).is_none() {
            continue;
        }
        options.path(relative);
        restored += 1;
    }
    if restored == 0 {
        return Ok(());
    }
    repo.checkout_index(Some(&mut index), Some(&mut options))
        .map_err(|e| format!("git checkout: {e}"))
}

/// Merge `theirs` into HEAD.
///
/// `theirs` is a branch, tag or commit as git would name it. A fast-forward
/// moves the current branch and reports its new head; an already-contained
/// branch reports `UpToDate`; anything else is merged into the index and
/// reported as `Conflicts` — with the conflicted paths, or an empty list for a
/// clean merge awaiting [`merge_continue`].
///
/// The working tree must be clean, because a merge writes to it, and untracked
/// files the incoming tree would overwrite are refused by name: safe checkout
/// would otherwise fail with "1 conflict prevents checkout", which says nothing.
pub fn merge(repo: &Path, theirs: &str) -> Result<MergeOutcome, String> {
    let repo = open(repo)?;
    match repo.state() {
        RepositoryState::Clean => {}
        RepositoryState::Merge => {
            return Err("a merge is already in progress: finish it or abort it first".to_string());
        }
        _ => {
            return Err(
                "another git operation is in progress: finish it or abort it first".to_string(),
            );
        }
    }
    let annotated = annotated_commit(&repo, theirs)?;
    let (analysis, _) = repo
        .merge_analysis(&[&annotated])
        .map_err(|e| format!("git merge: {e}"))?;
    if analysis.is_up_to_date() {
        return Ok(MergeOutcome::UpToDate);
    }
    if analysis.is_unborn() {
        return Err("cannot merge: the repository has no commits yet".to_string());
    }
    guard_clean(&repo, "merge")?;
    let target = repo
        .find_commit(annotated.id())
        .map_err(|e| format!("git merge: {e}"))?;
    let target_tree = target.tree().map_err(|e| format!("git merge: {e}"))?;
    let collisions = untracked_collisions(&repo, &target_tree)?;
    if !collisions.is_empty() {
        return Err(overwritten_error("merge", &collisions));
    }

    if analysis.is_fast_forward() {
        let mut options = CheckoutBuilder::new();
        options.safe();
        repo.checkout_tree(target.as_object(), Some(&mut options))
            .map_err(|e| format!("cannot merge {theirs}: {e}"))?;
        // The branch moves only after its tree is in the working directory:
        // a failed checkout leaves the branch where it was.
        let head = repo.head().map_err(|e| format!("git merge: {e}"))?;
        if head.is_branch() {
            let name = head
                .name()
                .map_err(|e| format!("cannot merge: {e}"))?
                .to_string();
            repo.reference(&name, annotated.id(), true, "merge: fast-forward")
                .map_err(|e| format!("cannot merge {theirs}: {e}"))?;
        } else {
            repo.set_head_detached(annotated.id())
                .map_err(|e| format!("cannot merge {theirs}: {e}"))?;
        }
        return Ok(MergeOutcome::FastForward {
            new_head: annotated.id().to_string(),
        });
    }

    repo.merge(&[&annotated], None, None)
        .map_err(|e| format!("cannot merge {theirs}: {e}"))?;
    Ok(MergeOutcome::Conflicts(conflicts(&repo)?))
}

/// The three index stages of a conflicted path, as UTF-8.
///
/// A merge writes `<<<<<<<` markers into the working file, but those markers
/// are the merged output, not the merge's inputs; the index still holds the
/// base (stage 1), ours (stage 2) and theirs (stage 3) blobs. Handing the three
/// texts to the editor lets it merge the file for real and lets
/// [`resolve`] write that result back.
pub fn conflict_texts(repo: &Path, path: &Path) -> Result<ConflictTexts, String> {
    let repo = open(repo)?;
    let relative = relative(&repo, path)?;
    let index = repo.index().map_err(|e| format!("git index: {e}"))?;
    let read = |stage: i32, label: &str| -> Result<Option<String>, String> {
        let Some(entry) = index.get_path(&relative, stage) else {
            return Ok(None);
        };
        let blob = repo
            .find_blob(entry.id)
            .map_err(|e| format!("git blob: {e}"))?;
        // Lossy decoding would edit the bytes the user actually has, so a
        // non-UTF-8 ledger file is reported instead of silently changed.
        String::from_utf8(blob.content().to_vec())
            .map(Some)
            .map_err(|_| format!("{} is not UTF-8 text ({label})", relative.display()))
    };
    let texts = ConflictTexts {
        base: read(1, "the merge base")?,
        ours: read(2, "our version")?,
        theirs: read(3, "their version")?,
    };
    if texts.base.is_none() && texts.ours.is_none() && texts.theirs.is_none() {
        return Err(format!("{} is not in conflict", relative.display()));
    }
    Ok(texts)
}

/// Finish the merge in progress: commit the index that [`merge`] (or a clean
/// merge) staged, with HEAD and the merged branches as parents.
///
/// Refuses while any path still has conflict stages, naming them: a merge
/// commit with markers in it would enter the ledger's history as if it were
/// booked data. The message git wrote into `MERGE_MSG` is reused, so the commit
/// reads like the one `git merge` would have made.
pub fn merge_continue(repo: &Path) -> Result<(), String> {
    let mut repo = open(repo)?;
    if repo.state() != RepositoryState::Merge {
        return Err("no merge in progress: there is nothing to continue".to_string());
    }
    // Collected first: `mergehead_foreach` needs the repository mutably, which
    // the commit handles below would not allow.
    let mut merge_heads = Vec::new();
    repo.mergehead_foreach(|id| {
        merge_heads.push(*id);
        true
    })
    .map_err(|e| format!("git merge: {e}"))?;

    let mut index = repo.index().map_err(|e| format!("git index: {e}"))?;
    let conflicted = conflict_paths(&index)?;
    if !conflicted.is_empty() {
        return Err(format!(
            "cannot finish the merge: unresolved conflicts in {}",
            conflicted.join(", ")
        ));
    }
    let tree_id = index
        .write_tree_to(&repo)
        .map_err(|e| format!("git index: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("git tree: {e}"))?;
    let head = repo
        .head()
        .map_err(|e| format!("git merge: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("git merge: {e}"))?;
    let mut parents = vec![head];
    for id in &merge_heads {
        parents.push(
            repo.find_commit(*id)
                .map_err(|e| format!("git merge: {e}"))?,
        );
    }
    let message = repo
        .message()
        .ok()
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| {
            let names: Vec<String> = merge_heads.iter().map(|id| short(*id)).collect();
            format!("Merge {}", names.join(", "))
        });
    let signature = repo
        .signature()
        .map_err(|e| format!("git identity: {e} (set user.name and user.email)"))?;
    let parents: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &parents,
    )
    .map_err(|e| format!("git commit: {e}"))?;
    repo.cleanup_state()
        .map_err(|e| format!("git merge: {e}"))?;
    Ok(())
}

/// Abort the merge in progress: put the branch back as it was before [`merge`]
/// touched anything.
///
/// HEAD never moved during the merge (only the index and working tree did), so
/// resetting hard to HEAD restores the pre-merge files, markers included, and
/// clearing the state removes `MERGE_HEAD`/`MERGE_MSG`.
pub fn merge_abort(repo: &Path) -> Result<(), String> {
    let repo = open(repo)?;
    if repo.state() != RepositoryState::Merge {
        return Err("no merge in progress: there is nothing to abort".to_string());
    }
    let head = repo
        .head()
        .map_err(|e| format!("git merge --abort: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("git merge --abort: {e}"))?;
    repo.reset(head.as_object(), ResetType::Hard, None)
        .map_err(|e| format!("git merge --abort: {e}"))?;
    repo.cleanup_state()
        .map_err(|e| format!("git merge --abort: {e}"))?;
    Ok(())
}

/// Undo one commit with a new commit, and report the new commit's id.
///
/// This is `git revert`: a ledger's history is the books, so the undo is
/// recorded rather than erased. The revert is computed against the index first,
/// so later commits that changed the same lines are reported as a conflict by
/// name and nothing in the working tree or repository moves.
pub fn revert_commit(repo: &Path, id: &str) -> Result<String, String> {
    let repo = open(repo)?;
    if repo.state() != RepositoryState::Clean {
        return Err(
            "another git operation is in progress: finish it or abort it first".to_string(),
        );
    }
    let target = find_commit(&repo, id)?;
    let target_short = short(target.id());
    if target.parent_count() > 1 {
        // A merge commit has no single set of changes to undo; git asks for a
        // mainline, and picking one silently would book a decision the user
        // never made.
        return Err(format!(
            "cannot revert {target_short}: it is a merge commit; undo the change on the branch that made it"
        ));
    }
    guard_clean(&repo, "revert")?;
    let head = repo
        .head()
        .map_err(|e| format!("git revert: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("git revert: {e}"))?;

    // The revert is computed as an index without touching the working tree:
    // a conflicting revert is refused before any file is written.
    let mut index = repo
        .revert_commit(&target, &head, 0, None)
        .map_err(|e| format!("cannot revert {target_short}: {e}"))?;
    let conflicted = conflict_paths(&index)?;
    if !conflicted.is_empty() {
        return Err(format!(
            "cannot revert {target_short}: later changes to {} conflict with the undo",
            conflicted.join(", ")
        ));
    }
    let tree_id = index
        .write_tree_to(&repo)
        .map_err(|e| format!("git index: {e}"))?;
    if tree_id == head.tree_id() {
        return Err(format!(
            "nothing to revert: {target_short} is already undone"
        ));
    }
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("git tree: {e}"))?;
    let collisions = untracked_collisions(&repo, &tree)?;
    if !collisions.is_empty() {
        return Err(overwritten_error("revert", &collisions));
    }
    let summary = target.summary().ok().flatten().unwrap_or_default();
    let message = if summary.is_empty() {
        format!("Revert commit {}", target.id())
    } else {
        format!(
            "Revert \"{summary}\"\n\nThis reverts commit {}.",
            target.id()
        )
    };
    let signature = repo
        .signature()
        .map_err(|e| format!("git identity: {e} (set user.name and user.email)"))?;
    let reverted = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            &[&head],
        )
        .map_err(|e| format!("git commit: {e}"))?;
    // The revert commit exists now; bring the working tree and index onto it.
    // The tree was guarded clean and no untracked file sits on a changed path,
    // so the only bytes replaced are the ones the revert undoes.
    let commit = repo
        .find_commit(reverted)
        .map_err(|e| format!("git revert: {e}"))?;
    repo.reset(commit.as_object(), ResetType::Hard, None)
        .map_err(|e| format!("git revert: {e}"))?;
    Ok(reverted.to_string())
}

/// Open the repository at `path`, the ledger's working directory.
///
/// Deliberately an open, not a discover: searching upwards would let a ledger
/// that sits in a plain folder under, say, a repository at `$HOME` silently
/// operate on that unrelated repository. The caller passes the root the
/// settings view already shows.
fn open(path: &Path) -> Result<Repository, String> {
    Repository::open(path)
        .map_err(|e| format!("cannot open the repository at {}: {e}", path.display()))
}

/// The working directory git2 resolves paths against.
fn repo_workdir<'r>(repo: &'r Repository) -> Result<&'r Path, String> {
    repo.workdir()
        .ok_or_else(|| "the repository has no working tree".to_string())
}

/// A branch name the app is allowed to create.
fn branch_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a branch name must not be empty".to_string());
    }
    if name.starts_with('-') {
        return Err(format!("`{name}` is not a valid branch name"));
    }
    if !git2::Reference::is_valid_name(&format!("refs/heads/{name}")) {
        return Err(format!("`{name}` is not a valid branch name"));
    }
    Ok(name.to_string())
}

/// The short name of the branch HEAD is on, or `None` while HEAD is detached
/// or the repository has no commit.
fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    head.shorthand().ok().map(str::to_string)
}

/// HEAD's commit, or `None` in a repository whose first commit is still to come.
fn head_commit<'r>(repo: &'r Repository) -> Option<git2::Commit<'r>> {
    repo.head().ok()?.peel_to_commit().ok()
}

/// The commit's first parent's tree, or `None` for the root commit, whose whole
/// tree is its change.
fn parent_tree<'r>(commit: &git2::Commit<'r>) -> Result<Option<git2::Tree<'r>>, String> {
    if commit.parent_count() == 0 {
        return Ok(None);
    }
    let parent = commit.parent(0).map_err(|e| format!("git commit: {e}"))?;
    Ok(Some(parent.tree().map_err(|e| format!("git tree: {e}"))?))
}

/// How many files the commit changed — all of them, or only those under `path`.
fn changed_files(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    path: Option<&Path>,
) -> Result<usize, String> {
    let tree = commit.tree().map_err(|e| format!("git tree: {e}"))?;
    let parent_tree = parent_tree(commit)?;
    let mut options = DiffOptions::new();
    if let Some(path) = path {
        options.pathspec(path);
    }
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))
        .map_err(|e| format!("git diff: {e}"))?;
    Ok(diff.deltas().len())
}

/// Parse `id` as a commit — a full or short hash, a branch, a tag.
fn find_commit<'r>(repo: &'r Repository, id: &str) -> Result<git2::Commit<'r>, String> {
    let object = repo
        .revparse_single(id)
        .map_err(|_| format!("no branch, tag or commit named `{id}`"))?;
    object
        .peel_to_commit()
        .map_err(|_| format!("`{id}` is not a commit"))
}

/// `revision` as an annotated commit, the form the merge API takes, with a
/// message that says what was looked for rather than "object not found".
fn annotated_commit<'r>(
    repo: &'r Repository,
    revision: &str,
) -> Result<git2::AnnotatedCommit<'r>, String> {
    let commit = find_commit(repo, revision)?;
    repo.find_annotated_commit(commit.id())
        .map_err(|e| format!("git merge: {e}"))
}

/// Move HEAD to a local branch and put its tree in the working directory.
///
/// The checkout comes first and `set_head` second, so a refused checkout leaves
/// the user on the branch they were on instead of a moved pointer over an old
/// working tree.
fn switch(repo: &Repository, name: &str) -> Result<(), String> {
    let branch = repo
        .find_branch(name, BranchType::Local)
        .map_err(|_| format!("no local branch named {name}"))?;
    let target = branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("branch {name}: {e}"))?;
    let tree = target.tree().map_err(|e| format!("branch {name}: {e}"))?;
    let collisions = untracked_collisions(repo, &tree)?;
    if !collisions.is_empty() {
        return Err(overwritten_error("switch branches", &collisions));
    }
    let mut options = CheckoutBuilder::new();
    options.safe();
    repo.checkout_tree(target.as_object(), Some(&mut options))
        .map_err(|e| format!("cannot switch to {name}: {e}"))?;
    repo.set_head(&format!("refs/heads/{name}"))
        .map_err(|e| format!("cannot switch to {name}: {e}"))?;
    Ok(())
}

/// The repository-relative form of `path`.
///
/// Relative paths are taken as they are (git2's own convention); an absolute
/// one — what the app's file views hold — is resolved through its directory,
/// because the file itself may be gone when a deletion is staged.
fn relative(repo: &Repository, path: &Path) -> Result<PathBuf, String> {
    if path.is_relative() {
        return Ok(path.to_path_buf());
    }
    let workdir = repo_workdir(repo)?;
    let workdir = fs::canonicalize(workdir)
        .map_err(|e| format!("cannot resolve {}: {e}", workdir.display()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(path);
    let parent = fs::canonicalize(parent)
        .map_err(|e| format!("cannot resolve {}: {e}", parent.display()))?;
    let absolute = match path.file_name() {
        Some(name) => parent.join(name),
        None => parent,
    };
    absolute
        .strip_prefix(&workdir)
        .map(Path::to_path_buf)
        .map_err(|_| {
            format!(
                "{} is outside the repository at {}",
                path.display(),
                workdir.display()
            )
        })
}

/// The tracked paths git reports as changed — staged or not, deleted included.
/// Untracked files are not in it: a checkout only has to refuse those when it
/// would write the same path, which [`untracked_collisions`] answers.
fn dirty_paths(repo: &Repository) -> Result<Vec<PathBuf>, String> {
    let mut options = git2::StatusOptions::new();
    options.include_untracked(false).include_ignored(false);
    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|e| format!("git status: {e}"))?;
    let mut paths = Vec::new();
    for entry in statuses.iter() {
        if entry.status().is_empty() {
            continue;
        }
        if let Ok(path) = entry.path() {
            paths.push(PathBuf::from(path));
        }
    }
    paths.sort();
    Ok(paths)
}

/// The untracked files in the working tree that `tree` also has, and would
/// therefore write over.
fn untracked_collisions(repo: &Repository, tree: &git2::Tree<'_>) -> Result<Vec<PathBuf>, String> {
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|e| format!("git status: {e}"))?;
    let mut paths = Vec::new();
    for entry in statuses.iter() {
        if !entry.status().contains(git2::Status::WT_NEW) {
            continue;
        }
        let Ok(path) = entry.path() else {
            continue;
        };
        if tree.get_path(Path::new(path)).is_ok() {
            paths.push(PathBuf::from(path));
        }
    }
    paths.sort();
    Ok(paths)
}

/// Refuse an operation while the working tree has tracked changes.
fn guard_clean(repo: &Repository, action: &str) -> Result<(), String> {
    let dirty = dirty_paths(repo)?;
    if dirty.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "cannot {action}: {} {} uncommitted changes",
            listing(&dirty),
            if dirty.len() == 1 { "has" } else { "have" }
        ))
    }
}

/// Refuse an operation that would write over untracked files.
fn overwritten_error(action: &str, paths: &[PathBuf]) -> String {
    format!(
        "cannot {action}: {} {} untracked and would be overwritten",
        listing(paths),
        if paths.len() == 1 { "is" } else { "are" }
    )
}

/// Up to five names, plus a count for the rest, so a message stays a message.
fn listing(paths: &[PathBuf]) -> String {
    let names: Vec<String> = paths
        .iter()
        .take(5)
        .map(|path| path.display().to_string())
        .collect();
    let mut listed = names.join(", ");
    if paths.len() > names.len() {
        listed.push_str(&format!(" and {} more", paths.len() - names.len()));
    }
    listed
}

/// The conflicted paths of an index, sorted by name.
fn conflict_paths(index: &git2::Index) -> Result<Vec<String>, String> {
    if !index.has_conflicts() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for conflict in index.conflicts().map_err(|e| format!("git index: {e}"))? {
        let conflict = conflict.map_err(|e| format!("git index: {e}"))?;
        let entry = conflict.our.or(conflict.their).or(conflict.ancestor);
        if let Some(entry) = entry {
            paths.push(String::from_utf8_lossy(&entry.path).into_owned());
        }
    }
    paths.sort();
    Ok(paths)
}

/// The conflicts the repository's index holds, classified by which stages are
/// present.
fn conflicts(repo: &Repository) -> Result<Vec<ConflictFile>, String> {
    let index = repo.index().map_err(|e| format!("git index: {e}"))?;
    if !index.has_conflicts() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for conflict in index.conflicts().map_err(|e| format!("git index: {e}"))? {
        let conflict = conflict.map_err(|e| format!("git index: {e}"))?;
        let base = conflict.ancestor.is_some();
        let ours = conflict.our.is_some();
        let theirs = conflict.their.is_some();
        let path = conflict
            .our
            .as_ref()
            .or(conflict.their.as_ref())
            .or(conflict.ancestor.as_ref())
            .map(|entry| String::from_utf8_lossy(&entry.path).into_owned())
            .unwrap_or_default();
        // The four kinds the UI has editors for; the remaining stage
        // combinations only arise from an add/rename on one side.
        let kind = match (base, ours, theirs) {
            (true, true, true) => "both_modified",
            (false, true, true) => "both_added",
            (true, false, true) => "deleted_by_us",
            (true, true, false) => "deleted_by_them",
            (false, true, false) => "added_by_us",
            (false, false, true) => "added_by_them",
            (true, false, false) => "deleted_by_both",
            (false, false, false) => "unclassified",
        };
        files.push(ConflictFile {
            path,
            kind: kind.to_string(),
            stages: u8::from(base) + u8::from(ours) + u8::from(theirs),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Render a diff as `=== path ===` blocks of patch text, the shape the ledger's
/// diff view already uses for HEAD (see `git::render_diff`) — git's own hunk
/// headers and lines, each prefixed by its marker.
fn render_patch(diff: &git2::Diff<'_>) -> Result<String, String> {
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

/// The first seven characters of an object id, the width git itself uses.
fn short(id: git2::Oid) -> String {
    id.to_string().chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A throwaway directory with a repository in it, removed on drop.
    ///
    /// This is a *repository* fixture on purpose, not the crate's
    /// `testing::TempDir`: the tests here need `init_opts` with an initial
    /// branch, an identity and the commit/read helpers below, none of which a
    /// plain directory of files carries. Kept local so this module tests
    /// itself and nothing else.
    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "ledger-app-git-ops-{name}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        /// A repository whose HEAD starts on `main`, with the identity commits
        /// need.
        fn init(&self) -> Repository {
            let mut options = git2::RepositoryInitOptions::new();
            options.initial_head("main");
            let repo = Repository::init_opts(&self.path, &mut options).unwrap();
            {
                let mut config = repo.config().unwrap();
                config.set_str("user.name", "Test Ledger").unwrap();
                config.set_str("user.email", "ledger@example.com").unwrap();
            }
            repo
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn file(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }

        fn write(&self, name: &str, text: &str) {
            let path = self.file(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, text).unwrap();
        }

        fn read(&self, name: &str) -> String {
            fs::read_to_string(self.file(name)).unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// A handle that reads the index and refs from disk again: the module's
    /// calls open their own repository, and libgit2 caches the index per
    /// handle, so a verification through the fixture's handle could see the
    /// state from before the call.
    fn fresh(repo: &Repository) -> Repository {
        Repository::open(repo.workdir().unwrap()).unwrap()
    }

    /// Stage everything and commit it at `time`, so the history's order is the
    /// test's choice rather than the clock's.
    fn commit_all(repo: &Repository, message: &str, time: i64) -> git2::Oid {
        let repo = &fresh(repo);
        let mut index = repo.index().unwrap();
        // `add_all` stages new and modified files; `update_all` records the
        // deletions `add_all` leaves alone.
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.update_all(["*"].iter(), None).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::new(
            "Test Ledger",
            "ledger@example.com",
            &git2::Time::new(time, 0),
        )
        .unwrap();
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .unwrap()
    }

    fn head(repo: &Repository) -> git2::Oid {
        fresh(repo).head().unwrap().peel_to_commit().unwrap().id()
    }

    fn index_text(repo: &Repository, name: &str) -> Option<String> {
        let repo = fresh(repo);
        let index = repo.index().unwrap();
        let entry = index.get_path(Path::new(name), 0)?;
        let blob = repo.find_blob(entry.id).unwrap();
        Some(String::from_utf8(blob.content().to_vec()).unwrap())
    }

    const JOURNAL: &str = "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
";

    #[test]
    fn branches_switch_and_refuse_a_dirty_tree() {
        let dir = TempRepo::new("branches");
        let repo = dir.init();
        dir.write("main.bean", JOURNAL);
        dir.write("books/2026.bean", "2026-01-01 open Assets:Bank:Checking\n");
        commit_all(&repo, "initial", 1000);

        let list = branches(dir.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "main");
        assert!(list[0].is_current);
        assert!(!list[0].is_remote);
        assert_eq!(list[0].upstream, None);

        create_branch(dir.path(), "feature", true).unwrap();
        let list = branches(dir.path()).unwrap();
        assert_eq!(list[0].name, "feature");
        assert!(list[0].is_current);
        assert!(!list.iter().find(|b| b.name == "main").unwrap().is_current);

        checkout(dir.path(), "main").unwrap();
        assert_eq!(current_branch(&repo).as_deref(), Some("main"));

        // A modified tracked file blocks the switch, and the message names it.
        dir.write("books/2026.bean", "2026-01-02 open Expenses:Food\n");
        let refused = checkout(dir.path(), "feature").unwrap_err();
        assert!(
            refused.contains("books/2026.bean") && refused.contains("uncommitted changes"),
            "{refused}"
        );
        assert_eq!(current_branch(&repo).as_deref(), Some("main"));
        assert_eq!(
            dir.read("books/2026.bean"),
            "2026-01-02 open Expenses:Food\n"
        );

        // The same guard keeps a branch from being created and switched to.
        assert!(create_branch(dir.path(), "other", true).is_err());
        assert!(repo.find_branch("other", BranchType::Local).is_err());

        // A clean tree switches, with the branch's files in place.
        discard(dir.path(), &[dir.file("books/2026.bean")]).unwrap();
        checkout(dir.path(), "feature").unwrap();
        assert_eq!(current_branch(&repo).as_deref(), Some("feature"));
        assert!(repo.find_branch("feature", BranchType::Local).is_ok());
        assert_eq!(
            dir.read("books/2026.bean"),
            "2026-01-01 open Assets:Bank:Checking\n"
        );
    }

    #[test]
    fn checkout_refuses_to_clobber_an_untracked_file() {
        let dir = TempRepo::new("untracked");
        let repo = dir.init();
        dir.write("main.bean", "committed\n");
        commit_all(&repo, "initial", 1000);

        create_branch(dir.path(), "feature", true).unwrap();
        dir.write("extra.bean", "on the feature branch\n");
        commit_all(&repo, "feature adds a file", 2000);
        checkout(dir.path(), "main").unwrap();
        assert!(!dir.file("extra.bean").exists());

        // The same path exists untracked here; switching would overwrite it.
        dir.write("extra.bean", "written on main\n");
        let refused = checkout(dir.path(), "feature").unwrap_err();
        assert!(
            refused.contains("extra.bean") && refused.contains("untracked"),
            "{refused}"
        );
        assert_eq!(dir.read("extra.bean"), "written on main\n");
        assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    }

    #[test]
    fn log_is_newest_first_and_filters_by_path() {
        let dir = TempRepo::new("log");
        let repo = dir.init();
        dir.write("main.bean", "main v1\n");
        dir.write("books/2026.bean", "books v1\n");
        commit_all(&repo, "initial", 1000);
        dir.write("books/2026.bean", "books v2\n");
        let books = commit_all(&repo, "books edit", 2000);
        dir.write("main.bean", "main v2\n");
        let main = commit_all(&repo, "main edit", 3000);

        let all = log(dir.path(), 2, None).unwrap();
        assert_eq!(
            all.iter().map(|c| c.summary.as_str()).collect::<Vec<_>>(),
            ["main edit", "books edit"]
        );
        assert_eq!(all[0].id, main.to_string());
        assert_eq!(all[0].short, main.to_string()[..7]);
        assert_eq!(all[0].author, "Test Ledger");
        assert_eq!(all[0].email, "ledger@example.com");
        assert_eq!(all[0].time, 3000);
        assert_eq!(all[0].files, 1);

        let main_only = log(dir.path(), 10, Some(&dir.file("main.bean"))).unwrap();
        assert_eq!(
            main_only
                .iter()
                .map(|c| c.summary.as_str())
                .collect::<Vec<_>>(),
            ["main edit", "initial"]
        );

        // A repository-relative path works as well as an absolute one.
        let books_only = log(dir.path(), 10, Some(Path::new("books/2026.bean"))).unwrap();
        assert_eq!(
            books_only
                .iter()
                .map(|c| c.summary.as_str())
                .collect::<Vec<_>>(),
            ["books edit", "initial"]
        );

        assert!(log(dir.path(), 0, None).unwrap().is_empty());

        let patch = commit_diff(dir.path(), &books.to_string()).unwrap();
        assert!(patch.contains("=== books/2026.bean ===\n"), "{patch}");
        assert!(patch.contains("-books v1\n"), "{patch}");
        assert!(patch.contains("+books v2\n"), "{patch}");
    }

    #[test]
    fn stage_unstage_and_discard() {
        let dir = TempRepo::new("stage");
        let repo = dir.init();
        dir.write("main.bean", "committed\n");
        commit_all(&repo, "initial", 1000);
        let path = dir.file("main.bean");

        dir.write("main.bean", "edited\n");
        stage(dir.path(), std::slice::from_ref(&path)).unwrap();
        assert_eq!(index_text(&repo, "main.bean").as_deref(), Some("edited\n"));

        unstage(dir.path(), std::slice::from_ref(&path)).unwrap();
        assert_eq!(
            index_text(&repo, "main.bean").as_deref(),
            Some("committed\n")
        );
        assert_eq!(dir.read("main.bean"), "edited\n");

        // Discard restores the index's version and leaves the index alone.
        discard(dir.path(), std::slice::from_ref(&path)).unwrap();
        assert_eq!(dir.read("main.bean"), "committed\n");

        // An untracked file can be staged and unstaged, but discard leaves it
        // on disk: nothing else holds a copy of it.
        let notes = dir.file("notes.txt");
        dir.write("notes.txt", "shopping list\n");
        stage(dir.path(), std::slice::from_ref(&notes)).unwrap();
        assert_eq!(
            index_text(&repo, "notes.txt").as_deref(),
            Some("shopping list\n")
        );
        unstage(dir.path(), std::slice::from_ref(&notes)).unwrap();
        assert!(index_text(&repo, "notes.txt").is_none());
        discard(dir.path(), std::slice::from_ref(&notes)).unwrap();
        assert_eq!(dir.read("notes.txt"), "shopping list\n");

        // A deletion is staged as a removal, as `git add` does.
        fs::remove_file(&path).unwrap();
        stage(dir.path(), std::slice::from_ref(&path)).unwrap();
        assert!(index_text(&repo, "main.bean").is_none());
    }

    #[test]
    fn merge_fast_forwards_and_reports_up_to_date() {
        let dir = TempRepo::new("ff");
        let repo = dir.init();
        dir.write("main.bean", "committed\n");
        commit_all(&repo, "initial", 1000);

        create_branch(dir.path(), "feature", true).unwrap();
        dir.write("books/2026.bean", "opening balances\n");
        let feature = commit_all(&repo, "feature work", 2000);

        checkout(dir.path(), "main").unwrap();
        let outcome = merge(dir.path(), "feature").unwrap();
        assert_eq!(
            outcome,
            MergeOutcome::FastForward {
                new_head: feature.to_string()
            }
        );
        assert_eq!(head(&repo), feature);
        assert_eq!(dir.read("books/2026.bean"), "opening balances\n");

        assert_eq!(
            merge(dir.path(), "feature").unwrap(),
            MergeOutcome::UpToDate
        );
    }

    #[test]
    fn a_clean_merge_is_staged_and_finished_by_continue() {
        let dir = TempRepo::new("clean-merge");
        let repo = dir.init();
        dir.write("main.bean", "committed\n");
        commit_all(&repo, "initial", 1000);

        create_branch(dir.path(), "feature", true).unwrap();
        dir.write("books/2026.bean", "feature side\n");
        commit_all(&repo, "feature adds books", 2000);

        checkout(dir.path(), "main").unwrap();
        dir.write("prices.bean", "main side\n");
        commit_all(&repo, "main adds prices", 3000);

        // Two divergent branches, no overlapping lines: the merge is staged
        // with no conflict to show, and still waits for the merge commit.
        let outcome = merge(dir.path(), "feature").unwrap();
        assert_eq!(outcome, MergeOutcome::Conflicts(Vec::new()));
        merge_continue(dir.path()).unwrap();

        assert_eq!(repo.state(), RepositoryState::Clean);
        let fresh = fresh(&repo);
        let commit = fresh.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 2);
        assert_eq!(dir.read("books/2026.bean"), "feature side\n");
        assert_eq!(dir.read("prices.bean"), "main side\n");
    }

    #[test]
    fn merge_reports_conflicts_resolves_and_commits() {
        let dir = TempRepo::new("conflict");
        let repo = dir.init();
        dir.write("main.bean", JOURNAL);
        commit_all(&repo, "initial", 1000);

        create_branch(dir.path(), "feature", true).unwrap();
        dir.write("main.bean", &JOURNAL.replace("-20.00 USD", "-21.00 USD"));
        commit_all(&repo, "feature re-prices", 2000);

        checkout(dir.path(), "main").unwrap();
        let ours = JOURNAL.replace("-20.00 USD", "-22.00 USD");
        dir.write("main.bean", &ours);
        commit_all(&repo, "main re-prices", 3000);

        let outcome = merge(dir.path(), "feature").unwrap();
        let MergeOutcome::Conflicts(files) = outcome else {
            panic!("expected conflicts, got {outcome:?}");
        };
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].path, "main.bean");
        assert_eq!(files[0].kind, "both_modified");
        assert_eq!(files[0].stages, 3);

        let texts = conflict_texts(dir.path(), &dir.file("main.bean")).unwrap();
        assert!(texts.base.as_deref().unwrap().contains("-20.00 USD"));
        assert!(texts.ours.as_deref().unwrap().contains("-22.00 USD"));
        assert!(texts.theirs.as_deref().unwrap().contains("-21.00 USD"));

        // The caller merges the three texts itself, writes the answer through the
        // ledger's edit pipeline (the command layer's `edit_named_file_source`,
        // where a loader refusal rolls the bytes back) and stages it here.
        let merged = JOURNAL.replace("-20.00 USD", "-21.50 USD");
        fs::write(dir.file("main.bean"), &merged).unwrap();
        stage(dir.path(), &[dir.file("main.bean")]).unwrap();
        merge_continue(dir.path()).unwrap();

        assert_eq!(repo.state(), RepositoryState::Clean);
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 2);
        assert_eq!(dir.read("main.bean"), merged);
        assert!(!fresh(&repo).index().unwrap().has_conflicts());
        // No half-merge left behind: no MERGE_HEAD, no merge message.
        assert!(repo.message().is_err());
    }

    #[test]
    fn merge_abort_leaves_the_branch_as_it_was() {
        let dir = TempRepo::new("abort");
        let repo = dir.init();
        dir.write("main.bean", JOURNAL);
        commit_all(&repo, "initial", 1000);

        create_branch(dir.path(), "feature", true).unwrap();
        dir.write("main.bean", &JOURNAL.replace("-20.00 USD", "-21.00 USD"));
        commit_all(&repo, "feature re-prices", 2000);

        checkout(dir.path(), "main").unwrap();
        let ours = JOURNAL.replace("-20.00 USD", "-22.00 USD");
        dir.write("main.bean", &ours);
        let mine = commit_all(&repo, "main re-prices", 3000);

        assert!(matches!(
            merge(dir.path(), "feature").unwrap(),
            MergeOutcome::Conflicts(files) if !files.is_empty()
        ));
        // The working tree really carries conflict markers before the abort.
        assert!(dir.read("main.bean").contains("<<<<<<<"));

        merge_abort(dir.path()).unwrap();
        assert_eq!(head(&repo), mine);
        assert_eq!(repo.state(), RepositoryState::Clean);
        assert_eq!(dir.read("main.bean"), ours);
        assert!(!fresh(&repo).index().unwrap().has_conflicts());
    }

    #[test]
    fn revert_commits_the_undo_and_refuses_a_conflicting_one() {
        let dir = TempRepo::new("revert");
        let repo = dir.init();
        dir.write("main.bean", "2026-01-10 * \"old\"\n");
        commit_all(&repo, "initial", 1000);
        dir.write("main.bean", "2026-01-10 * \"new\"\n");
        let change = commit_all(&repo, "rename the payee", 2000);

        let reverted = revert_commit(dir.path(), &change.to_string()).unwrap();
        assert_eq!(dir.read("main.bean"), "2026-01-10 * \"old\"\n");
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.id().to_string(), reverted);
        assert_eq!(commit.parent_count(), 1);
        assert!(
            commit
                .message()
                .unwrap()
                .contains("Revert \"rename the payee\""),
            "{:?}",
            commit.message()
        );
        assert_eq!(repo.state(), RepositoryState::Clean);

        // A later commit on the same line makes the undo conflict, and the
        // refusal leaves the repository untouched.
        dir.write("main.bean", "2026-01-10 * \"later\"\n");
        commit_all(&repo, "later edit", 3000);
        let before = head(&repo);
        let refused = revert_commit(dir.path(), &change.to_string()).unwrap_err();
        assert!(refused.contains("cannot revert"), "{refused}");
        assert!(refused.contains("main.bean"), "{refused}");
        assert_eq!(head(&repo), before);
        assert_eq!(dir.read("main.bean"), "2026-01-10 * \"later\"\n");
        assert_eq!(repo.state(), RepositoryState::Clean);
    }
}
