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
//! else.
//!
//! The mirror is not a copy the app owns outright: the folder is edited by the
//! same sync client that carries the ledger to other devices, so a write-back
//! that only knows the mirror's bytes would overwrite work it never saw. Every
//! pass is therefore three-way, with a per-file base (see [`Manifest`]): a file
//! only the folder changed is pulled on open and reported on a write, a file only
//! the app changed is pushed, and a file both changed is left alone, reported,
//! and settled only when the reader picks a side ([`resolve_conflicts`]). Opening
//! a tree reconciles ([`mirror_into`]); a write calls [`sync_back`], which differs
//! only in refusing to pull over bytes the app is showing.
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
/// Serialized as `"local"`/`"tree"` and read over IPC (`storage_kind`): the
/// frontend branches on it to decide whether a write has to be pushed into a
/// folder and whether "reload" means pulling one first. It follows the ledger
/// that is loaded — a path inside the mirror is `Tree` — so opening an ordinary
/// directory afterwards ends tree mode again.
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

/// Off Android the seam never holds a tree, so nothing can enter tree mode.
#[cfg(not(target_os = "android"))]
pub fn set_tree_active(_active: bool) {}

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

/// The operations the mirror needs from a tree.
///
/// Deliberately no rename and no metadata: a Beancount ledger is a handful of
/// text files that are edited in place, and every extra operation is another
/// thing the SAF provider may not support. `delete` is here because the app
/// deletes things of its own — a conversation, a pruned pre-image — and a
/// deletion that never reaches the folder is undone by the next restore.
pub trait TreeSource {
    /// The entries directly inside `dir` — `""` is the tree's own root.
    fn list(&self, dir: &str) -> Result<Vec<TreeEntry>, String>;

    /// The bytes of the file at `path`, a `/`-separated path relative to the
    /// tree's root.
    fn read(&self, path: &str) -> Result<Vec<u8>, String>;

    /// Replace the file at `path` with `bytes`, creating it — and any missing
    /// parent directories — when the tree does not have it yet.
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String>;

    /// Remove the file at `path`. A tree that cannot delete says so; the
    /// reconciliation then leaves the file alone rather than failing the whole
    /// sync.
    fn delete(&self, path: &str) -> Result<(), String> {
        Err(format!("{path}: this folder cannot delete files"))
    }
}

/// A file both sides changed: the mirror keeps its bytes and the tree is left
/// alone until they agree again (or the reader resolves it in the merge screen).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct MirrorConflict {
    /// The `/`-separated path relative to the ledger folder.
    pub path: String,
    /// Why the two sides could not be reconciled in one direction.
    pub reason: String,
}

/// What one reconciliation did, per relative path.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct SyncReport {
    /// Files whose tree version replaced the mirror's, because the app had not
    /// touched them.
    pub pulled: Vec<String>,
    /// Files the mirror's version replaced in the tree.
    pub pushed: Vec<String>,
    /// Files the mirror no longer has and the tree did not touch, so the tree's
    /// copy was removed.
    pub removed: Vec<String>,
    /// Files changed on both sides: neither was written.
    pub conflicts: Vec<MirrorConflict>,
    /// Files the folder changed while the app had them, which the app must not
    /// overwrite — the sync bar offers the reload the same way it does for a
    /// local ledger.
    pub external: Vec<String>,
}

impl SyncReport {
    /// Whether anything at all moved.
    pub fn is_empty(&self) -> bool {
        self.pulled.is_empty()
            && self.pushed.is_empty()
            && self.removed.is_empty()
            && self.conflicts.is_empty()
            && self.external.is_empty()
    }
}

/// Names never mirrored, in either direction.
///
/// A repository's metadata is not ledger content: copying it makes the private
/// mirror a repository of its own, so the app's `stage_file` rewrites a copy of
/// the index, and the write-back then publishes that index — and the refs
/// beside it — over the real repository, while git is nominally off in this mode.
/// The same reasoning covers the other two version-control directories.
fn is_metadata(name: &str) -> bool {
    matches!(name, ".git" | ".hg" | ".svn")
}

/// The bytes the tree held for each file the last time the two sides agreed —
/// the base a three-way comparison needs. Stored beside the mirror, never inside
/// it, so it can never reach the ledger folder.
///
/// A path that stopped agreeing is remembered as a conflict instead: it is left
/// untouched (and reported again) until the two sides are equal once more. That
/// is what keeps a resolution from being invented by whichever side happens to
/// be written next.
#[derive(Debug, Default)]
struct Manifest {
    files: std::collections::BTreeMap<String, String>,
    conflicts: std::collections::BTreeSet<String>,
}

impl Manifest {
    fn path(mirror: &Path) -> std::path::PathBuf {
        let name = mirror
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "tree".to_owned());
        let sibling = format!("{name}.manifest.json");
        match mirror.parent() {
            Some(parent) => parent.join(sibling),
            None => std::path::PathBuf::from(sibling),
        }
    }

    fn load(mirror: &Path) -> Self {
        let Ok(text) = fs::read_to_string(Self::path(mirror)) else {
            return Self::default();
        };
        #[derive(serde::Deserialize, Default)]
        struct Stored {
            #[serde(default)]
            files: std::collections::BTreeMap<String, String>,
            #[serde(default)]
            conflicts: std::collections::BTreeSet<String>,
        }
        match serde_json::from_str::<Stored>(&text) {
            Ok(stored) => Self {
                files: stored.files,
                conflicts: stored.conflicts,
            },
            // A manifest is a cache: losing it costs a pull, not data.
            Err(_) => Self::default(),
        }
    }

    fn save(&self, mirror: &Path) -> Result<(), String> {
        #[derive(serde::Serialize)]
        struct Stored<'a> {
            files: &'a std::collections::BTreeMap<String, String>,
            conflicts: &'a std::collections::BTreeSet<String>,
        }
        let path = Self::path(mirror);
        let stored = Stored {
            files: &self.files,
            conflicts: &self.conflicts,
        };
        let mut json = serde_json::to_vec(&stored).map_err(|error| format!("manifest: {error}"))?;
        json.push(b'\n');
        crate::fsutil::write_atomic(&path, &json, None)
    }
}

/// Forget the mirror: the app is about to open a *different* folder, so neither
/// the private copy nor what it knew about the previous tree applies.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code) // called by the Android pick; the tests exercise it here
)]
pub fn reset_mirror(target_dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(target_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("clear {}: {error}", target_dir.display())),
    }
    match fs::remove_file(Manifest::path(target_dir)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", Manifest::path(target_dir).display())),
    }
    Ok(())
}

/// One reconciliation at a time.
///
/// The manifest is a whole-file read-modify-write over a whole-tree walk, and the
/// callers overlap: the UI syncs on every write without awaiting, a conversation
/// command flushes its own pass, and a reload or a resolution can start while
/// either is running. Two passes reading the same base invent a conflict neither
/// side caused.
static RECONCILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serialized() -> std::sync::MutexGuard<'static, ()> {
    RECONCILE.lock().unwrap_or_else(|error| error.into_inner())
}

/// Open a tree into the mirror, three-way.
///
/// Called when a tree is picked or restored. It does **not** wipe the mirror:
/// files the app wrote and has not pushed yet are pushed first, and a file the
/// folder changed while the app was away is pulled, so neither side is thrown
/// away. A file both sides changed is reported and left alone.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code) // called by the Android open path; the tests exercise it here
)]
pub fn mirror_into(source: &dyn TreeSource, target_dir: &Path) -> Result<SyncReport, String> {
    let _serial = serialized();
    reconcile(source, target_dir, true)
}

/// Write the mirror's bytes home after an edit, three-way.
///
/// Only files the app actually changed are written; a file the folder changed
/// since it was mirrored is reported as external (and never overwritten), so the
/// reader is offered a reload instead of losing the other device's edit.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code) // called by the Android save path; the tests exercise it here
)]
pub fn sync_back(source: &dyn TreeSource, target_dir: &Path) -> Result<SyncReport, String> {
    let _serial = serialized();
    reconcile(source, target_dir, false)
}

/// Adopt a newly picked tree: copy it into a staging mirror, prove the result is
/// a ledger this app can load, and only then replace the mirror.
///
/// Replacing first would destroy the previous mirror — including edits this
/// device never pushed — before anything knows the picked folder is a ledger at
/// all; a folder without a journal would leave the app holding a ledger whose
/// files were just deleted.
pub fn adopt_tree(source: &dyn TreeSource, target_dir: &Path) -> Result<(), String> {
    let _serial = serialized();
    let name = target_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tree".to_owned());
    let staging = target_dir.with_file_name(format!("{name}.next"));
    reset_mirror(&staging)?;
    let adopted = reconcile(source, &staging, true).and_then(|_| {
        crate::session::load_baselined(&staging)
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    if let Err(error) = adopted {
        let _ = reset_mirror(&staging);
        return Err(error);
    }
    // The swap: the manifest moves with the bytes, or the next pass would compare
    // the new mirror against the old folder's base.
    reset_mirror(target_dir)?;
    fs::rename(&staging, target_dir)
        .map_err(|error| format!("cannot replace {}: {error}", target_dir.display()))?;
    fs::rename(Manifest::path(&staging), Manifest::path(target_dir)).map_err(|error| {
        format!(
            "cannot replace {}: {error}",
            Manifest::path(target_dir).display()
        )
    })
}

/// Reconcile the mirror and the tree, file by file, with the last agreed bytes
/// as the base. `pull` says which side wins a tree-side change to a file the app
/// did not touch: an open pulls it (the mirror is only a cache then), a write
/// leaves it and reports it.
fn reconcile(source: &dyn TreeSource, target_dir: &Path, pull: bool) -> Result<SyncReport, String> {
    let mut manifest = Manifest::load(target_dir);
    // A mirror the app has never recorded (an older version, a lost manifest)
    // is not evidence of a file the app created: without a manifest the folder
    // is the only record of which files exist.
    let knew_anything = !manifest.files.is_empty();
    let tree = walk_tree(source)?;
    let mut report = SyncReport::default();

    // The mirror first, so a freshly picked folder starts from an empty one.
    fs::create_dir_all(target_dir).map_err(|e| format!("{}: {e}", target_dir.display()))?;

    // The base starts as what the last pass agreed on and is moved per file to
    // the version the two sides ended on: a push makes the app's bytes the base,
    // a pull the folder's, and a path that could not be settled keeps the base it
    // had. Assigning the folder's snapshot instead would mistake the app's own
    // write for a later change in the folder — clobbering a peer's edit, or
    // turning a second app edit into a permanent conflict.
    let mut base = std::mem::take(&mut manifest.files);

    // 1. Everything the tree holds.
    for (path, tree_hash) in &tree {
        let local = target_dir.join(path);
        let tree_hash = tree_hash.clone();
        if manifest.conflicts.contains(path) {
            let local_hash = fs::read(&local).ok().map(|b| crate::sync::hash(&b));
            if local_hash.as_ref() == Some(&tree_hash) {
                manifest.conflicts.remove(path);
                base.insert(path.clone(), tree_hash);
            } else {
                report.conflicts.push(MirrorConflict {
                    path: path.clone(),
                    reason: "changed in the folder and in the app".to_string(),
                });
            }
            continue;
        }
        match fs::read(&local) {
            Ok(local_bytes) => {
                let local_hash = crate::sync::hash(&local_bytes);
                if local_hash == tree_hash {
                    base.insert(path.clone(), tree_hash);
                    continue;
                }
                match base.get(path).map(String::as_str) {
                    // Untouched here since the last agreement: the folder moved.
                    Some(known) if known == local_hash => {
                        if pull {
                            write_local(target_dir, path, &source.read(path)?)?;
                            report.pulled.push(path.clone());
                            base.insert(path.clone(), tree_hash);
                        } else {
                            // The app's copy stays; the folder's newer bytes are
                            // reported and the base stays where it was, so the
                            // next pass still sees one side moved.
                            report.external.push(path.clone());
                        }
                    }
                    // Untouched there: this is the app's edit, and it goes home.
                    Some(known) if known == tree_hash => {
                        source.write(path, &local_bytes)?;
                        report.pushed.push(path.clone());
                        base.insert(path.clone(), local_hash);
                    }
                    // Both moved. Neither side is written, and the base stays:
                    // the conflict is the record that nothing was agreed here.
                    Some(_) => {
                        manifest.conflicts.insert(path.clone());
                        report.conflicts.push(MirrorConflict {
                            path: path.clone(),
                            reason: "changed in the folder and in the app".to_string(),
                        });
                    }
                    // Never seen in the manifest (a folder this app did not
                    // mirror, a lost manifest, or a path whose deletion this app
                    // already pushed and the folder re-created): opening takes
                    // the folder's version, a write reports it instead of
                    // replacing bytes the reader is looking at.
                    None => {
                        if pull {
                            write_local(target_dir, path, &source.read(path)?)?;
                            report.pulled.push(path.clone());
                            base.insert(path.clone(), tree_hash);
                        } else {
                            report.external.push(path.clone());
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // The app deleted it. If the folder did not change it since the
                // last agreement, the deletion is the app's and goes home;
                // otherwise the folder's newer file is kept.
                match base.get(path).map(String::as_str) {
                    Some(known) if known == tree_hash => {
                        source.delete(path)?;
                        report.removed.push(path.clone());
                        base.remove(path);
                    }
                    _ => {
                        write_local(target_dir, path, &source.read(path)?)?;
                        report.pulled.push(path.clone());
                        base.insert(path.clone(), tree_hash);
                    }
                }
            }
            Err(error) => return Err(format!("{}: {error}", local.display())),
        }
    }

    // 2. Everything only the mirror has: a file the app created, or one the
    //    folder deleted.
    let mut extra = Vec::new();
    collect_local(target_dir, "", &mut extra)?;
    for path in extra {
        if tree.contains_key(&path) {
            continue;
        }
        if manifest.conflicts.contains(&path) {
            // A delete-versus-edit conflict stays visible until the reader
            // settles it: reporting it once and then skipping the path would
            // leave the file unpushable and unremovable with nothing on screen
            // saying why.
            report.conflicts.push(MirrorConflict {
                path,
                reason: "deleted in the folder, edited in the app".to_string(),
            });
            continue;
        }
        let bytes = fs::read(target_dir.join(&path))
            .map_err(|error| format!("{}: {error}", target_dir.join(&path).display()))?;
        let local_hash = crate::sync::hash(&bytes);
        match base.get(&path).map(String::as_str) {
            // Known and untouched here: the folder deleted it. Opening follows
            // the folder; a write must not drop a file the loaded ledger still
            // has, so it reports the deletion and leaves the reload to the
            // reader.
            Some(known) if known == local_hash => {
                if !pull {
                    report.external.push(path.clone());
                    continue;
                }
                source.delete(&path)?;
                fs::remove_file(target_dir.join(&path))
                    .map_err(|error| format!("{}: {error}", path))?;
                report.removed.push(path.clone());
                base.remove(&path);
            }
            // Known and edited here, while the folder deleted it: both sides
            // moved, and which one wins is the reader's call.
            Some(_) => {
                manifest.conflicts.insert(path.clone());
                report.conflicts.push(MirrorConflict {
                    path,
                    reason: "deleted in the folder, edited in the app".to_string(),
                });
            }
            // Never seen, by a mirror that knows other files: the app created
            // it, so it goes home. On a mirror with no manifest at all, the
            // folder's listing is the record and the stray file goes.
            None if knew_anything => {
                source.write(&path, &bytes)?;
                report.pushed.push(path.clone());
                base.insert(path.clone(), local_hash);
            }
            None => {
                fs::remove_file(target_dir.join(&path))
                    .map_err(|error| format!("{}: {error}", path))?;
                report.removed.push(path);
            }
        }
    }

    // 3. Save the base and the conflicts that still stand.
    manifest.files = base;
    manifest.save(target_dir)?;

    for list in [
        &mut report.pulled,
        &mut report.pushed,
        &mut report.removed,
        &mut report.external,
    ] {
        list.sort();
    }
    report.conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(report)
}

/// Which side a conflicted file is settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Resolve {
    /// Keep the app's bytes and write them to the folder.
    Mine,
    /// Take the folder's bytes, discarding the app's version of that file.
    Folder,
}

/// Settle every recorded conflict on one side, then reconcile once more.
///
/// A conflict is never resolved automatically — both sides changed, so either
/// choice discards someone's work — but it has to be resolvable, or the file
/// would stay unreadable-for-writing for the life of the mirror.
#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code) // called by the Android resolve command; the tests exercise it here
)]
pub fn resolve_conflicts(
    source: &dyn TreeSource,
    target_dir: &Path,
    take: Resolve,
) -> Result<SyncReport, String> {
    let _serial = serialized();
    let mut manifest = Manifest::load(target_dir);
    // One walk answers "does the folder still have this file?", so a deletion is
    // a fact rather than a read error that might have been a revoked grant.
    let tree = walk_tree(source)?;
    for path in std::mem::take(&mut manifest.conflicts) {
        let local = target_dir.join(&path);
        match take {
            Resolve::Mine => {
                let Ok(bytes) = fs::read(&local) else {
                    // The app deleted it: the deletion is what "mine" means.
                    source.delete(&path)?;
                    manifest.files.remove(&path);
                    continue;
                };
                source.write(&path, &bytes)?;
                manifest.files.insert(path, crate::sync::hash(&bytes));
            }
            Resolve::Folder => match tree.get(&path) {
                Some(hash) => {
                    let bytes = source.read(&path)?;
                    write_local(target_dir, &path, &bytes)?;
                    manifest.files.insert(path, hash.clone());
                }
                // Gone from the folder: the folder's answer is that it is gone.
                None => {
                    match fs::remove_file(&local) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(format!("{}: {error}", local.display())),
                    }
                    manifest.files.remove(&path);
                }
            },
        }
    }
    manifest.save(target_dir)?;
    reconcile(source, target_dir, true)
}

/// Every file the tree holds, by `/`-separated relative path and content hash.
///
/// Hashes, not bytes: a ledger folder may hold the documents its entries attach,
/// and keeping all of them in memory to compare is pointless when only the few
/// that need copying are read again.
fn walk_tree(
    source: &dyn TreeSource,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut files = std::collections::BTreeMap::new();
    walk_tree_into(source, "", &mut files)?;
    Ok(files)
}

fn walk_tree_into(
    source: &dyn TreeSource,
    relative: &str,
    files: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    for entry in source.list(relative)? {
        if is_metadata(&entry.name) {
            continue;
        }
        let child = child_path(relative, &entry.name)?;
        if entry.dir {
            walk_tree_into(source, &child, files)?;
        } else {
            let hash = crate::sync::hash(&source.read(&child)?);
            files.insert(child, hash);
        }
    }
    Ok(())
}

/// Every file the mirror holds, by relative path.
fn collect_local(dir: &Path, relative: &str, files: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("{}: unusable file name {name:?}", dir.display()))?;
        if is_metadata(&name) {
            continue;
        }
        let child = child_path(relative, &name)?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            collect_local(&entry.path(), &child, files)?;
        } else if file_type.is_file() {
            files.push(child);
        }
    }
    Ok(())
}

/// Write one file into the mirror, creating its parents.
fn write_local(target_dir: &Path, path: &str, bytes: &[u8]) -> Result<(), String> {
    let local = target_dir.join(path);
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&local, bytes).map_err(|error| format!("{}: {error}", local.display()))
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

    fn delete(&self, path: &str) -> Result<(), String> {
        let path = self.root.join(path);
        match fs::remove_file(&path) {
            // Deleting what is already gone is the same outcome.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other.map_err(|e| format!("{}: {e}", path.display())),
        }
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

        fn delete(&self, path: &str) -> Result<(), String> {
            let api = self.app.android_fs();
            // A file the folder no longer has is already the outcome asked for;
            // resolving is how absence shows here, before the delete can fail.
            let Ok(uri) = api.resolve_file_uri(&self.dir, Path::new(path)) else {
                return Ok(());
            };
            api.remove_file(&uri).map_err(|e| format!("{path}: {e}"))
        }
    }
}

#[cfg(target_os = "android")]
pub use android::AndroidTree;

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;

    use super::{
        LocalTree, Resolve, StorageKind, TreeEntry, TreeSource, kind, mirror_into,
        resolve_conflicts, sync_back,
    };
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
    fn mirror_drops_a_file_the_folder_deleted() {
        let source = tree("tree-refresh-source");
        let target = TempDir::new("tree-refresh-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        // The folder lost a file while the app was closed; the mirror's copy was
        // not edited here, so it goes with it.
        fs::remove_file(source.file("accounts/2026.bean")).unwrap();

        let report = mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        assert_eq!(report.removed, ["accounts/2026.bean"]);
        assert!(!mirror.join("accounts/2026.bean").exists());
        assert_eq!(fs::read_to_string(mirror.join("main.bean")).unwrap(), MAIN);
    }

    #[test]
    fn sync_back_never_overwrites_a_file_the_folder_changed() {
        let source = tree("tree-remote-source");
        let target = TempDir::new("tree-remote-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        // Another device edits the nested file, then this one edits the root.
        let remote = "2026-01-02 * \"Tea\"\n  Expenses:Food 2.00 USD\n  Assets:Bank\n";
        fs::write(source.file("accounts/2026.bean"), remote).unwrap();
        let local = format!("{MAIN}\n; edited here\n");
        fs::write(mirror.join("main.bean"), &local).unwrap();

        let tree = LocalTree::new(source.dir());
        let recording = Recording {
            inner: &tree,
            writes: RefCell::new(Vec::new()),
        };
        let report = sync_back(&recording, &mirror).unwrap();

        // Only this device's file went home; the other device's edit stands and
        // is reported so the app can offer a reload.
        assert_eq!(report.pushed, ["main.bean"]);
        assert_eq!(report.external, ["accounts/2026.bean"]);
        assert_eq!(*recording.writes.borrow(), ["main.bean"]);
        assert_eq!(
            fs::read_to_string(source.file("accounts/2026.bean")).unwrap(),
            remote
        );
    }

    #[test]
    fn a_write_reports_a_folder_deletion_instead_of_dropping_the_file() {
        let source = tree("tree-gone-source");
        let target = TempDir::new("tree-gone-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        // The folder lost a file while the app was running.
        fs::remove_file(source.file("accounts/2026.bean")).unwrap();

        let tree = LocalTree::new(source.dir());
        let recording = Recording {
            inner: &tree,
            writes: RefCell::new(Vec::new()),
        };
        let report = sync_back(&recording, &mirror).unwrap();

        // The loaded ledger still has the file, so the write pass neither deletes
        // the mirror's copy (a reload the reader never asked for) nor writes
        // anything: it reports the deletion, which is what offers the reload.
        assert_eq!(report.external, ["accounts/2026.bean"]);
        assert!(report.removed.is_empty(), "{report:?}");
        assert!(mirror.join("accounts/2026.bean").exists());
        assert!(recording.writes.borrow().is_empty());
    }

    #[test]
    fn taking_the_folders_side_settles_a_deleted_file() {
        let source = tree("tree-resolve-source");
        let target = TempDir::new("tree-resolve-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        let tree = LocalTree::new(source.dir());

        // Edited here, deleted there: the one state a reader has to settle.
        let edited = "2026-01-02 * \"Coffee\"\n  Expenses:Food 9.00 USD\n  Assets:Bank\n";
        fs::write(mirror.join("accounts/2026.bean"), edited).unwrap();
        fs::remove_file(source.file("accounts/2026.bean")).unwrap();
        let conflicted = sync_back(&tree, &mirror).unwrap();
        assert_eq!(conflicted.conflicts.len(), 1, "{conflicted:?}");

        // Taking the folder's answer is the deletion, and it clears.
        let settled = resolve_conflicts(&tree, &mirror, Resolve::Folder).unwrap();

        assert!(settled.conflicts.is_empty(), "{settled:?}");
        assert!(!mirror.join("accounts/2026.bean").exists());
        assert!(!source.file("accounts/2026.bean").exists());
        // And the state is stable: nothing to report on the next pass.
        assert!(sync_back(&tree, &mirror).unwrap().is_empty());
    }

    #[test]
    fn keeping_this_sides_bytes_writes_them_home() {
        let source = tree("tree-keep-source");
        let target = TempDir::new("tree-keep-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        let tree = LocalTree::new(source.dir());

        let remote = "2026-01-02 * \"Tea\"\n  Expenses:Food 2.00 USD\n  Assets:Bank\n";
        fs::write(source.file("accounts/2026.bean"), remote).unwrap();
        let local = "2026-01-02 * \"Coffee\"\n  Expenses:Food 7.00 USD\n  Assets:Bank\n";
        fs::write(mirror.join("accounts/2026.bean"), local).unwrap();
        assert_eq!(sync_back(&tree, &mirror).unwrap().conflicts.len(), 1);

        let settled = resolve_conflicts(&tree, &mirror, Resolve::Mine).unwrap();

        assert!(settled.conflicts.is_empty(), "{settled:?}");
        assert_eq!(
            fs::read_to_string(source.file("accounts/2026.bean")).unwrap(),
            local
        );
        assert!(sync_back(&tree, &mirror).unwrap().is_empty());
    }

    #[test]
    fn a_second_edit_after_a_push_still_goes_home() {
        let source = tree("tree-twice-source");
        let target = TempDir::new("tree-twice-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        let tree = LocalTree::new(source.dir());

        // First edit: written home, so the base must become these bytes — not
        // the folder's snapshot from before the write.
        let first = "2026-01-02 * \"Coffee\"\n  Expenses:Food 4.00 USD\n  Assets:Bank\n";
        fs::write(mirror.join("accounts/2026.bean"), first).unwrap();
        assert_eq!(
            sync_back(&tree, &mirror).unwrap().pushed,
            ["accounts/2026.bean"]
        );

        // Second edit to the same file: still the app's own change.
        let second = "2026-01-02 * \"Coffee\"\n  Expenses:Food 5.00 USD\n  Assets:Bank\n";
        fs::write(mirror.join("accounts/2026.bean"), second).unwrap();
        let report = sync_back(&tree, &mirror).unwrap();

        assert_eq!(report.pushed, ["accounts/2026.bean"], "{report:?}");
        assert!(report.conflicts.is_empty(), "{report:?}");
        assert_eq!(
            fs::read_to_string(source.file("accounts/2026.bean")).unwrap(),
            second
        );
    }

    #[test]
    fn an_external_change_is_never_pushed_over_later() {
        let source = tree("tree-external-source");
        let target = TempDir::new("tree-external-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        let tree = LocalTree::new(source.dir());

        // Another device writes the nested file; this app has no edit of its own.
        let remote = "2026-01-02 * \"Tea\"\n  Expenses:Food 2.00 USD\n  Assets:Bank\n";
        fs::write(source.file("accounts/2026.bean"), remote).unwrap();
        let first = sync_back(&tree, &mirror).unwrap();
        assert_eq!(first.external, ["accounts/2026.bean"]);
        assert!(first.pushed.is_empty(), "{first:?}");

        // A later pass must still report it — the base cannot have moved to the
        // folder's version, or the mirror's stale copy would be written home.
        let second = sync_back(&tree, &mirror).unwrap();
        assert_eq!(second.external, ["accounts/2026.bean"], "{second:?}");
        assert!(second.pushed.is_empty(), "{second:?}");
        assert_eq!(
            fs::read_to_string(source.file("accounts/2026.bean")).unwrap(),
            remote
        );

        // Opening reconciles it: the folder's version replaces the mirror's.
        let opened = mirror_into(&tree, &mirror).unwrap();
        assert_eq!(opened.pulled, ["accounts/2026.bean"]);
        assert_eq!(
            fs::read_to_string(mirror.join("accounts/2026.bean")).unwrap(),
            remote
        );
    }

    #[test]
    fn a_file_both_sides_changed_is_reported_and_left_alone() {
        let source = tree("tree-both-source");
        let target = TempDir::new("tree-both-target");
        let mirror = target.file("mirror");
        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();

        let remote = "2026-01-02 * \"Tea\"\n  Expenses:Food 2.00 USD\n  Assets:Bank\n";
        fs::write(source.file("accounts/2026.bean"), remote).unwrap();
        let local = "2026-01-02 * \"Coffee\"\n  Expenses:Food 9.00 USD\n  Assets:Bank\n";
        fs::write(mirror.join("accounts/2026.bean"), local).unwrap();

        let report = sync_back(&LocalTree::new(source.dir()), &mirror).unwrap();

        assert!(report.pushed.is_empty(), "{report:?}");
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].path, "accounts/2026.bean");
        // Neither side was written, and the conflict is remembered: the next
        // pass reports it again instead of inventing a winner.
        assert_eq!(
            fs::read_to_string(source.file("accounts/2026.bean")).unwrap(),
            remote
        );
        assert_eq!(
            fs::read_to_string(mirror.join("accounts/2026.bean")).unwrap(),
            local
        );
        let again = sync_back(&LocalTree::new(source.dir()), &mirror).unwrap();
        assert_eq!(again.conflicts.len(), 1, "{again:?}");
    }

    #[test]
    fn version_control_metadata_is_never_mirrored_or_written() {
        let source = tree("tree-git-source");
        let target = TempDir::new("tree-git-target");
        let mirror = target.file("mirror");
        fs::create_dir_all(source.file(".git/refs")).unwrap();
        fs::write(source.file(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        mirror_into(&LocalTree::new(source.dir()), &mirror).unwrap();
        assert!(!mirror.join(".git").exists());

        // And a repository the mirror somehow gained is not pushed back either.
        fs::create_dir_all(mirror.join(".git")).unwrap();
        fs::write(mirror.join(".git/HEAD"), "ref: refs/heads/other\n").unwrap();
        let tree = LocalTree::new(source.dir());
        let recording = Recording {
            inner: &tree,
            writes: RefCell::new(Vec::new()),
        };
        sync_back(&recording, &mirror).unwrap();

        assert!(
            recording.writes.borrow().is_empty(),
            "{:?}",
            recording.writes.borrow()
        );
        assert_eq!(
            fs::read_to_string(source.file(".git/HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
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
        let report = sync_back(&recording, &mirror).unwrap();

        assert_eq!(report.pushed, ["accounts/2026.bean"]);
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

        let report = sync_back(&LocalTree::new(source.dir()), &mirror).unwrap();

        assert_eq!(report.pushed, ["accounts/2027.bean"]);
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
        let report = sync_back(&recording, &mirror).unwrap();

        assert!(report.is_empty(), "{report:?}");
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
