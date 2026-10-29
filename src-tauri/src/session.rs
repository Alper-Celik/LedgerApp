// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The loaded-ledger session: the ledger plus the baseline its `modified` flags
//! are measured against, and how the journal to open is found.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rustledger_loader::{Ledger, LoadOptions};

use crate::model::LoadSummary;

/// A loaded ledger together with the baseline the register's `modified` flags
/// are measured against.
pub(crate) struct Loaded {
    pub(crate) ledger: Ledger,
    /// The journal this session was opened with — the file the user pointed at,
    /// with its `include`s followed. Every edit reloads *this*, never the file
    /// it just wrote: an included file on its own carries none of the `open`
    /// directives that live in its parent, so reloading it alone reports every
    /// account it touches as never opened.
    pub(crate) root: PathBuf,
    /// Every file's text as this session first saw it, keyed by path. It stands
    /// in wherever git has no HEAD blob for the file: a ledger that is not
    /// committed yet — a fresh journal, a directory that was never added — is
    /// not a change in itself, only the edits made to it are.
    pub(crate) baseline: HashMap<PathBuf, Arc<str>>,
}

impl Loaded {
    /// Wrap a freshly loaded ledger taken from `root`, recording what its files
    /// look like now.
    pub(crate) fn fresh(ledger: Ledger, root: PathBuf) -> Self {
        let mut loaded = Self {
            ledger,
            root,
            baseline: HashMap::new(),
        };
        loaded.remember_files();
        loaded
    }

    /// Take in files this session has not seen before, keeping what earlier
    /// loads recorded so that an edit still reads as a change against its
    /// baseline.
    pub(crate) fn remember_files(&mut self) {
        for file in self.ledger.source_map.files() {
            self.baseline
                .entry(file.path.clone())
                .or_insert_with(|| Arc::clone(&file.source));
        }
    }
}

/// Load a ledger from disk — the `.bean` journal itself, or a directory holding
/// one — and take the session baseline for its files, so the app and a reload
/// after an edit agree on what "changed" means.
pub(crate) fn load_baselined(path: &Path) -> Result<Loaded, String> {
    let root = journal_in(path)?;
    load_journal(&root).map(|ledger| Loaded::fresh(ledger, root))
}

/// Load the journal at `root` with the app's loader options — the one place the
/// loader is configured, so a reload after an edit parses the file exactly as
/// opening it did.
pub(crate) fn load_journal(root: &Path) -> Result<Ledger, String> {
    // `path_security` is off by default in the loader, which means an `include`
    // could reach outside the journal's directory — a ledger is user data, but it
    // is also a file that arrives from elsewhere. Refuse traversal; a ledger that
    // genuinely shares files above its directory needs an explicit option, not a
    // silent hole.
    let options = LoadOptions {
        path_security: true,
        ..LoadOptions::default()
    };
    rustledger_loader::load(root, &options).map_err(|e| e.to_string())
}

/// The journal a path names: the file itself, or the root journal directly
/// inside a directory — the loader's own discovery order (`main.bean`,
/// `main.beancount`, `ledger.bean`, …). This is what makes opening a folder a
/// way to point the app at a ledger without naming its root file.
fn journal_in(path: &Path) -> Result<PathBuf, String> {
    // Every folder consumer takes the root's parent, and a bare `main.bean` has
    // an empty one: `canonicalize("")` is ENOENT (so every attachment fails) and
    // libgit2 cannot open "" (so the repository is invisible). Resolving here
    // fixes the directory, the summary the UI hands back, and cwd-dependence in
    // one place.
    let path = std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !path.is_dir() {
        return Ok(path);
    }
    rustledger_loader::discover_journal_file(&path)
        .ok_or_else(|| format!("no root journal (main.bean, …) in {}", path.display()))
}

/// The summary of a loaded ledger: its root journal and what the load found.
pub(crate) fn summary_of(loaded: &Loaded) -> LoadSummary {
    LoadSummary {
        path: loaded.root.display().to_string(),
        directive_count: u32::try_from(loaded.ledger.directives.len()).unwrap_or(u32::MAX),
        error_count: u32::try_from(loaded.ledger.errors.len()).unwrap_or(u32::MAX),
    }
}

/// The sentence a launch leaves behind when the folder it was pointed at could
/// not be reopened: what failed, and that the picker is the only way back — a
/// phone has no path to fall back on, and a storage grant can be revoked between
/// launches.
// The one caller is the phone's restore, which a desktop build does not have;
// the test drives the same sentence.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn reopen_failure(error: &str) -> String {
    format!(
        "The ledger folder could not be reopened: {error}. Pick it again with “Open folder…” — the app may have lost access to it."
    )
}

/// Shared app state: the currently loaded ledger, if any.
#[derive(Default)]
pub struct AppState {
    pub(crate) loaded: RwLock<Option<Loaded>>,
    /// The last thing that went wrong with the ledger's *folder* rather than
    /// with the ledger: it could not be reopened at launch, or a file the app
    /// wrote into its private copy never reached it. Kept here because these
    /// failures have no command to fail — a restore runs before any window
    /// exists, and a best-effort flush has already written what the reader
    /// asked for — so this is the only place the reader can be told. Cleared by
    /// the next write to the folder that works.
    pub(crate) storage_failure: RwLock<Option<String>>,
}

impl AppState {
    /// Open the ledger at startup: the last CLI argument if one was given,
    /// else `LEDGER_PATH` from the environment (set per-machine via a
    /// git-ignored `.envrc.local`), else `main.bean` in the working
    /// directory. A missing or broken file just leaves the app running with
    /// no ledger loaded.
    pub(crate) fn new() -> Self {
        let path = resolve_ledger_path();
        let loaded = match load_baselined(Path::new(&path)) {
            Ok(loaded) => Some(loaded),
            Err(e) => {
                eprintln!("not loading {}: {e}", path.display());
                None
            }
        };
        Self {
            loaded: RwLock::new(loaded),
            storage_failure: RwLock::new(None),
        }
    }

    /// Record what went wrong with the folder, as a sentence the UI can show
    /// over IPC (`storage_error`). It replaces any earlier one: the newest
    /// failure is the one worth telling.
    pub(crate) fn note_storage_failure(&self, message: impl Into<String>) {
        *self
            .storage_failure
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(message.into());
    }

    /// Forget it: something reached the folder, so the warning is stale.
    pub(crate) fn clear_storage_failure(&self) {
        *self
            .storage_failure
            .write()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// What last went wrong with the folder, for the `storage_error` command.
    pub(crate) fn storage_failure(&self) -> Option<String> {
        self.storage_failure
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Resolve the ledger to open: last CLI argument, else `LEDGER_PATH`, else
/// `main.bean`. A directory is resolved to the root journal inside it when it
/// is loaded (see [`load_baselined`]).
fn resolve_ledger_path() -> PathBuf {
    // `args()`/`var()` are the UTF-8 views, and a journal path is a filesystem
    // path: a name that is not valid Unicode would abort the process at startup
    // (`args()` panics) or be ignored silently (`var()`), while the CLI reads the
    // same input as a `PathBuf` and opens it.
    let raw = std::env::args_os()
        .skip(1) // drop the program name
        .last()
        .or_else(|| std::env::var_os("LEDGER_PATH"))
        .unwrap_or_else(|| std::ffi::OsString::from("main.bean"));
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::testing::TempDir;

    /// The sentence a failed restore leaves behind survives being read — the UI
    /// asks for it long after `setup` ran — and a later success clears it.
    #[test]
    fn a_folder_failure_is_remembered_until_the_folder_answers_again() {
        let state = AppState::default();
        assert_eq!(
            state.storage_failure(),
            None,
            "a fresh state has nothing to say"
        );

        state.note_storage_failure(reopen_failure("permission denied"));
        let remembered = state.storage_failure().expect("the failure was not kept");
        assert!(
            remembered.contains("could not be reopened"),
            "the reason is missing: {remembered}"
        );
        assert!(
            remembered.contains("Open folder"),
            "the way out is missing: {remembered}"
        );
        assert!(
            remembered.contains("permission denied"),
            "the cause is missing: {remembered}"
        );
        // Reading does not consume it: the alert may be drawn more than once.
        assert_eq!(
            state.storage_failure().as_deref(),
            Some(remembered.as_str())
        );

        state.clear_storage_failure();
        assert_eq!(
            state.storage_failure(),
            None,
            "a success must clear the warning"
        );
    }

    /// A folder is a ledger as soon as it holds a root journal: the loader's
    /// discovery names it, and the session's root becomes that journal — the
    /// file every later reload re-reads — rather than the folder.
    #[test]
    fn opens_a_folder_as_the_journal_inside_it() {
        let dir = TempDir::new("folder");
        let root = dir.file("main.bean");
        fs::write(
            &root,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
",
        )
        .unwrap();

        let loaded = load_baselined(dir.dir()).unwrap();
        assert_eq!(loaded.root, root);
        assert!(
            loaded.ledger.errors.is_empty(),
            "errors: {:?}",
            loaded.ledger.errors
        );
        assert_eq!(summary_of(&loaded).path, root.display().to_string());
    }

    /// A folder with no journal in it is refused by name — the loader is never
    /// asked to parse a directory.
    #[test]
    fn a_folder_without_a_journal_is_refused() {
        let dir = TempDir::new("empty-folder");
        let Err(refused) = load_baselined(dir.dir()) else {
            panic!("a folder with no journal loaded");
        };
        assert!(refused.contains("no root journal"), "{refused}");
    }
}
