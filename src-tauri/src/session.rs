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
    rustledger_loader::load(root, &LoadOptions::default()).map_err(|e| e.to_string())
}

/// The journal a path names: the file itself, or the root journal directly
/// inside a directory — the loader's own discovery order (`main.bean`,
/// `main.beancount`, `ledger.bean`, …). This is what makes opening a folder a
/// way to point the app at a ledger without naming its root file.
fn journal_in(path: &Path) -> Result<PathBuf, String> {
    if !path.is_dir() {
        return Ok(path.to_path_buf());
    }
    rustledger_loader::discover_journal_file(path)
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

/// Shared app state: the currently loaded ledger, if any.
#[derive(Default)]
pub struct AppState {
    pub(crate) loaded: RwLock<Option<Loaded>>,
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
        }
    }
}

/// Resolve the ledger to open: last CLI argument, else `LEDGER_PATH`, else
/// `main.bean`. A directory is resolved to the root journal inside it when it
/// is loaded (see [`load_baselined`]).
fn resolve_ledger_path() -> PathBuf {
    let raw = std::env::args()
        .skip(1) // drop the program name
        .last()
        .or_else(|| std::env::var("LEDGER_PATH").ok())
        .unwrap_or_else(|| "main.bean".to_string());
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::testing::TempDir;

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
