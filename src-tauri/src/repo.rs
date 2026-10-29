// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ledger directory's git repository: whether there is one, where it starts,
//! the identity a commit would be attributed to, and the two writes the settings
//! view needs — creating the repository and setting that identity.
//!
//! Only the local storage kind has a repository. A ledger in an Android SAF tree
//! lives in a private mirror (see [`crate::storage`]), a copy the app writes back
//! on save; a repository there would record the mirror's history and be
//! meaningless to the user, so [`state`] reports no repository and the writes
//! refuse.

use std::path::Path;

use crate::storage::{StorageKind, kind};

/// What the settings view shows about the ledger's repository: whether it exists,
/// where it starts, and the identity commits would be attributed to.
///
/// `repo: false` covers both "this directory is not under version control" and
/// "git is not available for this storage" — the settings view offers to create a
/// repository in the first case, and nothing at all in the second.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct RepoState {
    pub repo: bool,
    /// The repository's working-tree root, as a path string; `None` when there
    /// is no repository.
    pub root: Option<String>,
    /// `user.name` from the repository's own config, when it has one.
    pub user_name: Option<String>,
    /// `user.email` from the repository's own config, when it has one.
    pub user_email: Option<String>,
}

/// The body of [`git_state`](crate::commands::git_state): the ledger directory's
/// repository as it stands now.
pub fn state(ledger_dir: &Path) -> RepoState {
    state_in(kind(), ledger_dir)
}

fn state_in(storage: StorageKind, ledger_dir: &Path) -> RepoState {
    let none = RepoState {
        repo: false,
        root: None,
        user_name: None,
        user_email: None,
    };
    // A mirrored SAF tree has no meaningful repository; saying so here keeps
    // every view from having to know about Android.
    if storage == StorageKind::Tree {
        return none;
    }
    let Ok(repo) = git2::Repository::discover(ledger_dir) else {
        return none;
    };
    let config = local_config(&repo);
    RepoState {
        repo: true,
        root: repo.workdir().map(workdir_string),
        user_name: config
            .as_ref()
            .and_then(|config| config.get_string("user.name").ok()),
        user_email: config
            .as_ref()
            .and_then(|config| config.get_string("user.email").ok()),
    }
}

/// The working tree's root as a path string. libgit2 hands back a trailing
/// separator; the settings view wants a plain path to show (and to hand back to
/// the platform), so it is trimmed — except for a repository rooted at `/`, where
/// the separator is the whole path.
fn workdir_string(root: &Path) -> String {
    let text = root.to_string_lossy();
    match text.strip_suffix(std::path::MAIN_SEPARATOR) {
        Some(trimmed) if !trimmed.is_empty() => trimmed.to_owned(),
        _ => text.into_owned(),
    }
}

/// The body of [`git_init`](crate::commands::git_init): create a repository in
/// the ledger's directory.
///
/// Idempotent, and never makes a commit: a ledger that is already inside a
/// repository — this directory or one above it — is left alone instead of
/// growing a nested repository whose diff and commit paths would confuse the
/// one the user already has.
pub fn init(ledger_dir: &Path) -> Result<(), String> {
    init_in(kind(), ledger_dir)
}

fn init_in(storage: StorageKind, ledger_dir: &Path) -> Result<(), String> {
    if storage == StorageKind::Tree {
        return Err(tree_refusal());
    }
    if git2::Repository::discover(ledger_dir).is_ok() {
        return Ok(());
    }
    git2::Repository::init(ledger_dir)
        .map(|_| ())
        .map_err(|e| format!("git init {}: {e}", ledger_dir.display()))
}

/// The body of [`git_set_identity`](crate::commands::git_set_identity): write
/// `user.name`/`user.email` into the repository's own config.
///
/// The repository config is the right home for this: the app must not need a
/// global git setup, and the identity belongs with the ledger it commits.
pub fn set_identity(ledger_dir: &Path, name: &str, email: &str) -> Result<(), String> {
    set_identity_in(kind(), ledger_dir, name, email)
}

fn set_identity_in(
    storage: StorageKind,
    ledger_dir: &Path,
    name: &str,
    email: &str,
) -> Result<(), String> {
    if storage == StorageKind::Tree {
        return Err(tree_refusal());
    }
    let repo = git2::Repository::discover(ledger_dir).map_err(|_| {
        format!(
            "{} is not in a git repository — initialise one first",
            ledger_dir.display()
        )
    })?;
    // `Repository::config` writes to the repository's own file (the highest
    // priority level), which is exactly where the identity should live.
    let mut config = repo.config().map_err(|e| format!("git config: {e}"))?;
    config
        .set_str("user.name", name)
        .map_err(|e| format!("set user.name: {e}"))?;
    config
        .set_str("user.email", email)
        .map_err(|e| format!("set user.email: {e}"))?;
    Ok(())
}

/// The repository's own config file, not the global/system layers that
/// `Repository::config` would stack under it: the identity shown must be the one
/// this repository commits with, and a machine-wide `user.name` is not a fact
/// about this ledger.
fn local_config(repo: &git2::Repository) -> Option<git2::Config> {
    // `commondir` is the git directory itself for an ordinary repository and the
    // shared one for a linked worktree, whose own directory has no config.
    git2::Config::open(&repo.commondir().join("config")).ok()
}

fn tree_refusal() -> String {
    "the ledger is stored in a SAF tree; git is not available there".to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{RepoState, init, init_in, set_identity, set_identity_in, state, state_in};
    use crate::storage::StorageKind;
    use crate::testing::TempDir;

    /// A repository in `dir` with no identity and no commits — what a fresh
    /// `git init` leaves behind.
    fn empty_repo(dir: &TempDir) {
        init(dir.dir()).unwrap();
    }

    #[test]
    fn a_plain_directory_is_not_a_repository() {
        let dir = TempDir::new("repo-plain");

        assert_eq!(
            state(dir.dir()),
            RepoState {
                repo: false,
                root: None,
                user_name: None,
                user_email: None,
            }
        );
    }

    #[test]
    fn init_creates_a_repository_without_a_commit_or_identity() {
        let dir = TempDir::new("repo-init");

        init(dir.dir()).unwrap();

        let state = state(dir.dir());
        assert!(state.repo);
        let root = state.root.as_deref().unwrap();
        assert_eq!(
            fs::canonicalize(Path::new(root)).unwrap(),
            fs::canonicalize(dir.dir()).unwrap()
        );
        // A path to show, not libgit2's trailing separator.
        assert!(!root.ends_with(std::path::MAIN_SEPARATOR), "{root}");
        // The repository's own config has no identity yet — and on a machine
        // whose global config has one (most development machines do), reading
        // it must still not show up here: this is the repository's identity,
        // not the machine's.
        assert_eq!(state.user_name, None);
        assert_eq!(state.user_email, None);
        // ...and nothing was committed on the way in.
        assert!(git2::Repository::open(dir.dir()).unwrap().head().is_err());
    }

    #[test]
    fn init_is_idempotent() {
        let dir = TempDir::new("repo-init-twice");
        init(dir.dir()).unwrap();
        let root = state(dir.dir()).root;

        init(dir.dir()).unwrap();

        assert_eq!(state(dir.dir()).root, root);
    }

    #[test]
    fn init_inside_an_existing_repository_adds_no_nested_one() {
        let dir = TempDir::new("repo-init-nested");
        empty_repo(&dir);
        fs::create_dir(dir.file("beancount")).unwrap();

        init(&dir.file("beancount")).unwrap();

        assert!(!dir.file("beancount/.git").exists());
        assert_eq!(state(&dir.file("beancount")).root, state(dir.dir()).root);
    }

    #[test]
    fn state_finds_the_repository_above_the_ledger_directory() {
        let dir = TempDir::new("repo-above");
        empty_repo(&dir);
        fs::create_dir(dir.file("beancount")).unwrap();

        let nested = state(&dir.file("beancount"));

        assert!(nested.repo);
        assert_eq!(nested.root, state(dir.dir()).root);
    }

    #[test]
    fn set_identity_without_a_repository_is_refused() {
        let dir = TempDir::new("repo-no-repo-identity");

        let error = set_identity(dir.dir(), "Alper", "alper@example.com").unwrap_err();

        assert!(error.contains("git repository"), "{error}");
    }

    #[test]
    fn identity_round_trips_through_the_repository_config() {
        let dir = TempDir::new("repo-identity");
        empty_repo(&dir);

        set_identity(dir.dir(), "Alper Çelik", "alper@alper-celik.dev").unwrap();

        let state = state(dir.dir());
        assert_eq!(state.user_name.as_deref(), Some("Alper Çelik"));
        assert_eq!(state.user_email.as_deref(), Some("alper@alper-celik.dev"));
        // The value went into the repository's own config file, which is what
        // makes it travel with the ledger rather than with this machine.
        let config = fs::read_to_string(dir.file(".git/config")).unwrap();
        assert!(config.contains("Alper Çelik"), "{config}");
        assert!(config.contains("alper@alper-celik.dev"), "{config}");
    }

    #[test]
    fn tree_storage_has_no_git_even_inside_a_repository() {
        let dir = TempDir::new("repo-tree-mode");
        empty_repo(&dir);
        set_identity(dir.dir(), "Alper", "alper@example.com").unwrap();
        assert!(state(dir.dir()).repo);

        let tree_state = state_in(StorageKind::Tree, dir.dir());

        assert_eq!(
            tree_state,
            RepoState {
                repo: false,
                root: None,
                user_name: None,
                user_email: None,
            }
        );
        assert!(init_in(StorageKind::Tree, dir.dir()).is_err());
        assert!(set_identity_in(StorageKind::Tree, dir.dir(), "A", "b").is_err());
    }
}
