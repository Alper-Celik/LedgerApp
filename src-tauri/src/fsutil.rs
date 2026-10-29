// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Replacing a file's bytes whole.
//!
//! Every store in the backend needs the same rule — the journal, the assistant's
//! config, the index series, dashboards, conversations, the log's state file, the
//! mirror's manifest: a crash must leave either the old file or the new one and
//! never a half-written mixture, a reader must never see the temporary file, and
//! a failed write must leave no litter behind. One implementation so those
//! guarantees cannot drift apart per module.
//!
//! Two writes stay outside it on purpose: the log's blobs are named by their own
//! hash (so the create is the identity check) and the mirror's directory swap is
//! a rename of a whole tree, not a file.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Write `bytes` to `path` whole or not at all.
///
/// The bytes go to a dotted temporary file beside the target, are flushed to the
/// device, get their permissions (the requested `mode`, or the target's own when
/// it exists), and only then are renamed over it; the directory is flushed so
/// the rename itself survives a crash. `mode` is Unix permission bits; pass the
/// target's own via `None` for an ordinary file.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), String> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    // Dotted (a listing or a `git status` that runs mid-write ignores it) and
    // unique per process and call (two writers cannot pick the same name).
    let temporary = directory.join(format!(".{name}.{}.{}.tmp", std::process::id(), next_id()));
    let existing = fs::metadata(path).ok().map(|meta| meta.permissions());

    let result = (|| -> std::io::Result<()> {
        // `create_new`, not `create`: a symlink planted at the temporary name
        // must not be followed into some other file, and a name this process is
        // already using is a bug worth surfacing rather than truncating.
        let mut file = create_new(&temporary)?;
        // The mode goes on *before* the bytes: a store that asks for 0600 (the
        // assistant's key) must not be world-readable for the duration of the
        // write and its flush. A target that already exists keeps its own mode,
        // so a replacement is not a permission change; a new file keeps the
        // umask's.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            match (mode, &existing) {
                (Some(mode), _) => {
                    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?
                }
                (None, Some(permissions)) => fs::set_permissions(&temporary, permissions.clone())?,
                (None, None) => {}
            }
        }
        #[cfg(not(unix))]
        let _ = (mode, &existing);
        file.write_all(bytes)?;
        // On the device before the rename, so the rename never points at bytes
        // that are still in a cache.
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        // The rename is durable only once the directory itself is flushed.
        if let Ok(handle) = File::open(directory) {
            let _ = handle.sync_all();
        }
        Ok(())
    })();

    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(format!("cannot write {}: {error}", path.display()))
        }
    }
}

/// Create the temporary file, refusing anything already at its path.
///
/// `pid` and a counter make a collision another process's doing — a stale file
/// from a recycled pid, or something planted — so it is removed once and the
/// create retried.
fn create_new(path: &Path) -> std::io::Result<File> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(path)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        }
        other => other,
    }
}

/// A number no other write in this process is using.
fn next_id() -> u32 {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::write_atomic;
    use crate::testing::TempDir;

    #[test]
    fn replaces_the_bytes_and_leaves_no_temporary_behind() {
        let dir = TempDir::new("fsutil-write");
        let path = dir.file("store.json");
        std::fs::write(&path, b"old\n").unwrap();

        write_atomic(&path, b"new\n", None).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new\n");
        let leftovers: Vec<String> = std::fs::read_dir(dir.dir())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "store.json")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_requested_mode_applies_to_the_replacement() {
        let dir = TempDir::new("fsutil-mode");
        let path = dir.file("secret.json");
        std::fs::write(&path, b"{}\n").unwrap();

        write_atomic(&path, b"{}", Some(0o600)).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }

    #[test]
    fn a_failed_write_reports_the_target_and_keeps_the_old_bytes() {
        let dir = TempDir::new("fsutil-failure");
        // A directory in the target's place: the rename cannot land on it.
        let path = dir.file("store.json");
        std::fs::create_dir(&path).unwrap();

        let error = write_atomic(&path, b"new\n", None).unwrap_err();

        assert!(error.contains("cannot write"), "{error}");
        assert!(path.is_dir());
    }
}
