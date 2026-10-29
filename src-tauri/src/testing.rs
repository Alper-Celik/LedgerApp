// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fixtures shared by the module tests: temporary directories, throwaway
//! repositories, and the same `AppState` the app builds.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::accounts::account_transaction_views;
use crate::model::{PostingEdit, TransactionEdit, TransactionView};
use crate::session::{AppState, load_baselined};

/// A throwaway directory for one test's files — the ledger, its includes, or
/// whatever a file tool is pointed at — removed when the fixture drops.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// A directory of its own: the process id plus a counter keeps tests
    /// running at the same time out of each other's way.
    pub(crate) fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "ledger-app-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Write `text` to `name` under the directory, creating any parent
    /// directories, and hand back the path.
    ///
    /// Every test that exercises a file tool starts the same way — a throwaway
    /// directory and a file or two in it — and each such module had grown its
    /// own `scratch`/`seed` pair to do it. The write lives here once so those
    /// fixtures only say which file holds what.
    pub(crate) fn seed(&self, name: &str, text: &str) -> PathBuf {
        let path = self.file(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, text).unwrap();
        path
    }

    /// The directory itself, for the tests that hand a folder to the loader.
    pub(crate) fn dir(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A repository in `dir` whose first commit holds `tracked` — the files the
/// caller has already written. Identity is set, as `commit_ledger` needs it.
pub(crate) fn repo_with_commit(dir: &TempDir, tracked: &[&str]) -> git2::Repository {
    let repo = git2::Repository::init(&dir.path).unwrap();
    {
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test Ledger").unwrap();
        config.set_str("user.email", "ledger@example.com").unwrap();
    }
    {
        let mut index = repo.index().unwrap();
        for name in tracked {
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap();
    }
    repo
}

/// Write the ledger fixture, put it under version control with one commit,
/// and hand back the repository and the ledger file's path.
pub(crate) fn committed_fixture(dir: &TempDir, ledger: &str) -> (git2::Repository, PathBuf) {
    let path = dir.file("main.bean");
    fs::write(&path, ledger).unwrap();
    let repo = repo_with_commit(dir, &["main.bean"]);
    (repo, path)
}

/// The app state around a freshly loaded copy of `path`, baseline included —
/// the same `load_baselined` the app itself uses. Every other field keeps the
/// state's own default, so a new field on `AppState` does not reach here.
pub(crate) fn loaded_state(path: &Path) -> AppState {
    let state = AppState::default();
    *state.loaded.write().unwrap_or_else(|e| e.into_inner()) = Some(load_baselined(path).unwrap());
    state
}

/// The `TransactionView.id` the frontend would be holding for `narration`,
/// taken from the same views it reads.
pub(crate) fn view_id(state: &AppState, narration: &str) -> u32 {
    views(state)
        .into_iter()
        .find(|view| view.narration == narration)
        .unwrap_or_else(|| panic!("no transaction narrated `{narration}`"))
        .id
}

/// The narrations of the transactions the register would mark as modified.
pub(crate) fn modified_narrations(state: &AppState) -> Vec<String> {
    views(state)
        .into_iter()
        .filter(|view| view.modified)
        .map(|view| view.narration)
        .collect()
}

/// Every transaction of the held ledger, as the register sees them.
pub(crate) fn views(state: &AppState) -> Vec<TransactionView> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().unwrap();
    account_transaction_views(&loaded.ledger, &loaded.baseline, "")
}

/// A units edit for the posting at `index`.
pub(crate) fn posting(index: u32, units: &str) -> PostingEdit {
    PostingEdit {
        index,
        units: units.to_string(),
    }
}

/// A transaction edit of the shape the register sends, for the tests that
/// rewrite the header as little as possible.
pub(crate) fn edit_of(
    id: u32,
    date: &str,
    narration: &str,
    postings: Vec<PostingEdit>,
) -> TransactionEdit {
    TransactionEdit {
        id,
        date: date.to_string(),
        flag: "*".to_string(),
        payee: None,
        narration: narration.to_string(),
        postings,
    }
}
