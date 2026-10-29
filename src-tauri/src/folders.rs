// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The in-app folder picker's data: the folders inside a directory, and which
//! of them already hold a root journal.
//!
//! This backs the in-app picker, the fallback where the system chooser has no
//! implementation (`pick_ledger_folder` answers with an error on phones, today).
//! Desktop takes the system chooser instead: Tauri's dialog plugin through the
//! xdg-desktop-portal on Linux, which is a D-Bus service — the plugin's default
//! GTK backend is what must stay away from this process (rfd builds the dialog on
//! Qt's GLib context after initialising GTK on its own thread, which segfaults).
//! Listing directories ourselves has no toolkit, no portal and no thread
//! affinity to get wrong.

use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{FolderEntry, FolderListing};

/// The folders directly inside `path`, name-sorted, hidden ones left out.
///
/// A folder is flagged `ledger` when the loader finds a root journal in it, so
/// the picker can point at the ones that will open rather than only failing
/// after the click.
pub(crate) fn list_folders(path: &Path) -> Result<FolderListing, String> {
    let path = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut folders = Vec::new();
    let entries = fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        // Hidden directories are configuration, not ledgers.
        if name.starts_with('.') {
            continue;
        }
        let child = entry.path();
        // `is_dir` follows symlinks: a linked folder is a folder to open.
        if !child.is_dir() {
            continue;
        }
        folders.push(FolderEntry {
            ledger: has_journal(&child),
            name,
            path: child.display().to_string(),
        });
    }
    folders.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(FolderListing {
        ledger: has_journal(&path),
        parent: path.parent().map(|parent| parent.display().to_string()),
        path: path.display().to_string(),
        folders,
    })
}

/// Whether a root journal sits directly in `dir`.
fn has_journal(dir: &Path) -> bool {
    rustledger_loader::discover_journal_file(dir).is_some()
}

/// The user's home directory, the picker's starting point.
pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn lists_folders_and_flags_the_ones_with_a_journal() {
        let dir = TempDir::new("browse");
        fs::create_dir(dir.dir().join("Books")).unwrap();
        fs::create_dir(dir.dir().join("archive")).unwrap();
        fs::create_dir(dir.dir().join(".hidden")).unwrap();
        fs::write(dir.dir().join("notes.txt"), "not a folder").unwrap();
        fs::write(
            dir.dir().join("Books/main.bean"),
            "2026-01-01 open Assets:Bank\n",
        )
        .unwrap();

        let listing = list_folders(dir.dir()).unwrap();
        let names: Vec<&str> = listing.folders.iter().map(|f| f.name.as_str()).collect();

        // Name-sorted, files and hidden folders left out.
        assert_eq!(names, ["archive", "Books"]);
        assert!(!listing.ledger, "the parent holds no journal itself");
        let books = listing.folders.iter().find(|f| f.name == "Books").unwrap();
        assert!(books.ledger);
        assert_eq!(
            books.path,
            fs::canonicalize(dir.file("Books"))
                .unwrap()
                .display()
                .to_string()
        );
        let canonical = fs::canonicalize(dir.dir()).unwrap();
        assert_eq!(listing.path, canonical.display().to_string());
        assert_eq!(
            listing.parent,
            canonical
                .parent()
                .map(|parent| parent.display().to_string())
        );
    }
}
