// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where the ledger's files live.
//!
//! A desktop platform hands the app a directory it can read and write directly,
//! and that directory *is* the ledger. Android cannot: a folder the user picks is
//! a document tree behind the Storage Access Framework, addressed by content
//! URIs instead of paths, and the loader, the editor, `git2` and the rustledger
//! crates all want real paths on a real filesystem. So the app keeps a private,
//! ordinary-filesystem mirror of that tree and hands the mirror to everything
//! else; [`mirror_into`] fills it when a tree is opened and [`sync_back`] writes
//! the edited files home.
//!
//! The mirror logic is plain `std::fs` over a [`TreeSource`], so it compiles and
//! is tested on every platform; only the provider that talks to Android is
//! `cfg(target_os = "android")`. That is why a desktop build still gets
//! [`StorageKind`] and the trait: nothing about the seam is phone-specific, the
//! phone is just the only place a tree appears.

use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicBool, Ordering};

/// Which storage the ledger lives in.
///
/// Serialized as `"local"`/`"tree"` so it can cross IPC once the frontend has a
/// command to read it: today the SAF path is implemented and unit-tested but no
/// command opens a tree or reports this kind, so the app is always `Local`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// A directory this process reads and writes directly — every desktop
    /// platform.
    Local,
    /// An Android SAF tree, reached through a local mirror (see the module
    /// docs). Never the case off Android.
    Tree,
}

/// The storage mode this process is in.
///
/// Android is the only platform that can answer [`StorageKind::Tree`], and only
/// once the app has opened a SAF tree (or restored a persisted one) and said so
/// with `set_tree_active`. A phone with no tree yet is on the local fallback,
/// exactly like a desktop. Everywhere else the seam never has a tree, so the
/// answer is always [`StorageKind::Local`] and the mirror functions have no
/// caller.
pub fn kind() -> StorageKind {
    if tree_active() {
        StorageKind::Tree
    } else {
        StorageKind::Local
    }
}

/// Android: record that the ledger now lives in a SAF tree (or no longer does).
///
/// Called by the wiring that opens or closes a tree, so that [`kind`] — and with
/// it git, which is meaningless in a mirrored tree — answers for the mirror
/// rather than for a local directory.
#[cfg(target_os = "android")]
pub fn set_tree_active(active: bool) {
    TREE_ACTIVE.store(active, Ordering::Relaxed);
}

#[cfg(target_os = "android")]
static TREE_ACTIVE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "android")]
fn tree_active() -> bool {
    TREE_ACTIVE.load(Ordering::Relaxed)
}

#[cfg(not(target_os = "android"))]
fn tree_active() -> bool {
    // No platform but Android has a Storage Access Framework, so the seam is
    // never in tree mode here — but the branch stays real code so the `Tree`
    // variant above is still constructed, and `repo` still asks the same
    // question on every platform.
    false
}

/// One entry of a tree's directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The entry's name inside its directory — a single path component, never a
    /// path.
    pub name: String,
    /// Whether the entry is a directory.
    pub dir: bool,
}

/// The three operations the mirror needs from a tree.
///
/// Deliberately no delete, no rename and no metadata: a Beancount ledger is a
/// handful of text files that are edited in place, and every extra operation is
/// another thing the SAF provider may not support. A file missing from the tree
/// is simply not mirrored; a file the app creates in the mirror is created in
/// the tree on write.
pub trait TreeSource {
    /// The entries directly inside `dir` — `""` is the tree's own root.
    fn list(&self, dir: &str) -> Result<Vec<TreeEntry>, String>;

    /// The bytes of the file at `path`, a `/`-separated path relative to the
    /// tree's root.
    fn read(&self, path: &str) -> Result<Vec<u8>, String>;

    /// Replace the file at `path` with `bytes`, creating it — and any missing
    /// parent directories — when the tree does not have it yet.
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String>;
}

/// Copy the tree under `source` into `target_dir`, preserving relative paths.
///
/// The target is the app's own private mirror, so it is emptied first: the tree
/// is the authority on which files exist, and a file deleted there must not
/// survive in the mirror only to be written back by the next [`sync_back`].
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code) // called by the Android open path; the tests exercise it here
)]
pub fn mirror_into(source: &dyn TreeSource, target_dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(target_dir) {
        Ok(()) => {}
        // Nothing was there — the usual first mirror of a tree.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("clear {}: {e}", target_dir.display())),
    }
    fs::create_dir_all(target_dir).map_err(|e| format!("{}: {e}", target_dir.display()))?;
    copy_dir(source, "", target_dir)
}

/// Mirror one directory level and recurse into its subdirectories.
fn copy_dir(source: &dyn TreeSource, relative: &str, target: &Path) -> Result<(), String> {
    for entry in source.list(relative)? {
        let child = child_path(relative, &entry.name)?;
        let path = target.join(&entry.name);
        if entry.dir {
            fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            copy_dir(source, &child, &path)?;
        } else {
            let bytes = source.read(&child)?;
            fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// Write back the files the mirror changed, returning their relative paths,
/// sorted.
///
/// A file is written only when its bytes differ from the tree's, decided by
/// comparing the whole file rather than a timestamp (a content provider's
/// timestamps are not reliable enough to trust, and a ledger is small). A file
/// the tree does not have — a new include, say — is created. Nothing is written
/// back for a file the app merely read, and a deletion in the mirror is not
/// propagated: the tree's files are edited far more often than removed, and the
/// three operations in [`TreeSource`] cannot express a delete.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code) // called by the Android save path; the tests exercise it here
)]
pub fn sync_back(source: &dyn TreeSource, target_dir: &Path) -> Result<Vec<String>, String> {
    let mut changed = Vec::new();
    push_changes(source, "", target_dir, &mut changed)?;
    changed.sort();
    Ok(changed)
}

/// Walk one directory level of the mirror and write back the files that differ.
fn push_changes(
    source: &dyn TreeSource,
    relative: &str,
    dir: &Path,
    changed: &mut Vec<String>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("{}: unusable file name {name:?}", dir.display()))?;
        let child = child_path(relative, &name)?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        if file_type.is_dir() {
            push_changes(source, &child, &entry.path(), changed)?;
        } else if file_type.is_file() {
            let bytes =
                fs::read(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
            // A read failure on the tree side is normally "the tree does not
            // have this file yet": write it, and a genuine read problem shows up
            // as the write's own error instead of passing for "unchanged".
            let unchanged = source
                .read(&child)
                .map(|remote| remote == bytes)
                .unwrap_or(false);
            if !unchanged {
                source.write(&child, &bytes)?;
                changed.push(child);
            }
        }
        // Anything else — a symlink a provider cannot produce, a leftover from
        // some other tool — is left alone rather than followed out of the
        // mirror.
    }
    Ok(())
}

/// A child's `/`-separated path from its parent's, refusing a name that is not a
/// single ordinary path component: neither a tree nor a content provider may
/// steer the mirror (or a write-back) outside itself with a `..` or a path.
fn child_path(parent: &str, name: &str) -> Result<String, String> {
    let mut components = Path::new(name).components();
    let single = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if !single {
        return Err(format!("{parent:?}: unusable entry name {name:?}"));
    }
    Ok(if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    })
}

/// A [`TreeSource`] over an ordinary directory — what a desktop folder is, and
/// what the mirror tests drive the seam with.
#[cfg(test)]
struct LocalTree {
    root: PathBuf,
}

#[cfg(test)]
impl LocalTree {
    fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[cfg(test)]
impl TreeSource for LocalTree {
    fn list(&self, dir: &str) -> Result<Vec<TreeEntry>, String> {
        let path = self.root.join(dir);
        let mut entries = Vec::new();
        let listing = fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        for entry in listing {
            let entry = entry.map_err(|e| format!("{}: {e}", path.display()))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| format!("{}: unusable file name {name:?}", path.display()))?;
            let file_type = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
            if file_type.is_dir() {
                entries.push(TreeEntry { name, dir: true });
            } else if file_type.is_file() {
                entries.push(TreeEntry { name, dir: false });
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        let path = self.root.join(path);
        fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Android: the SAF implementation of the seam, and the persisted grant that
/// lets the app reopen a tree after a restart.
///
/// The plugin's sync API is blocking JNI, so the mirror is opened, read and
/// written from Tauri's blocking pool rather than the async runtime (see how the
/// commands call it) — the same rule as the rest of the backend: no blocking I/O
/// on the event loop.
#[cfg(target_os = "android")]
mod android {
    use std::io::{Read, Write};
    use std::path::Path;

    use tauri::AppHandle;
    use tauri_plugin_android_fs::{AndroidFsExt, FsUri};

    use crate::AppRuntime;

    use super::{TreeEntry, TreeSource};

    /// A tree behind the Storage Access Framework, addressed by the content URI
    /// of its root directory.
    pub struct AndroidTree {
        app: AppHandle<AppRuntime>,
        dir: FsUri,
    }

    impl AndroidTree {
        /// Ask the system directory picker for a tree and keep the grant across
        /// restarts. `Ok(None)` is the user closing the picker without a choice
        /// — not a failure.
        pub fn pick(app: &AppHandle<AppRuntime>) -> Result<Option<Self>, String> {
            let api = app.android_fs();
            let picked = api
                .picker()
                .pick_dir(None, false)
                .map_err(|e| format!("directory picker: {e}"))?;
            let Some(dir) = picked else {
                return Ok(None);
            };
            // Without this the grant dies with the process and a restored URI
            // would be a permission error on the next start.
            api.picker()
                .persist_uri_permission(&dir)
                .map_err(|e| format!("persist directory access: {e}"))?;
            Ok(Some(Self {
                app: app.clone(),
                dir,
            }))
        }

        /// Rebuild the tree from the URI the app stored when it opened it (see
        /// [`Self::to_json`]). The persisted grant is what makes this work after
        /// a restart; the caller checks for it before asking.
        pub fn from_json(app: &AppHandle<AppRuntime>, json: &str) -> Result<Self, String> {
            let dir = FsUri::from_json_str(json).map_err(|e| format!("stored tree URI: {e}"))?;
            Ok(Self {
                app: app.clone(),
                dir,
            })
        }

        /// The tree's URI as JSON, for the app's settings — the only thing that
        /// has to outlive the process.
        pub fn to_json(&self) -> Result<String, String> {
            self.dir
                .to_json_string()
                .map_err(|e| format!("tree URI: {e}"))
        }

        /// The URI of a directory inside the tree; `""` is the tree's root.
        fn dir_uri(&self, dir: &str) -> Result<FsUri, String> {
            if dir.is_empty() {
                return Ok(self.dir.clone());
            }
            self.app
                .android_fs()
                .resolve_dir_uri(&self.dir, Path::new(dir))
                .map_err(|e| format!("{dir}: {e}"))
        }
    }

    impl TreeSource for AndroidTree {
        fn list(&self, dir: &str) -> Result<Vec<TreeEntry>, String> {
            let uri = self.dir_uri(dir)?;
            let mut entries = self
                .app
                .android_fs()
                .read_dir(&uri)
                .map_err(|e| format!("{dir:?}: {e}"))?;
            entries.sort_by(|a, b| a.name().cmp(b.name()));
            Ok(entries
                .into_iter()
                .map(|entry| TreeEntry {
                    name: entry.name().to_owned(),
                    dir: entry.is_dir(),
                })
                .collect())
        }

        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            let api = self.app.android_fs();
            let uri = api
                .resolve_file_uri(&self.dir, Path::new(path))
                .map_err(|e| format!("{path}: {e}"))?;
            let mut file = api
                .open_file_readable(&uri)
                .map_err(|e| format!("{path}: {e}"))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|e| format!("{path}: {e}"))?;
            Ok(bytes)
        }

        fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
            let api = self.app.android_fs();
            let uri = match api.resolve_file_uri(&self.dir, Path::new(path)) {
                Ok(uri) => uri,
                // SAF has no "create if missing" on open: a file the mirror
                // created is not in the tree yet, so create it first (the call
                // brings missing parents with it) and then write the bytes.
                Err(_) => api
                    .create_new_file(&self.dir, Path::new(path), None)
                    .map_err(|e| format!("create {path}: {e}"))?,
            };
            // The writable handle truncates what the tree had there, which is
            // what a whole-file write-back wants.
            let mut file = api
                .open_file_writable(&uri)
                .map_err(|e| format!("{path}: {e}"))?;
            file.write_all(bytes).map_err(|e| format!("{path}: {e}"))
        }
    }
}

#[cfg(target_os = "android")]
pub use android::AndroidTree;

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;

    use super::{LocalTree, StorageKind, TreeEntry, TreeSource, kind, mirror_into, sync_back};
    use crate::testing::TempDir;

    /// The ledger fixture: a root file including a nested one, the shape the
    /// loader follows.
    const MAIN: &str = "include \"accounts/2026.bean\"\n\n2026-01-01 open Assets:Bank\n";
    const ACCOUNTS: &str = "2026-01-02 * \"Coffee\"\n  Expenses:Food 3.50 USD\n  Assets:Bank\n";

    /// A tree on disk holding a root include and its nested file.
    fn tree(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        fs::create_dir_all(dir.file("accounts")).unwrap();
        fs::write(dir.file("main.bean"), MAIN).unwrap();
        fs::write(dir.file("accounts/2026.bean"), ACCOUNTS).unwrap();
        dir
    }

    /// A [`TreeSource`] that records the paths written through it, so a test can
    /// assert a sync-back touched only what changed.
    struct Recording<'a> {
        inner: &'a LocalTree,
        writes: RefCell<Vec<String>>,
    }

    impl<'a> TreeSource for Recording<'a> {
        fn list(&self, dir: &str) -> Result<Vec<TreeEntry>, String> {
            self.inner.list(dir)
        }

        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            self.inner.read(path)
        }

        fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
            self.writes.borrow_mut().push(path.to_owned());
            self.inner.write(path, bytes)
        }
    }

    #[test]
    fn mirror_copies_the_tree_with_its_nested_include() {
        let source = tree("tree-mirror-source");
        let target = TempDir::new("tree-mirror-target");
        let mirror = target.file("mirror");

        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        assert_eq!(fs::read_to_string(mirror.join("main.bean")).unwrap(), MAIN);
        assert_eq!(
            fs::read_to_string(mirror.join("accounts/2026.bean")).unwrap(),
            ACCOUNTS
        );
    }

    #[test]
    fn mirror_replaces_a_stale_copy() {
        let source = tree("tree-refresh-source");
        let target = TempDir::new("tree-refresh-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        // A file that has since been deleted from the tree.
        fs::write(mirror.join("gone.bean"), "2026-01-01 close Assets:Bank\n").unwrap();

        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        assert!(!mirror.join("gone.bean").exists());
        assert_eq!(fs::read_to_string(mirror.join("main.bean")).unwrap(), MAIN);
    }

    #[test]
    fn sync_back_writes_only_the_changed_file() {
        let source = tree("tree-sync-source");
        let target = TempDir::new("tree-sync-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        let edited = "2026-01-02 * \"Coffee\"\n  Expenses:Food 4.00 USD\n  Assets:Bank\n";
        fs::write(mirror.join("accounts/2026.bean"), edited).unwrap();

        let tree = LocalTree::new(source.dir());
        let recording = Recording {
            inner: &tree,
            writes: RefCell::new(Vec::new()),
        };
        let changed = sync_back(&recording, &mirror).unwrap();

        assert_eq!(changed, ["accounts/2026.bean"]);
        assert_eq!(*recording.writes.borrow(), ["accounts/2026.bean"]);
        assert_eq!(
            fs::read_to_string(source.file("accounts/2026.bean")).unwrap(),
            edited
        );
        // The untouched file is byte-for-byte what it was, and was not written.
        assert_eq!(fs::read_to_string(source.file("main.bean")).unwrap(), MAIN);
    }

    #[test]
    fn sync_back_creates_a_file_the_tree_does_not_have() {
        let source = tree("tree-new-source");
        let target = TempDir::new("tree-new-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        let added = "2026-01-03 * \"Tea\"\n  Expenses:Food 2.00 USD\n  Assets:Bank\n";
        fs::write(mirror.join("accounts/2027.bean"), added).unwrap();

        let changed = sync_back(&LocalTree::new(source.dir()), &mirror).unwrap();

        assert_eq!(changed, ["accounts/2027.bean"]);
        assert_eq!(
            fs::read_to_string(source.file("accounts/2027.bean")).unwrap(),
            added
        );
    }

    #[test]
    fn sync_back_with_no_edit_writes_nothing() {
        let source = tree("tree-clean-source");
        let target = TempDir::new("tree-clean-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        let tree = LocalTree::new(source.dir());
        let recording = Recording {
            inner: &tree,
            writes: RefCell::new(Vec::new()),
        };
        let changed = sync_back(&recording, &mirror).unwrap();

        assert!(changed.is_empty());
        assert!(recording.writes.borrow().is_empty());
    }

    /// A tree whose provider reports a name that would escape the mirror.
    struct Escaping;

    impl TreeSource for Escaping {
        fn list(&self, _dir: &str) -> Result<Vec<TreeEntry>, String> {
            Ok(vec![TreeEntry {
                name: "../escape.bean".to_owned(),
                dir: false,
            }])
        }

        fn read(&self, _path: &str) -> Result<Vec<u8>, String> {
            Ok(b"2026-01-01 close Assets:Bank\n".to_vec())
        }

        fn write(&self, _path: &str, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn mirror_refuses_a_name_that_would_escape_the_target() {
        let target = TempDir::new("tree-escape");
        let mirror = target.file("mirror");

        let error = mirror_into(&Escaping, &mirror).unwrap_err();

        assert!(error.contains("unusable entry name"), "{error}");
        assert!(!target.file("escape.bean").exists());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn local_directory_is_not_a_tree() {
        // Off Android there is no SAF mode to be in, whatever was picked before.
        assert_eq!(kind(), StorageKind::Local);
    }
}
