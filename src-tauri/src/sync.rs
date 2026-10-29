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

use std::fs;
use std::path::{Path, PathBuf};

use blake3::Hasher;
use serde::Serialize;

use crate::history;

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

impl ExternalChange {
    /// Whether the file is new to this device (no recorded hash).
    pub fn is_new(&self) -> bool {
        self.seen.is_empty()
    }
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
            &[history::Change::new(&ledger, None, Some(&fs::read(&ledger).unwrap()))],
        )
        .unwrap();

        // Another device rewrote it.
        fs::write(&ledger, "2026-01-02 open Assets:Cash\n").unwrap();

        let changes = changed(&log).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, ledger.display().to_string());
        assert!(!changes[0].is_new());
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
        assert!(changes[0].disk.is_empty(), "a deleted file compares as empty");
    }

    #[test]
    fn a_file_the_log_never_mentions_is_ignored() {
        let dir = TempDir::new("sync-unknown");
        fs::write(dir.file("other.bean"), "not ours\n").unwrap();
        let log = log_dir(dir.dir());

        assert!(changed(&log).unwrap().is_empty());
    }
}
