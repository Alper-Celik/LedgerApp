// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The operation log: for every write to the ledger, who wrote it and what the
//! file looked like on both sides, so an AI edit — or any edit — can be listed,
//! undone, and reconciled when the ledger reaches a second device through a
//! file sync instead of git.
//!
//! Git cannot be the answer for every storage mode: a phone ledger lives in a
//! Storage Access Framework tree only the app can write, and the user syncs it
//! themselves (Drive, Syncthing). So the log lives *inside* the ledger — it has
//! to travel with it, or the peer device would have no idea what we did — and
//! never talks to a remote.
//!
//! Layout under the caller's `root` (the ledger's `.ledger/log`):
//!
//! ```text
//! root/
//!   device                 this device's stable random id, created on first use
//!   <device>.jsonl         one JSON line per operation, appended, never rewritten
//!   blobs/<hash>           pre- and post-images by BLAKE3 hash (dedupe is free)
//!   state.json             what each device last saw per path (a sync detector's
//!                          comparison map; conflict bases read the blobs instead)
//!   .lock-<device>         held while one line is appended, or the log prunes
//! ```
//!
//! One file per device is what makes the sync survivable: two devices never
//! write the same `.jsonl`, so a merge is a union of lines rather than a
//! conflict. Only two processes on the *same* device can collide, and they
//! serialise on that device's own lock file — a lock synced from a peer names
//! the peer's id and is never waited on here.
//!
//! A line is the operation itself:
//!
//! ```json
//! { "id": "2026-10-29T12-34-56Z-1a2b-0000", "at": "…Z", "device": "…",
//!   "origin": "ai", "summary": "assistant edit",
//!   "ops": [{ "path": "…/main.bean", "before_hash": "…", "after_hash": "…",
//!             "hunks": [{ "at": "…", "removed": 1, "added": 1 }] }] }
//! ```
//!
//! Paths are whatever the ledger has — this module imposes no folder structure
//! on it, and never writes to it. [`revert`] and [`revert_plan`] hand the
//! caller the bytes to put back and refuse when the file no longer holds what
//! the operation left there, so an undo cannot clobber a later edit — ours or a
//! peer's.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The log's own id, so a reader can tell which device wrote an operation.
const DEVICE_FILE: &str = "device";
/// The per-device last-seen hashes conflict detection starts from. One file per
/// device: the log is synced, and a whole-file rewrite by two devices would drop
/// whichever section the other read first.
const STATE_PREFIX: &str = "state-";
/// The single file every device wrote before the split, still read as a fallback.
const LEGACY_STATE_FILE: &str = "state.json";
/// Pre- and post-images, named by their hash.
const BLOB_DIR: &str = "blobs";
/// Held while one process on this device appends or prunes. One per device:
/// the log is synced, and a peer's lock must never make this device wait.
const LOCK_PREFIX: &str = ".lock-";
/// How long an append waits for the lock before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// A lock this old was left by a process that died holding it.
const LOCK_STALE: Duration = Duration::from_secs(60);

/// Who made the write. The log is only useful if a reader can tell an
/// assistant's edit from the user's own, so the writer names itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    User,
    Ai,
    Importer,
}

/// One line the writer changed, in the terms a merge needs: the line hash it
/// was applied at (before the edit) and how many lines it took and made.
///
/// Only the tools that edit lines know this; a whole-file write passes `None`,
/// and the operation is still exact — the hashes above it say everything a
/// revert needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Hunk {
    /// The hash of the line the edit was anchored at, before the edit.
    pub at: String,
    pub removed: u32,
    pub added: u32,
}

/// What a caller hands [`record`] for one file: the bytes on each side of the
/// write, and the hunks when the writer knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The ledger path exactly as the caller names it; recorded verbatim.
    pub path: PathBuf,
    /// `None` when the file did not exist before the write.
    pub before: Option<Vec<u8>>,
    /// `None` when the write removed the file.
    pub after: Option<Vec<u8>>,
    pub hunks: Option<Vec<Hunk>>,
}

impl Change {
    /// A change with no hunks — what a whole-file write knows.
    pub fn new(path: impl Into<PathBuf>, before: Option<&[u8]>, after: Option<&[u8]>) -> Self {
        Self {
            path: path.into(),
            before: before.map(|bytes| bytes.to_vec()),
            after: after.map(|bytes| bytes.to_vec()),
            hunks: None,
        }
    }

    /// The same change with the line hunks the writer worked out. Nothing calls
    /// this yet: the line tools attach hunks once they edit through the pipeline,
    /// and the log already carries the field for them.
    #[allow(dead_code)]
    pub fn with_hunks(mut self, hunks: Vec<Hunk>) -> Self {
        self.hunks = Some(hunks);
        self
    }
}

/// One file of an operation, as it is stored.
///
/// `path` is the ledger path exactly as the writer named it — the path
/// [`revert`] hands back, so undo lands on the same file even if the ledger is
/// opened from somewhere else. The hashes are the authority: `None` means the
/// file did not exist on that side of the write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FileOp {
    pub path: PathBuf,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub hunks: Option<Vec<Hunk>>,
}

/// One write, as the log remembers it: one line of `<device>.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Operation {
    /// Unique across devices: `2026-10-29T12-34-56Z-1a2b-0000`.
    pub id: String,
    /// RFC 3339 in UTC with nanoseconds, e.g. `2026-10-29T12:34:56.123456789Z`.
    /// Newest-first order is this field, so it must sort like a timestamp.
    pub at: String,
    /// The device that wrote it: this log's `device` file.
    pub device: String,
    pub origin: Origin,
    /// What the write was, in the writer's words ("assistant edit", "import
    /// from bank.csv"). Shown in the history list, never parsed.
    pub summary: String,
    pub ops: Vec<FileOp>,
}

/// Which operations a caller wants to see. [`Filter::default`] is everything.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub origin: Option<Origin>,
    /// Only operations that touched this ledger path, exactly as recorded.
    pub path: Option<PathBuf>,
    /// The newest `limit` matching operations; `0` means no limit.
    pub limit: usize,
}

impl Filter {
    /// Whether `operation` is one this filter shows: its origin, when named,
    /// and one of its files, when a path is named.
    ///
    /// The limit is deliberately not part of this test — it says how many
    /// matches to keep, not which operations match — so a reader can apply the
    /// filter while it reads and keep only the newest `limit` of the matches.
    fn accepts(&self, operation: &Operation) -> bool {
        self.origin.is_none_or(|origin| operation.origin == origin)
            && self
                .path
                .as_ref()
                .is_none_or(|path| operation.ops.iter().any(|file| &file.path == path))
    }
}

/// How much of the log's images to keep.
///
/// The log lines themselves are never deleted — they are the record of what
/// happened, and a peer may still need to see them — so retention is about the
/// blobs, which are what actually fill the disk. Every field is a ceiling that
/// only applies when nonzero, except the two `keep_newest` windows, which are
/// always kept: the newest few operations stay whatever the limits say, so do
/// the newest few AI operations (the ones a user most wants back after a bad
/// assistant edit), and so does the newest operation that touched each path,
/// because that is the merge base a peer's version gets compared against.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    /// Keep images for at most this many operations; `0` = no ceiling.
    pub max_operations: usize,
    /// Keep at most this many bytes of images; `0` = no ceiling.
    pub max_bytes: u64,
    /// The newest this many operations are never pruned.
    pub keep_newest: usize,
    /// The newest this many `ai` operations are never pruned.
    pub keep_newest_ai: usize,
}

impl Default for Retention {
    /// A log bounded enough to sit beside the ledger: images for a couple of
    /// hundred operations or a few tens of megabytes, with the last ten writes,
    /// the last twenty AI writes, and every path's merge base pinned.
    fn default() -> Self {
        Self {
            max_operations: 200,
            max_bytes: 64 * 1024 * 1024,
            keep_newest: 10,
            keep_newest_ai: 20,
        }
    }
}

/// What reverting one file means: put these bytes back, or — when `before` is
/// `None` — the file did not exist before the operation, so the way back is to
/// delete it. [`revert`] carries only the bytes; this shape is what reports
/// creations, which have no bytes to return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertEntry {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
}

/// Record one write from this device.
///
/// Under `.lock`: the images go to `blobs/`, one line is appended to
/// `<device>.jsonl`, then `state.json` is updated — in that order, so a reader
/// that sees the line finds everything it names, and a prune cannot delete an
/// image this write is about to name. The lock is per log directory, so only
/// two processes on this device contend; the append is one write of a whole
/// line, and only this device writes that file, so no reader ever sees half an
/// operation.
///
/// Changes whose two sides are byte-identical are dropped: the edit funnel can
/// reach here with text equal to what is on disk, and that is not an operation.
/// An operation left with nothing to say is refused rather than recorded.
///
/// A recorded write also gives retention its production caller: once the image
/// store has outgrown [`Retention::default`] by the margin [`over_retention`]
/// describes, the write prunes it back. The check is a directory listing, so an
/// ordinary edit stays cheap; a prune that fails is reported on stderr and never
/// fails the write that already landed.
pub fn record(
    root: &Path,
    origin: Origin,
    summary: &str,
    changes: &[Change],
) -> Result<Operation, String> {
    let mut kept = Vec::with_capacity(changes.len());
    let mut seen = HashSet::new();
    for change in changes {
        if change.before.as_deref() == change.after.as_deref() {
            continue;
        }
        if !seen.insert(change.path.clone()) {
            return Err(format!(
                "{} appears twice in one operation",
                change.path.display()
            ));
        }
        kept.push(change);
    }
    if kept.is_empty() {
        return Err("nothing to record: every file is unchanged".to_string());
    }

    let device = device_id(root)?;
    // The lock covers the images as well as the line: `prune` holds it too, so
    // it cannot delete an image this write is about to name.
    let lock = Lock::acquire(root, &device)?;
    let blobs = root.join(BLOB_DIR);
    fs::create_dir_all(&blobs).map_err(|error| format!("{}: {error}", blobs.display()))?;

    let mut ops = Vec::with_capacity(kept.len());
    for change in kept {
        // Both sides first: an image is what makes the line revertible and, for
        // a path that moved on, the base a merge compares against.
        let before_hash = match change.before.as_deref() {
            Some(bytes) => Some(write_blob(&blobs, bytes)?),
            None => None,
        };
        let after_hash = match change.after.as_deref() {
            Some(bytes) => Some(write_blob(&blobs, bytes)?),
            None => None,
        };
        ops.push(FileOp {
            path: change.path.clone(),
            before_hash,
            after_hash,
            hunks: change.hunks.clone(),
        });
    }

    let (at, stem) = timestamp_parts(SystemTime::now());
    let id = format!("{stem}-{}-{:04x}", device_short(&device), next_id_part());
    let operation = Operation {
        id,
        at,
        device,
        origin,
        summary: summary.to_string(),
        ops,
    };

    append_line(root, &operation)?;
    remember(root, &operation)?;
    // The write is on disk; the lock is only needed for it — `prune` takes the
    // lock itself, so it could not run while this one is held.
    drop(lock);

    // Retention runs here, where writes actually happen: the log has no other
    // production caller, and images are what fill the disk. It is skipped while
    // the store is inside its ceiling — reading the directory is the whole
    // check — so an ordinary edit pays for a directory listing, not for the
    // parse, sort and blob walk a prune costs. A prune that fails must not fail
    // a write that already landed.
    let retention = Retention::default();
    if over_retention(root, retention)
        && let Err(error) = prune(root, retention)
    {
        eprintln!("history: prune skipped: {error}");
    }
    Ok(operation)
}

/// Operations in the log, newest first, filtered and limited.
///
/// A line whose JSON cannot be parsed is skipped and reported once — one line
/// on stderr naming the file and line number — rather than failing the list: a
/// half-synced `.jsonl` should still list the rest. [`list_with_issues`]
/// returns the same reports without printing them, for callers that surface
/// them in their own UI.
pub fn list(root: &Path, filter: Filter) -> Result<Vec<Operation>, String> {
    let (operations, issues) = list_with_issues(root, filter)?;
    for issue in &issues {
        eprintln!("history: {issue}");
    }
    Ok(operations)
}

/// [`list`], with the skipped lines returned instead of printed.
///
/// The filter is applied while the log is read, and each device's log lends at
/// most the newest `limit` matches to the answer — a bounded selection, kept in
/// the log's own order, so a page of a log holding years of writes costs the
/// reads plus `limit · log limit` comparisons rather than a sort of every
/// operation ever recorded. Devices are merged before the final truncation, so
/// the answer is the same newest `limit` matches across the whole log.
pub fn list_with_issues(
    root: &Path,
    filter: Filter,
) -> Result<(Vec<Operation>, Vec<String>), String> {
    let (mut operations, issues, _torn) = read_operations(root, Some(&filter))?;
    // Each device's candidates are already newest first; sorting the merged
    // (at most devices × limit) list puts the devices' clocks in one order.
    sort_newest_first(&mut operations);
    if filter.limit > 0 {
        operations.truncate(filter.limit);
    }
    Ok((operations, issues))
}

/// The bytes to put back to undo operation `id`, one entry per file that
/// existed before it — the files the operation *created* have nothing to put
/// back and are not in the result; [`revert_plan`] reports those as `None`.
///
/// This only reads. It refuses, before returning anything, when a file no
/// longer holds what the operation left there: `"<path> changed since operation
/// <id>"`, so an undo cannot clobber an edit made after it, local or synced. A
/// file the operation deleted is expected to be absent, and any file that
/// reappeared in its place is the same refusal.
#[cfg(test)]
pub fn revert(root: &Path, id: &str) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    Ok(revert_plan(root, id)?
        .into_iter()
        .filter_map(|entry| entry.before.map(|bytes| (entry.path, bytes)))
        .collect())
}

/// The whole undo of operation `id`: every file it touched, with the bytes to
/// write back (`Some`) or `None` for a file the operation created, which undo
/// removes. Same checks as [`revert`].
pub fn revert_plan(root: &Path, id: &str) -> Result<Vec<RevertEntry>, String> {
    let operation = find(root, id)?;
    let blobs = root.join(BLOB_DIR);

    let mut entries = Vec::with_capacity(operation.ops.len());
    for file in &operation.ops {
        // The path as this device spells it: another device recorded its own.
        let path = local_path(root, &file.path).unwrap_or_else(|| file.path.clone());
        // The file must still be what this operation left behind.
        match file.after_hash.as_deref() {
            Some(expected) => {
                let current = fs::read(&path).map_err(|error| {
                    format!("{} changed since operation {id}: {error}", path.display())
                })?;
                if hash_of(&current) != expected {
                    return Err(format!(
                        "{} changed since operation {id}",
                        file.path.display()
                    ));
                }
            }
            None => {
                if path.exists() {
                    return Err(format!("{} changed since operation {id}", path.display()));
                }
            }
        }

        let before = match file.before_hash.as_deref() {
            Some(expected) => {
                let stored = blobs.join(expected);
                let bytes = fs::read(&stored).map_err(|error| {
                    format!(
                        "{}: {error} (the pre-image of {} may have been pruned)",
                        stored.display(),
                        file.path.display()
                    )
                })?;
                if hash_of(&bytes) != expected {
                    return Err(format!(
                        "{} no longer holds what operation {id} recorded",
                        stored.display()
                    ));
                }
                Some(bytes)
            }
            None => None,
        };
        entries.push(RevertEntry { path, before });
    }
    Ok(entries)
}

/// The newest pre-image the log holds for `path`: the `before` blob of the
/// newest operation that touched it.
///
/// That is the merge base a conflicted copy from another device is compared
/// against (see `crate::conflict_cmd`): the log travels inside the ledger, so
/// the version this ledger had before its own last write is the closest thing
/// to "what both devices started from" that exists without git.
///
/// `Ok(None)` when no operation touched the path, or when the newest one
/// *created* it — before that write there was nothing to compare against. A
/// blob that is missing or no longer hashes to its name is an error rather than
/// `None`: the log lines are still there, so an empty base would be a claim
/// ("nothing came before") this answer cannot make.
///
/// Paths are matched as the writers named them and, failing that, by their
/// canonical form, so a ledger opened through a symlinked folder still finds
/// the images its own writes recorded.
pub fn pre_image_for(root: &Path, path: &Path) -> Result<Option<Vec<u8>>, String> {
    let (operations, _issues) = list_with_issues(root, Filter::default())?;
    // Newest first, and an operation lists a path at most once (`record`
    // refuses a duplicate), so the first match is the newest operation's own.
    let Some(file) = operations
        .iter()
        .flat_map(|operation| &operation.ops)
        .find(|file| same_path(&file.path, path))
        .or_else(|| {
            // A peer recorded its own spelling; both name one file when their
            // ledger-relative tails match.
            operations
                .iter()
                .flat_map(|operation| &operation.ops)
                .find(|file| match local_path(root, &file.path) {
                    Some(local) => same_path(&local, path),
                    None => false,
                })
        })
    else {
        return Ok(None);
    };
    let Some(hash) = file.before_hash.as_deref() else {
        return Ok(None);
    };
    let stored = root.join(BLOB_DIR).join(hash);
    let bytes = fs::read(&stored).map_err(|error| {
        format!(
            "{}: {error} (the pre-image of {} may have been pruned)",
            stored.display(),
            path.display()
        )
    })?;
    if hash_of(&bytes) != hash {
        return Err(format!(
            "{} no longer holds the pre-image of {} it recorded",
            stored.display(),
            path.display()
        ));
    }
    Ok(Some(bytes))
}

/// Whether two spellings name the same file. The log records paths exactly as
/// the writer named them, so an absolute path and a relative one — or a
/// symlinked folder and its target — have to compare equal, or a merge base
/// would silently go missing.
fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// The ledger directory a log directory belongs to (`<ledger>/.ledger/log`).
fn ledger_of(root: &Path) -> Option<&Path> {
    root.parent()?.parent()
}

/// A recorded path as it names a file on *this* machine.
///
/// The log travels with the ledger, and it records paths as the writer named
/// them: a phone records its install-private mirror, which no other device has.
/// The ledger-relative tail of the path is the part every device shares, so it is
/// resolved against this device's own ledger directory. An existing path is
/// returned untouched.
fn local_path(root: &Path, recorded: &Path) -> Option<PathBuf> {
    if recorded.exists() {
        return Some(recorded.to_path_buf());
    }
    let ledger = ledger_of(root)?;
    let parts: Vec<_> = recorded.components().collect();
    for start in 0..parts.len() {
        let mut candidate = ledger.to_path_buf();
        for part in &parts[start..] {
            candidate.push(part.as_os_str());
        }
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Delete the images past the limits, returning the bytes freed.
///
/// The lines stay: an operation is a fact, and a peer that syncs tomorrow still
/// needs to see it. What goes is `blobs/`: every image no kept operation
/// refers to. Kept, always, are the newest [`Retention::keep_newest`]
/// operations, the newest [`Retention::keep_newest_ai`] AI operations, and the
/// newest operation that touched each path — that last one is the merge base
/// conflict detection compares a peer's version against, so pruning below it
/// would leave a conflict with nothing to compare.
///
/// An operation whose line cannot be parsed is not protected as an operation,
/// but the hashes the torn bytes still spell out are: a sync stopped half-way
/// through a peer's line must not cost that peer its pre-image when the rest
/// arrives.
///
/// The prune runs under the same lock as an append, so it never reads a line
/// this device is in the middle of writing or deletes an image a write that has
/// not landed yet is about to name.
pub fn prune(root: &Path, retention: Retention) -> Result<u64, String> {
    prune_after(root, retention, sync_grace())
}

/// A prune that leaves unreferenced images younger than `grace` alone.
fn prune_after(root: &Path, retention: Retention, grace: Duration) -> Result<u64, String> {
    // Nothing to reclaim, and pruning should not create the log: the first
    // write owns that.
    if !root.exists() {
        return Ok(0);
    }
    let _lock = Lock::acquire(root, &device_id(root)?)?;
    let (mut operations, _issues, torn) = read_operations(root, None)?;
    sort_newest_first(&mut operations);
    let blobs = root.join(BLOB_DIR);

    let mut protected = vec![false; operations.len()];
    for flag in protected.iter_mut().take(retention.keep_newest) {
        *flag = true;
    }
    let mut kept_ai = 0;
    for (index, operation) in operations.iter().enumerate() {
        if operation.origin == Origin::Ai && kept_ai < retention.keep_newest_ai {
            protected[index] = true;
            kept_ai += 1;
        }
    }
    let mut paths = HashSet::new();
    for (index, operation) in operations.iter().enumerate() {
        for file in &operation.ops {
            if paths.insert(file.path.clone()) {
                protected[index] = true;
            }
        }
    }

    // Walk newest first, keeping an operation when it is protected or still
    // fits the ceilings. The images are cumulative, so an operation that does
    // not fit is skipped but the walk goes on: an older one may reference
    // nothing new at all.
    let mut referenced: HashSet<String> = HashSet::new();
    let mut kept_bytes = 0u64;
    for line in &torn {
        for hash in salvage_hashes(line) {
            if referenced.insert(hash.clone()) {
                kept_bytes += blob_size(&blobs, &hash);
            }
        }
    }
    let mut kept_operations = 0;
    for (index, operation) in operations.iter().enumerate() {
        let must_keep = protected[index];
        if !must_keep && retention.max_operations > 0 && kept_operations >= retention.max_operations
        {
            continue;
        }
        let mut added = Vec::new();
        let mut added_bytes = 0;
        for hash in hashes_of(operation) {
            if !referenced.contains(hash) {
                added_bytes += blob_size(&blobs, hash);
                added.push(hash.to_string());
            }
        }
        if !must_keep && retention.max_bytes > 0 && kept_bytes + added_bytes > retention.max_bytes {
            continue;
        }
        referenced.extend(added);
        kept_bytes += added_bytes;
        kept_operations += 1;
    }

    // Unreferenced images are garbage: their operations lost the retention
    // race, or their line is gone.
    let entries = match fs::read_dir(&blobs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("{}: {error}", blobs.display())),
    };
    let mut freed = 0;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", blobs.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if referenced.contains(&name) {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        if !kind.is_file() {
            continue;
        }
        // An image whose operation has not arrived yet is indistinguishable from
        // garbage: a sync client may deliver the blobs before the lines that
        // name them, so an image younger than a round trip is left for a later
        // prune rather than deleted under the peer's feet. The ceilings are
        // approximate; the peer's pre-image is not.
        let metadata = entry
            .metadata()
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        if metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < grace)
        {
            continue;
        }
        let size = metadata.len();
        fs::remove_file(entry.path())
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        freed += size;
    }
    Ok(freed)
}

/// How far past [`Retention::max_operations`] the log may grow before a write
/// pays for a prune: a quarter over, so a burst of writes is cleaned in one
/// pass instead of every write cleaning up after itself.
const PRUNE_MARGIN: (u64, u64) = (5, 4);

/// How many images one operation can name: its before and its after. The
/// retention counts operations; the store it prunes counts images.
const IMAGES_PER_OPERATION: u64 = 2;

/// Whether the log's images have outgrown what `retention` keeps, by the margin
/// a write lets them run over before it prunes.
///
/// The measure is the image store, not the log's lines. A prune never deletes a
/// line — they are the record of what happened — so a line count would sit over
/// the ceiling for the rest of the ledger's life and make every write parse the
/// whole log, which is the scan this check exists to avoid. Images are what
/// pruning reclaims, and they fall back under the ceiling once it has run, so
/// the writes that follow it stay cheap. Each operation names up to two of them
/// (the file's before and its after), which is how the retention's ceiling in
/// operations becomes one in images; the listing stops as soon as the ceiling
/// is crossed, so its cost is bounded by that ceiling, not by the store.
fn over_retention(root: &Path, retention: Retention) -> bool {
    let (numerator, denominator) = PRUNE_MARGIN;
    let ceiling = (retention.max_operations as u64)
        .saturating_mul(numerator)
        .saturating_mul(IMAGES_PER_OPERATION)
        / denominator;
    if ceiling == 0 {
        return false;
    }
    let Ok(entries) = fs::read_dir(root.join(BLOB_DIR)) else {
        return false;
    };
    let mut images = 0u64;
    for entry in entries.flatten() {
        // Dot files are this module's bookkeeping, never an image.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        images += 1;
        if images > ceiling {
            return true;
        }
    }
    false
}

/// This device's id, creating it on first use.
///
/// Stable and random rather than a hostname: two machines can share a name, and
/// the id is also what keeps two devices' operation ids apart.
/// Whether `id` is the shape [`device_id`] generates: 16 lowercase hex.
fn is_device_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn device_id(root: &Path) -> Result<String, String> {
    // The id names a file *and* a lock inside the log directory, and the file is
    // synced: a peer could hand us a traversal in it. Anything that is not the
    // shape this function generates is replaced with a fresh id.

    let path = root.join(DEVICE_FILE);
    let mut empty_reads = 0;
    for _ in 0..200 {
        match fs::read_to_string(&path) {
            Ok(text) => {
                let id = text.trim();
                if !id.is_empty() {
                    // Generated ids are 16 lowercase hex characters. Anything
                    // else (a synced file from a peer, a hand edit) is not a
                    // filename this app may use: replace it rather than splice a
                    // path out of it.
                    if is_device_id(id) {
                        return Ok(id.to_string());
                    }
                    let _ = fs::remove_file(&path);
                    continue;
                }
                // Another process creates the file and writes the id in the
                // next breath, so an empty read is almost certainly that gap —
                // take the name back only if it stays empty far longer than a
                // write takes, which means the write died.
                empty_reads += 1;
                if empty_reads > 25 {
                    let _ = fs::remove_file(&path);
                    empty_reads = 0;
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }

        fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
        let id = random_id();
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                writeln!(file, "{id}").map_err(|error| format!("{}: {error}", path.display()))?;
                return Ok(id);
            }
            // Another process on this device won the race: its id is ours too.
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    Err(format!(
        "{}: could not read or create a device id",
        path.display()
    ))
}

/// What this device last saw for each path: the hash of the version, or `None`
/// when the path was last known to be gone.
///
/// Conflict detection is then "the file on disk hashes to neither of these" —
/// neither what we last saw nor what our newest operation left there. A state
/// file that cannot be parsed reads as empty: it is a cache, and the log lines
/// are the record.
pub fn last_seen(root: &Path) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    let device = device_id(root)?;
    let state = read_state(root, &device);
    Ok(state
        .devices
        .get(&device)
        .map(|known| {
            known
                .paths
                .iter()
                .map(|(path, seen)| (PathBuf::from(path), seen.hash.clone()))
                .collect()
        })
        .unwrap_or_default())
}

/// One operation, by id, from whichever device's log holds it.
fn find(root: &Path, id: &str) -> Result<Operation, String> {
    let (operations, _issues, _torn) = read_operations(root, None)?;
    let mut matches = operations
        .into_iter()
        .filter(|operation| operation.id == id);
    let Some(operation) = matches.next() else {
        return Err(format!("no operation {id} under {}", root.display()));
    };
    if matches.next().is_some() {
        // Two devices picked the same id (the same second, the same suffix);
        // guessing which one the caller meant would be worse than refusing.
        return Err(format!("more than one operation has id {id}"));
    }
    Ok(operation)
}

/// Every operation in every device's log, with one report per unparseable line
/// and the raw bytes of those lines: their hashes still name images that must
/// not be pruned while the rest of the line is on its way from a peer.
///
/// `filter` narrows what is kept, and its limit bounds each device's share: a
/// device's log contributes at most its newest `limit` matches, held in a heap
/// of that size rather than in a copy of the log. `None` keeps everything, which
/// is what [`find`] and [`prune`] need. Every line is still parsed — the origin
/// and path a filter matches on live inside the operation, and a torn line has
/// to be reported wherever it falls — but nothing past the bound is retained.
fn read_operations(
    root: &Path,
    filter: Option<&Filter>,
) -> Result<(Vec<Operation>, Vec<String>, Vec<Vec<u8>>), String> {
    let limit = filter.and_then(|filter| (filter.limit > 0).then_some(filter.limit));
    let mut operations = Vec::new();
    let mut issues = Vec::new();
    let mut torn = Vec::new();
    for path in log_files(root)? {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            // Vanished between the scan and the read: nothing to report.
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                issues.push(format!("skipped {}: {error}", path.display()));
                continue;
            }
        };
        // This device's newest matches, oldest at the top of the heap so the
        // one a bounded selection must drop is the one `pop` reaches.
        let mut newest: BinaryHeap<Oldest> = BinaryHeap::new();
        // Split on bytes, not lines of a `String`: a torn sync can leave one
        // line that is not UTF-8, and that line alone should be skipped.
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            let line = line.trim_ascii();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<Operation>(line) {
                Ok(operation) => {
                    if filter.is_some_and(|filter| !filter.accepts(&operation)) {
                        continue;
                    }
                    match limit {
                        // A full selection: the newcomer takes the oldest
                        // slot only when it is newer than what holds it.
                        Some(limit) if newest.len() >= limit => {
                            let newer = newest.peek().is_some_and(|oldest| {
                                compare_newest_first(&operation, &oldest.0).is_lt()
                            });
                            if newer {
                                newest.pop();
                                newest.push(Oldest(operation));
                            }
                        }
                        Some(_) => newest.push(Oldest(operation)),
                        None => operations.push(operation),
                    }
                }
                Err(error) => {
                    issues.push(format!("skipped {}:{}: {error}", path.display(), index + 1));
                    torn.push(line.to_vec());
                }
            }
        }
        // Ascending in the log's order is newest first, which is how the
        // candidates join the answer.
        operations.extend(newest.into_sorted_vec().into_iter().map(|entry| entry.0));
    }
    Ok((operations, issues, torn))
}

/// The device logs under `root`, by name.
fn log_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        // A log that was never written is an empty log, not a failure.
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", root.display())),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", root.display()))?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Dot files are this module's own bookkeeping, never a device log.
        if name.starts_with('.') {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        if kind.is_file() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

/// Newest first. `at` carries the nanoseconds that separate two writes in the
/// same second; `id` and `device` break a tie that even that cannot (two
/// devices writing in the same nanosecond) deterministically. Across devices
/// this order follows each writer's clock, so it is exact within one device and
/// approximate between two — which is all a list needs; revert decides on
/// hashes, not on this order.
fn sort_newest_first(operations: &mut [Operation]) {
    operations.sort_by(compare_newest_first);
}

/// The log's newest-first order, as a comparison: the key [`sort_newest_first`]
/// sorts by, shared so a bounded selection and the final list cannot disagree
/// about which operation is the newest.
fn compare_newest_first(a: &Operation, b: &Operation) -> Ordering {
    b.at.cmp(&a.at)
        .then_with(|| b.id.cmp(&a.id))
        .then_with(|| b.device.cmp(&a.device))
}

/// [`Operation`] under [`compare_newest_first`], for a bounded selection.
///
/// `Ord` is the log's own comparison — newer sorts lower — so a max-heap's top
/// is the *oldest* operation it holds: exactly the one a selection over
/// `limit` slots drops when a newer write arrives. Equality is "sorts the
/// same", which is all a heap asks of it.
struct Oldest(Operation);

impl PartialEq for Oldest {
    fn eq(&self, other: &Self) -> bool {
        compare_newest_first(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for Oldest {}

impl PartialOrd for Oldest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Oldest {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_newest_first(&self.0, &other.0)
    }
}

/// Append one operation to this device's log as a single whole line.
fn append_line(root: &Path, operation: &Operation) -> Result<(), String> {
    let path = root.join(format!("{}.jsonl", operation.device));
    let line = serde_json::to_vec(operation).map_err(|error| format!("operation: {error}"))?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;

    // A tail without its newline is an append the machine or a sync cut in
    // half; joining onto it would swallow this whole operation in one
    // unparseable line, so start a fresh one.
    let mut bytes = Vec::with_capacity(line.len() + 1);
    let len = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len();
    if len > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1))
            .and_then(|_| file.read_exact(&mut last))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if last[0] != b'\n' {
            bytes.push(b'\n');
        }
    }
    bytes.extend_from_slice(&line);
    bytes.push(b'\n');

    // One `write_all` under the lock: nothing else writes this file, so the
    // line is whole before any reader can see it.
    file.write_all(&bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.flush()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(())
}

/// Put `bytes` in `blobs/` under their hash — the same bytes written twice are
/// the same file, so dedupe costs nothing.
fn write_blob(blobs: &Path, bytes: &[u8]) -> Result<String, String> {
    let hash = hash_of(bytes);
    let target = blobs.join(&hash);
    if target.exists() {
        return Ok(hash);
    }
    // Write under a temporary name and rename: a reader sees the whole image or
    // none of it, and a peer writing the same hash writes identical bytes.
    let temp = blobs.join(format!(".tmp-{hash}-{:04x}", next_id_part()));
    fs::write(&temp, bytes).map_err(|error| format!("{}: {error}", temp.display()))?;
    if let Err(error) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(format!("{}: {error}", target.display()));
    }
    Ok(hash)
}

fn blob_size(blobs: &Path, hash: &str) -> u64 {
    fs::metadata(blobs.join(hash))
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn hashes_of(operation: &Operation) -> Vec<&str> {
    let mut hashes = Vec::with_capacity(operation.ops.len() * 2);
    for file in &operation.ops {
        if let Some(hash) = file.before_hash.as_deref() {
            hashes.push(hash);
        }
        if let Some(hash) = file.after_hash.as_deref() {
            hashes.push(hash);
        }
    }
    hashes
}

/// The image names a torn line still spells out: every 64-hex-character run in
/// it. A line cut in the middle of a hash loses that one, which no reader can
/// recover from the bytes that are left.
fn salvage_hashes(line: &[u8]) -> Vec<String> {
    let mut hashes = Vec::new();
    for token in line.split(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase()) {
        if token.len() == 64 {
            if let Ok(hash) = std::str::from_utf8(token) {
                hashes.push(hash.to_string());
            }
        }
    }
    hashes
}

/// What this device last saw per path, per device: the state file's shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// Keyed by device, so two devices syncing the same file keep each other's
    /// sections instead of overwriting the only copy on the other machine.
    #[serde(default)]
    devices: BTreeMap<String, DeviceState>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DeviceState {
    #[serde(default)]
    paths: BTreeMap<String, Seen>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Seen {
    hash: Option<String>,
    /// Which device produced that version.
    device: String,
    at: String,
}

/// This device's state file.
fn state_file(root: &Path, device: &str) -> PathBuf {
    let short: String = device.chars().take(12).collect();
    root.join(format!("{STATE_PREFIX}{short}.json"))
}

/// Read this device's state, or an empty one when it is missing or unreadable —
/// it is a cache, and a stale or lost one costs a conflict report, not data.
///
/// A device that has not written since the split still reads the single file all
/// devices used to share; anything it writes goes to its own file, so a peer's
/// writes can never be dropped by this device's rename.
fn read_state(root: &Path, device: &str) -> State {
    let own = fs::read_to_string(state_file(root, device));
    let legacy = fs::read_to_string(root.join(LEGACY_STATE_FILE));
    own.or(legacy)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Record in `state.json` what this operation left each path at.
fn remember(root: &Path, operation: &Operation) -> Result<(), String> {
    let mut state = read_state(root, &operation.device);
    let device = state.devices.entry(operation.device.clone()).or_default();
    for file in &operation.ops {
        device.paths.insert(
            file.path.to_string_lossy().into_owned(),
            Seen {
                hash: file.after_hash.clone(),
                device: operation.device.clone(),
                at: operation.at.clone(),
            },
        );
    }
    write_state(root, &operation.device, &state)
}

/// Record what `paths` hold now, without inventing an operation.
///
/// A branch switch, a merge finished or a revert rewrites the working tree
/// through git2, which the operation log does not see. `sync::changed` compares
/// the disk against what this device last saw, so without this the app's own
/// write is reported as "changed in this folder outside the app" — and stays
/// reported until something else rewrites the file.
pub fn note_seen(root: &Path, paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let device = device_id(root)?;
    let _lock = Lock::acquire(root, &device)?;
    let mut state = read_state(root, &device);
    let (at, _) = timestamp_parts(SystemTime::now());
    let known = state.devices.entry(device.clone()).or_default();
    for path in paths {
        let hash = fs::read(path).ok().map(|bytes| crate::sync::hash(&bytes));
        known.paths.insert(
            path.to_string_lossy().into_owned(),
            Seen {
                hash,
                device: device.clone(),
                at: at.clone(),
            },
        );
    }
    write_state(root, &device, &state)
}

/// Swap `state.json` in whole: a temp file and a rename, so a peer syncing
/// mid-write never copies half a state file.
fn write_state(root: &Path, device: &str, state: &State) -> Result<(), String> {
    let path = state_file(root, device);
    let temp = root.join(format!(".{STATE_PREFIX}{:04x}.json", next_id_part()));
    let mut json = serde_json::to_vec_pretty(state).map_err(|error| format!("state: {error}"))?;
    json.push(b'\n');
    fs::write(&temp, json).map_err(|error| format!("{}: {error}", temp.display()))?;
    if let Err(error) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("{}: {error}", path.display()));
    }
    Ok(())
}

/// Held for one append. A file rather than an advisory lock: a file survives
/// being synced, copied, and inspected, and `O_EXCL` is the one primitive every
/// platform this app runs on agrees on.
pub(crate) struct Lock {
    path: PathBuf,
}

impl Lock {
    pub(crate) fn acquire(root: &Path, device: &str) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
        let path = lock_path(root, device);
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    // A process that died holding the lock must not wedge the
                    // log forever; an append takes milliseconds, so anything
                    // this old is a corpse.
                    if is_stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(format!("{}: still locked", path.display()));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
    }
}

impl Lock {
    /// The lock a directory's whole-file store (the index series) is written
    /// under.
    ///
    /// Unlike the log's, this one is shared by every process that can reach the
    /// directory — the app and the CLI — because they resolve the same config
    /// directory and a read-modify-write of a shared file is exactly what the
    /// lock exists for.
    pub(crate) fn for_store(dir: &Path) -> Result<Self, String> {
        Self::acquire(dir, "indexes")
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// How long an unreferenced image is left alone: long enough for a sync client
/// to deliver the line that names it after the image itself.
const SYNC_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// The grace a prune in this build applies. Tests run without it, because it is
/// about how a folder syncs, and one test drives it explicitly.
fn sync_grace() -> Duration {
    #[cfg(test)]
    {
        Duration::ZERO
    }
    #[cfg(not(test))]
    {
        SYNC_GRACE
    }
}

/// The lock a device's writers share.
fn lock_path(root: &Path, device: &str) -> PathBuf {
    root.join(format!("{LOCK_PREFIX}{device}"))
}

fn is_stale(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    modified.elapsed().is_ok_and(|age| age > LOCK_STALE)
}

/// The content hash the log compares by: 64 hex characters of BLAKE3, fast
/// enough to hash every image on every write.
fn hash_of(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// A random id for this device: 16 hex characters of OS entropy. The hostname
/// is not it — two machines can share one, and the id must not.
fn random_id() -> String {
    let mut seeded = std::collections::hash_map::RandomState::new().build_hasher();
    seeded.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    seeded.write_u32(std::process::id());
    format!("{:016x}", seeded.finish())
}

/// The id's per-process part. Seeded from the clock so a process that restarts
/// inside the same second does not reuse the ids the previous run wrote, and
/// counting from there so one process never repeats itself.
fn next_id_part() -> u16 {
    static COUNTER: LazyLock<AtomicU32> = LazyLock::new(|| {
        let mut seeded = std::collections::hash_map::RandomState::new().build_hasher();
        seeded.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        seeded.write_u32(std::process::id());
        AtomicU32::new(seeded.finish() as u32)
    });
    (COUNTER.fetch_add(1, AtomicOrdering::Relaxed) & 0xFFFF) as u16
}

fn device_short(device: &str) -> &str {
    device.get(..4).unwrap_or(device)
}

/// The `at` timestamp and the id stem, both UTC: the timestamp has the
/// nanoseconds that order two writes within a second, the stem only the second
/// (it names the id, and a fixed-width stem keeps ids sorted).
fn timestamp_parts(now: SystemTime) -> (String, String) {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = i64::try_from(since.as_secs()).unwrap_or(i64::MAX);
    let nanos = since.subsec_nanos();
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = date_of(days);
    let (hour, minute, second) = (of_day / 3_600, of_day % 3_600 / 60, of_day % 60);
    let at = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z");
    let stem = format!("{year:04}-{month:02}-{day:02}T{hour:02}-{minute:02}-{second:02}Z");
    (at, stem)
}

/// The calendar date `days` days after 1970-01-01 — Howard Hinnant's
/// `civil_from_days`, which counts from a March-based era so leap days fall at
/// the end of a year and the arithmetic has no special case. The crate formats
/// its other timestamp the same way; this module keeps its own copy because
/// that one lives behind a private helper in `conversations`.
fn date_of(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * shifted_month + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    })
    .unwrap_or(1);
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    /// A ledger directory inside `dir` with the file written, so the paths a
    /// write names all sit under one folder the way the app's do.
    fn ledger_file(dir: &TempDir, name: &str, text: &str) -> PathBuf {
        let ledger = dir.file("ledger");
        fs::create_dir_all(&ledger).unwrap();
        let path = ledger.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    /// The log directory a ledger keeps: `<ledger>/.ledger/log`.
    fn log_of(dir: &TempDir) -> PathBuf {
        dir.file("ledger").join(".ledger").join("log")
    }

    fn lines_of(root: &Path) -> Vec<Vec<u8>> {
        let mut lines = Vec::new();
        for path in log_files(root).unwrap() {
            for line in fs::read(&path).unwrap().split(|byte| *byte == b'\n') {
                let line = line.trim_ascii();
                if !line.is_empty() {
                    lines.push(line.to_vec());
                }
            }
        }
        lines
    }

    #[test]
    fn records_and_lists_a_round_trip() {
        let dir = TempDir::new("history-round-trip");
        let path = ledger_file(&dir, "main.bean", "before\n");
        let root = log_of(&dir);

        let change =
            Change::new(path.clone(), Some(b"before\n"), Some(b"after\n")).with_hunks(vec![Hunk {
                at: "9f2c".to_string(),
                removed: 1,
                added: 1,
            }]);
        let operation = record(&root, Origin::Ai, "assistant edit", &[change]).unwrap();

        // The id is `YYYY-MM-DDTHH-MM-SSZ-<device4>-<part4>`.
        assert_eq!(operation.id.len(), 30, "id: {}", operation.id);
        let stem = &operation.id[..20];
        assert_eq!(&stem[4..5], "-", "id: {}", operation.id);
        assert_eq!(&stem[10..11], "T", "id: {}", operation.id);
        assert_eq!(&stem[19..20], "Z", "id: {}", operation.id);
        assert!(
            operation.at.starts_with(&stem[..10]),
            "at: {}",
            operation.at
        );
        assert!(operation.at.ends_with('Z'), "at: {}", operation.at);
        assert_eq!(operation.device.len(), 16);
        assert_eq!(&operation.id[21..25], &operation.device[..4]);

        // One line, on this device's log, naming both sides by their hashes.
        let log = root.join(format!("{}.jsonl", operation.device));
        assert!(log.is_file(), "{}", log.display());
        let lines = lines_of(&root);
        assert_eq!(lines.len(), 1);
        let json: serde_json::Value = serde_json::from_slice(&lines[0]).unwrap();
        assert_eq!(json["id"], operation.id);
        assert_eq!(json["origin"], "ai");
        assert_eq!(json["summary"], "assistant edit");
        assert_eq!(json["device"], operation.device);
        assert_eq!(json["ops"][0]["path"], path.display().to_string());
        assert_eq!(json["ops"][0]["before_hash"], hash_of(b"before\n"));
        assert_eq!(json["ops"][0]["after_hash"], hash_of(b"after\n"));
        assert_eq!(json["ops"][0]["hunks"][0]["at"], "9f2c");
        assert_eq!(json["ops"][0]["hunks"][0]["removed"], 1);

        // Both images are in the blob store, under their own hashes.
        let blobs = root.join(BLOB_DIR);
        assert_eq!(
            fs::read(blobs.join(hash_of(b"before\n"))).unwrap(),
            b"before\n"
        );
        assert_eq!(
            fs::read(blobs.join(hash_of(b"after\n"))).unwrap(),
            b"after\n"
        );

        // The device id is on disk, and stable.
        assert_eq!(
            fs::read_to_string(root.join(DEVICE_FILE)).unwrap().trim(),
            operation.device
        );
        assert_eq!(device_id(&root).unwrap(), operation.device);

        // What we last saw is in the state file, for conflict detection.
        assert_eq!(
            last_seen(&root).unwrap(),
            vec![(path.clone(), Some(hash_of(b"after\n")))]
        );

        let listed = list(&root, Filter::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], operation);
        assert_eq!(listed[0].ops[0].hunks.as_ref().unwrap()[0].added, 1);
    }

    #[test]
    fn filters_by_origin_and_path_and_limit() {
        let dir = TempDir::new("history-filters");
        let main = ledger_file(&dir, "main.bean", "one\n");
        let other = ledger_file(&dir, "other.bean", "two\n");
        let root = log_of(&dir);

        let first = record(
            &root,
            Origin::User,
            "typo",
            &[Change::new(main.clone(), Some(b"one\n"), Some(b"two\n"))],
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let second = record(
            &root,
            Origin::Ai,
            "assistant edit",
            &[Change::new(other.clone(), Some(b"two\n"), Some(b"three\n"))],
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let third = record(
            &root,
            Origin::Importer,
            "import",
            &[
                Change::new(main.clone(), Some(b"two\n"), Some(b"three\n")),
                Change::new(other.clone(), Some(b"three\n"), Some(b"four\n")),
            ],
        )
        .unwrap();

        // Newest first, always.
        let all = list(&root, Filter::default()).unwrap();
        let ids: Vec<&str> = all.iter().map(|operation| operation.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![third.id.as_str(), second.id.as_str(), first.id.as_str()]
        );

        let ai = list(
            &root,
            Filter {
                origin: Some(Origin::Ai),
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(ai.len(), 1);
        assert_eq!(ai[0].id, second.id);

        let importer = list(
            &root,
            Filter {
                origin: Some(Origin::Importer),
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(importer.len(), 1);
        assert_eq!(importer[0].id, third.id);

        // A path filter matches an operation that touched the file, whichever
        // other files it also touched.
        let touching_main = list(
            &root,
            Filter {
                path: Some(main.clone()),
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(touching_main.len(), 2);
        assert_eq!(touching_main[0].id, third.id);
        assert_eq!(touching_main[1].id, first.id);

        let nothing = list(
            &root,
            Filter {
                path: Some(dir.file("elsewhere.bean")),
                ..Filter::default()
            },
        )
        .unwrap();
        assert!(nothing.is_empty());

        // A limit takes the newest matches, after filtering.
        let limited = list(
            &root,
            Filter {
                origin: None,
                path: Some(main),
                limit: 1,
            },
        )
        .unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, third.id);
    }

    #[test]
    fn revert_returns_the_exact_previous_bytes() {
        let dir = TempDir::new("history-revert");
        // Carriage returns and non-ASCII: the journal stores bytes, not lines.
        let before = "café\r\nlines\n".as_bytes();
        let after = "changed\r\n".as_bytes();
        let path = ledger_file(&dir, "main.bean", std::str::from_utf8(after).unwrap());
        let root = log_of(&dir);

        let operation = record(
            &root,
            Origin::Ai,
            "assistant edit",
            &[Change::new(path.clone(), Some(before), Some(after))],
        )
        .unwrap();

        let restored = revert(&root, &operation.id).unwrap();
        assert_eq!(restored, vec![(path.clone(), before.to_vec())]);
        // Reverting only hands bytes back: the file still holds the new text.
        assert_eq!(fs::read(&path).unwrap(), after);
    }

    #[test]
    fn revert_refuses_after_a_later_edit() {
        let dir = TempDir::new("history-refuse");
        let path = ledger_file(&dir, "main.bean", "first\n");
        let root = log_of(&dir);

        let operation = record(
            &root,
            Origin::Ai,
            "assistant edit",
            &[Change::new(
                path.clone(),
                Some(b"first\n"),
                Some(b"second\n"),
            )],
        )
        .unwrap();
        fs::write(&path, b"second\n").unwrap();

        // A later edit — the user's, an import's, a peer's — must not be
        // clobbered.
        fs::write(&path, b"third\n").unwrap();
        let refused = revert(&root, &operation.id).unwrap_err();
        assert!(refused.contains("changed since operation"), "{refused}");
        assert!(refused.contains(&operation.id), "{refused}");
        assert!(revert_plan(&root, &operation.id).is_err());

        // A missing file is the same refusal, and an unknown id is an error.
        fs::remove_file(&path).unwrap();
        assert!(revert(&root, &operation.id).is_err());
        assert!(revert(&root, "nope").is_err());
    }

    #[test]
    fn created_and_deleted_files() {
        let dir = TempDir::new("history-create-delete");
        let created = ledger_file(&dir, "old.bean", "stale\n");
        fs::remove_file(&created).unwrap();
        let removed = ledger_file(&dir, "gone.bean", "kept\n");
        let root = log_of(&dir);

        let operation = record(
            &root,
            Origin::User,
            "new file",
            &[
                Change::new(created.clone(), None, Some(b"fresh\n")),
                Change::new(removed.clone(), Some(b"kept\n"), None),
            ],
        )
        .unwrap();
        assert_eq!(operation.ops.len(), 2);
        let created_op = operation
            .ops
            .iter()
            .find(|file| file.path == created)
            .unwrap();
        assert_eq!(created_op.before_hash, None);
        let deleted_op = operation
            .ops
            .iter()
            .find(|file| file.path == removed)
            .unwrap();
        assert_eq!(deleted_op.after_hash, None);
        // The deleted file's image is kept; there is no "before" for a creation.
        assert_eq!(
            fs::read(root.join(BLOB_DIR).join(hash_of(b"kept\n"))).unwrap(),
            b"kept\n"
        );

        // The write the journal describes: a new file, one file gone.
        fs::write(&created, b"fresh\n").unwrap();
        fs::remove_file(&removed).unwrap();

        // The plan reports the creation as "no bytes before", and the deletion
        // as the bytes to put back.
        let plan = revert_plan(&root, &operation.id).unwrap();
        assert_eq!(plan.len(), 2);
        let created_entry = plan.iter().find(|entry| entry.path == created).unwrap();
        assert_eq!(created_entry.before, None);
        let removed_entry = plan.iter().find(|entry| entry.path == removed).unwrap();
        assert_eq!(removed_entry.before.as_deref(), Some(&b"kept\n"[..]));

        // `revert` is the bytes only: the created file contributes nothing.
        let restored = revert(&root, &operation.id).unwrap();
        assert_eq!(restored, vec![(removed.clone(), b"kept\n".to_vec())]);

        // A created file that was edited afterwards still blocks the undo, and
        // so does one that reappeared where the operation deleted it.
        fs::write(&created, b"edited\n").unwrap();
        let refused = revert_plan(&root, &operation.id).unwrap_err();
        assert!(refused.contains("changed since operation"), "{refused}");
        fs::write(&created, b"fresh\n").unwrap();
        fs::write(&removed, b"back\n").unwrap();
        let refused = revert_plan(&root, &operation.id).unwrap_err();
        assert!(refused.contains("changed since operation"), "{refused}");

        // The state file remembers the deletion as "gone".
        let seen = last_seen(&root).unwrap();
        assert_eq!(
            seen.iter()
                .find(|(path, _)| path == &removed)
                .map(|(_, hash)| hash.clone()),
            Some(None)
        );
    }

    /// Where a version's image lives, from its bytes.
    fn blob(root: &Path, bytes: &[u8]) -> PathBuf {
        root.join(BLOB_DIR).join(hash_of(bytes))
    }

    /// The bytes a directory holds, for the retention assertions.
    fn total_bytes(dir: &Path) -> u64 {
        let mut total = 0;
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                total += total_bytes(&entry.path());
            } else {
                total += entry.metadata().unwrap().len();
            }
        }
        total
    }

    #[test]
    fn prune_keeps_the_newest_and_each_paths_base() {
        let dir = TempDir::new("history-prune");
        let a = ledger_file(&dir, "a.bean", "v0a\n");
        let root = log_of(&dir);

        // Four versions of `a`, then one write to another file that ends up the
        // newest operation of all.
        let mut written = Vec::new();
        let mut previous = String::from("v0a\n");
        for index in 1..=4 {
            std::thread::sleep(Duration::from_millis(2));
            let after = format!("v{index}a\n");
            written.push(
                record(
                    &root,
                    Origin::User,
                    &format!("write {index}"),
                    &[Change::new(
                        a.clone(),
                        Some(previous.as_bytes()),
                        Some(after.as_bytes()),
                    )],
                )
                .unwrap(),
            );
            previous = after;
            fs::write(&a, &previous).unwrap();
        }
        let b = ledger_file(&dir, "b.bean", "v0b\n");
        std::thread::sleep(Duration::from_millis(2));
        let newest = record(
            &root,
            Origin::User,
            "other file",
            &[Change::new(b.clone(), Some(b"v0b\n"), Some(b"v1b\n"))],
        )
        .unwrap();
        fs::write(&b, b"v1b\n").unwrap();

        // `keep_newest` covers only the newest operation (on `b`), and nothing
        // else is pinned: `a`'s newest write survives on the merge-base rule
        // alone, because an older `a` operation cannot be a base for a peer.
        let freed = prune(
            &root,
            Retention {
                max_operations: 1,
                max_bytes: 0,
                keep_newest: 1,
                keep_newest_ai: 0,
            },
        )
        .unwrap();
        assert!(freed > 0, "freed: {freed}");

        // The lines are all still there: the log is the record of what
        // happened, whatever happened to the images.
        let listed = list(&root, Filter::default()).unwrap();
        assert_eq!(listed.len(), 5);
        assert_eq!(listed[0].id, newest.id);

        // `a`'s newest write kept both of its sides — the merge base a peer's
        // version is compared against — while everything older went.
        assert!(blob(&root, b"v3a\n").exists());
        assert!(blob(&root, b"v4a\n").exists());
        assert!(!blob(&root, b"v0a\n").exists());
        assert!(!blob(&root, b"v1a\n").exists());
        assert!(!blob(&root, b"v2a\n").exists());
        assert!(blob(&root, b"v1b\n").exists());

        // The kept images still undo: the newest write, and `a`'s newest.
        assert_eq!(revert(&root, &newest.id).unwrap()[0].1, b"v0b\n".to_vec());
        assert_eq!(
            revert(&root, &written[3].id).unwrap()[0].1,
            b"v3a\n".to_vec()
        );

        // Undo the newest `a` write, and the one before it cannot follow: its
        // after state matches, but its image was pruned — said out loud rather
        // than guessed.
        fs::write(&a, b"v3a\n").unwrap();
        let refused = revert(&root, &written[2].id).unwrap_err();
        assert!(refused.contains("pruned"), "{refused}");

        // And once the file has moved on again, that is the refusal instead.
        fs::write(&a, b"v9a\n").unwrap();
        let refused = revert(&root, &written[2].id).unwrap_err();
        assert!(refused.contains("changed since operation"), "{refused}");
    }

    #[test]
    fn prune_bounds_bytes() {
        let dir = TempDir::new("history-prune-bytes");
        let path = ledger_file(&dir, "main.bean", "start\n");
        let root = log_of(&dir);

        let mut written = Vec::new();
        let mut previous = String::from("start\n");
        let mut versions = vec![previous.clone()];
        for index in 0..4 {
            std::thread::sleep(Duration::from_millis(2));
            let after = format!("version {index} {}\n", "x".repeat(200));
            written.push(
                record(
                    &root,
                    Origin::User,
                    "write",
                    &[Change::new(
                        path.clone(),
                        Some(previous.as_bytes()),
                        Some(after.as_bytes()),
                    )],
                )
                .unwrap(),
            );
            previous = after;
            versions.push(previous.clone());
            fs::write(&path, &previous).unwrap();
        }

        // A budget for a single image: only the newest operation's two sides
        // fit, and it is never pruned anyway.
        let freed = prune(
            &root,
            Retention {
                max_operations: 0,
                max_bytes: 250,
                keep_newest: 1,
                keep_newest_ai: 0,
            },
        )
        .unwrap();
        assert!(freed > 0, "freed: {freed}");
        assert!(blob(&root, versions[4].as_bytes()).exists());
        assert!(blob(&root, versions[3].as_bytes()).exists());
        assert!(!blob(&root, versions[0].as_bytes()).exists());
        assert!(!blob(&root, versions[1].as_bytes()).exists());
        // The store now holds the newest operation's images and no more.
        let kept = total_bytes(&root.join(BLOB_DIR));
        assert!(kept <= 2 * versions[4].len() as u64, "kept: {kept}");
        assert_eq!(
            revert(&root, &written[3].id).unwrap()[0].1,
            versions[3].as_bytes().to_vec()
        );

        // Nothing left to prune is not an error.
        assert_eq!(
            prune(
                &root,
                Retention {
                    max_operations: 0,
                    max_bytes: 1,
                    keep_newest: 10,
                    keep_newest_ai: 10,
                },
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn prune_keeps_the_images_a_torn_line_names() {
        let dir = TempDir::new("history-torn-salvage");
        let path = ledger_file(&dir, "main.bean", "v0\n");
        let root = log_of(&dir);

        let first = record(
            &root,
            Origin::User,
            "first",
            &[Change::new(path.clone(), Some(b"v0\n"), Some(b"v1\n"))],
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let second = record(
            &root,
            Origin::User,
            "second",
            &[Change::new(path.clone(), Some(b"v1\n"), Some(b"v2\n"))],
        )
        .unwrap();
        assert_ne!(first.id, second.id);

        // A peer's newest line, half-copied: it cannot be parsed, but the hash
        // of the image it names is still in the bytes.
        let torn = format!(
            "{{\"id\": \"{}-peer\", \"at\": \"{}\", \"device\": \"peer\", \"origin\": \"ai\", \"summary\": \"torn\", \"ops\": [{{\"path\": \"{}\", \"before_hash\": \"{}\", ",
            second.id,
            second.at,
            path.display(),
            hash_of(b"v0\n")
        );
        fs::write(root.join("peer.jsonl"), torn.as_bytes()).unwrap();

        // Nothing is unprotected: `v0` is referenced only by the torn bytes,
        // and that is enough to keep it.
        let retention = Retention {
            max_operations: 1,
            max_bytes: 0,
            keep_newest: 1,
            keep_newest_ai: 0,
        };
        assert_eq!(prune(&root, retention).unwrap(), 0);
        assert!(blob(&root, b"v0\n").exists());

        // With the torn line gone, the same prune lets `v0` go.
        fs::write(root.join("peer.jsonl"), b"").unwrap();
        let freed = prune(&root, retention).unwrap();
        assert_eq!(freed, b"v0\n".len() as u64);
        assert!(!blob(&root, b"v0\n").exists());
        // The newest operation's images, and `v1` as its before, are still
        // there for a revert.
        fs::write(&path, b"v2\n").unwrap();
        assert_eq!(revert(&root, &second.id).unwrap()[0].1, b"v1\n".to_vec());
    }

    #[test]
    fn prune_keeps_the_ai_window() {
        let dir = TempDir::new("history-prune-ai");
        let path = ledger_file(&dir, "main.bean", "v0\n");
        let root = log_of(&dir);

        // A chain where the AI write sits in the middle, so the plain window
        // alone would drop it and a later operation would replace it.
        let mut previous = String::from("v0\n");
        let mut versions = vec![previous.clone()];
        let mut ids = Vec::new();
        for (index, origin) in [Origin::User, Origin::Ai, Origin::User, Origin::User]
            .into_iter()
            .enumerate()
        {
            std::thread::sleep(Duration::from_millis(2));
            let after = format!("v{}\n", index + 1);
            ids.push(
                record(
                    &root,
                    origin,
                    &format!("write {index}"),
                    &[Change::new(
                        path.clone(),
                        Some(previous.as_bytes()),
                        Some(after.as_bytes()),
                    )],
                )
                .unwrap()
                .id,
            );
            previous = after;
            versions.push(previous.clone());
            fs::write(&path, &previous).unwrap();
        }

        let keep_the_ai = Retention {
            max_operations: 1,
            max_bytes: 0,
            keep_newest: 1,
            keep_newest_ai: 1,
        };
        let freed = prune(&root, keep_the_ai).unwrap();
        assert!(freed > 0, "freed: {freed}");
        // `v1` is only ever the AI operation's own before-image, so its
        // presence is that window holding.
        assert!(blob(&root, b"v1\n").exists());
        assert!(!blob(&root, b"v0\n").exists());

        // Take the AI window away and the same image goes.
        let freed = prune(
            &root,
            Retention {
                keep_newest_ai: 0,
                ..keep_the_ai
            },
        )
        .unwrap();
        assert!(freed > 0, "freed: {freed}");
        assert!(!blob(&root, b"v1\n").exists());
        assert!(blob(&root, b"v3\n").exists());
        assert!(blob(&root, b"v4\n").exists());
        // The newest operation is still whole.
        assert_eq!(
            revert(&root, ids.last().unwrap()).unwrap()[0].1,
            b"v3\n".to_vec()
        );
    }

    #[test]
    fn corrupt_line_is_skipped_while_others_load() {
        let dir = TempDir::new("history-corrupt");
        let path = ledger_file(&dir, "main.bean", "good\n");
        let root = log_of(&dir);

        let good = record(
            &root,
            Origin::User,
            "write",
            &[Change::new(
                path.clone(),
                Some(b"good\n"),
                Some(b"better\n"),
            )],
        )
        .unwrap();

        // A torn line, as a sync that stopped in the middle would leave: an
        // unterminated one in our own log, and a whole unsynced file.
        let log = root.join(format!("{}.jsonl", good.device));
        let mut text = fs::read_to_string(&log).unwrap();
        text.push_str("{\"id\": \"torn\"");
        fs::write(&log, text).unwrap();
        fs::write(root.join("peer.jsonl"), b"not json\n").unwrap();

        let (listed, issues) = list_with_issues(&root, Filter::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, good.id);
        assert_eq!(issues.len(), 2, "issues: {issues:?}");
        assert!(
            issues.iter().any(|issue| issue.contains("peer.jsonl:1")),
            "{issues:?}"
        );

        // `list` reports the skips on stderr and still answers.
        assert_eq!(list(&root, Filter::default()).unwrap().len(), 1);

        // The corrupt line does not block appending after it: the next write
        // lands on its own line and reads back.
        std::thread::sleep(Duration::from_millis(2));
        let next = record(
            &root,
            Origin::Ai,
            "after the tear",
            &[Change::new(
                path.clone(),
                Some(b"better\n"),
                Some(b"best\n"),
            )],
        )
        .unwrap();
        let (listed, issues) = list_with_issues(&root, Filter::default()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, next.id);
        assert_eq!(issues.len(), 2, "issues: {issues:?}");
    }

    #[test]
    fn concurrent_records_each_survive_on_their_own_line() {
        let dir = TempDir::new("history-concurrent");
        let root = log_of(&dir);
        fs::create_dir_all(dir.file("ledger")).unwrap();

        // Two threads, one log file, one lock: every operation must survive as
        // its own complete line.
        let threads: Vec<_> = (0..4)
            .map(|index| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let mut ids = Vec::new();
                    for turn in 0..15 {
                        let path = dir_file(&root, &format!("{index}-{turn}.bean"));
                        let before = format!("before {index} {turn}\n");
                        let after = format!("after {index} {turn}\n");
                        ids.push(
                            record(
                                &root,
                                Origin::Ai,
                                "concurrent",
                                &[Change::new(
                                    path,
                                    Some(before.as_bytes()),
                                    Some(after.as_bytes()),
                                )],
                            )
                            .unwrap()
                            .id,
                        );
                    }
                    ids
                })
            })
            .collect();
        let mut ids: Vec<String> = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect();

        // One log file — this is all one device — with one line per operation,
        // every line whole, and no id reused.
        let files = log_files(&root).unwrap();
        assert_eq!(files.len(), 1, "files: {files:?}");
        let lines = lines_of(&root);
        assert_eq!(lines.len(), 60);
        for line in &lines {
            let operation: Operation = serde_json::from_slice(line).unwrap();
            assert_eq!(operation.origin, Origin::Ai);
            assert!(!operation.ops.is_empty());
        }
        ids.sort();
        let unique: HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 60);
        assert_eq!(list(&root, Filter::default()).unwrap().len(), 60);
        // The lock is released.
        assert!(!lock_path(&root, &device_id(&root).unwrap()).exists());
    }

    /// A path under the ledger for the concurrency tests, which have no
    /// `TempDir` to ask.
    fn dir_file(root: &Path, name: &str) -> PathBuf {
        root.parent().unwrap().parent().unwrap().join(name)
    }

    #[test]
    fn a_held_lock_makes_the_next_record_wait() {
        let dir = TempDir::new("history-lock");
        let path = ledger_file(&dir, "main.bean", "one\n");
        let root = log_of(&dir);
        fs::create_dir_all(&root).unwrap();
        let lock = lock_path(&root, &device_id(&root).unwrap());
        fs::write(&lock, b"").unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = {
            let root = root.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                sender
                    .send(record(
                        &root,
                        Origin::User,
                        "waited",
                        &[Change::new(path, Some(b"one\n"), Some(b"two\n"))],
                    ))
                    .unwrap();
            })
        };

        // A fresh lock is live, so the append waits rather than breaking it.
        assert!(receiver.recv_timeout(Duration::from_millis(150)).is_err());
        fs::remove_file(&lock).unwrap();
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .is_ok()
        );
        handle.join().unwrap();
        assert_eq!(list(&root, Filter::default()).unwrap().len(), 1);
    }

    #[test]
    fn a_stale_lock_is_broken() {
        let dir = TempDir::new("history-stale");
        let path = ledger_file(&dir, "main.bean", "one\n");
        let root = log_of(&dir);
        fs::create_dir_all(&root).unwrap();

        // A lock left by a process that died holding it: older than anything a
        // live append could be, so it is broken instead of waited on.
        let lock = lock_path(&root, &device_id(&root).unwrap());
        let file = fs::File::create(&lock).unwrap();
        file.set_modified(SystemTime::now() - LOCK_STALE - Duration::from_secs(1))
            .unwrap();
        drop(file);
        assert!(is_stale(&lock));

        let operation = record(
            &root,
            Origin::User,
            "after the crash",
            &[Change::new(path, Some(b"one\n"), Some(b"two\n"))],
        )
        .unwrap();
        assert!(!lock.exists());
        assert_eq!(list(&root, Filter::default()).unwrap()[0].id, operation.id);
    }

    /// Appends for [`concurrent_processes_share_one_log`]: a child re-runs this
    /// test binary and appends, so the lock is exercised across processes, not
    /// just threads. Without the environment it does nothing.
    #[test]
    fn append_helper_process() {
        let Ok(root) = std::env::var("LEDGER_HISTORY_HELPER_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let tag = std::env::var("LEDGER_HISTORY_HELPER_TAG").unwrap();
        for turn in 0..10 {
            let path = root
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!("{tag}-{turn}.bean"));
            let before = format!("{tag} before {turn}\n");
            let after = format!("{tag} after {turn}\n");
            record(
                &root,
                Origin::Ai,
                "child",
                &[Change::new(
                    path,
                    Some(before.as_bytes()),
                    Some(after.as_bytes()),
                )],
            )
            .unwrap();
        }
    }

    #[test]
    fn concurrent_processes_share_one_log() {
        let dir = TempDir::new("history-processes");
        fs::create_dir_all(dir.file("ledger")).unwrap();
        let root = log_of(&dir);
        let exe = std::env::current_exe().unwrap();

        let children: Vec<_> = (0..4)
            .map(|index| {
                std::process::Command::new(&exe)
                    .args([
                        "history::tests::append_helper_process",
                        "--exact",
                        "--nocapture",
                    ])
                    .env("LEDGER_HISTORY_HELPER_ROOT", &root)
                    .env("LEDGER_HISTORY_HELPER_TAG", format!("child{index}"))
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut child in children {
            let status = child.wait().unwrap();
            assert!(status.success(), "child failed: {status}");
        }

        // Four processes, one log file, forty whole lines.
        assert_eq!(log_files(&root).unwrap().len(), 1);
        let lines = lines_of(&root);
        assert_eq!(lines.len(), 40);
        for line in &lines {
            serde_json::from_slice::<Operation>(line).unwrap();
        }
        assert_eq!(list(&root, Filter::default()).unwrap().len(), 40);
    }

    #[test]
    fn no_op_and_empty_writes_are_refused() {
        let dir = TempDir::new("history-noop");
        let path = ledger_file(&dir, "main.bean", "same\n");
        let root = log_of(&dir);

        let error = record(&root, Origin::User, "nothing", &[]).unwrap_err();
        assert!(error.contains("nothing to record"), "{error}");
        let error = record(
            &root,
            Origin::User,
            "nothing",
            &[Change::new(path.clone(), Some(b"same\n"), Some(b"same\n"))],
        )
        .unwrap_err();
        assert!(error.contains("nothing to record"), "{error}");
        assert!(list(&root, Filter::default()).unwrap().is_empty());

        // Two entries for one file are a caller bug, not an operation.
        let error = record(
            &root,
            Origin::User,
            "twice",
            &[
                Change::new(path.clone(), Some(b"same\n"), Some(b"one\n")),
                Change::new(path, Some(b"same\n"), Some(b"two\n")),
            ],
        )
        .unwrap_err();
        assert!(error.contains("appears twice"), "{error}");
    }

    #[test]
    fn two_devices_keep_two_logs() {
        let dir = TempDir::new("history-devices");
        let path = ledger_file(&dir, "main.bean", "ours\n");
        let root = log_of(&dir);

        let ours = record(
            &root,
            Origin::User,
            "here",
            &[Change::new(
                path.clone(),
                Some(b"ours\n"),
                Some(b"ours 2\n"),
            )],
        )
        .unwrap();

        // A peer's line, in the shape the other device would have written it:
        // same second, same suffix, different device.
        let peer = Operation {
            id: format!(
                "{}-ffff-{:04x}",
                &ours.id[..20],
                u16::from_str_radix(&ours.id[26..], 16).unwrap()
            ),
            at: ours.at.clone(),
            device: "ffffffffffffffff".to_string(),
            origin: Origin::Ai,
            summary: "peer".to_string(),
            ops: vec![FileOp {
                path: path.clone(),
                before_hash: Some(hash_of(b"ours 2\n")),
                after_hash: Some(hash_of(b"ours 3\n")),
                hunks: None,
            }],
        };
        fs::write(
            root.join("ffffffffffffffff.jsonl"),
            [serde_json::to_vec(&peer).unwrap(), b"\n".to_vec()].concat(),
        )
        .unwrap();
        // The peer's images would have synced with it, and so did its write.
        fs::write(root.join(BLOB_DIR).join(hash_of(b"ours 2\n")), b"ours 2\n").unwrap();
        fs::write(&path, b"ours 3\n").unwrap();

        let listed = list(&root, Filter::default()).unwrap();
        assert_eq!(listed.len(), 2);
        // Both list, from their own files, and the ids stay apart.
        assert_ne!(listed[0].id, listed[1].id);
        assert!(
            listed
                .iter()
                .any(|operation| operation.device == peer.device)
        );
        assert_eq!(
            revert(&root, &peer.id).unwrap(),
            vec![(path, b"ours 2\n".to_vec())]
        );
    }

    /// A limit is a window over the merged log, not one per device: each
    /// device's log lends its own newest candidates, the merge puts them in one
    /// order, and the cut takes the newest matches — after the filter, never
    /// before it.
    #[test]
    fn a_limit_window_is_the_newest_across_devices() {
        let dir = TempDir::new("history-limit-devices");
        let path = ledger_file(&dir, "main.bean", "v0\n");
        let root = log_of(&dir);

        let first = record(
            &root,
            Origin::User,
            "here 1",
            &[Change::new(path.clone(), Some(b"v0\n"), Some(b"v1\n"))],
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let second = record(
            &root,
            Origin::User,
            "here 2",
            &[Change::new(path.clone(), Some(b"v1\n"), Some(b"v2\n"))],
        )
        .unwrap();

        // A peer's line, newer than both writes here and touching the same
        // path.
        let peer = Operation {
            id: "2100-01-01T00-00-00Z-ffff-0000".to_string(),
            at: "2100-01-01T00:00:00.000000000Z".to_string(),
            device: "ffffffffffffffff".to_string(),
            origin: Origin::Ai,
            summary: "peer".to_string(),
            ops: vec![FileOp {
                path: path.clone(),
                before_hash: Some(hash_of(b"v2\n")),
                after_hash: Some(hash_of(b"v3\n")),
                hunks: None,
            }],
        };
        fs::write(
            root.join("ffffffffffffffff.jsonl"),
            [serde_json::to_vec(&peer).unwrap(), b"\n".to_vec()].concat(),
        )
        .unwrap();

        // The newest two of the merged log: the peer's line, then the newest
        // write here — whichever device's file they live in.
        let newest = list(
            &root,
            Filter {
                limit: 2,
                ..Filter::default()
            },
        )
        .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(
            newest
                .iter()
                .map(|operation| operation.id.as_str())
                .collect::<Vec<_>>(),
            [peer.id.as_str(), second.id.as_str()]
        );
        // The filter runs before the cut: a user-only window holds the newest
        // user write, though the peer's newer line would win an unfiltered one.
        let user = list(
            &root,
            Filter {
                origin: Some(Origin::User),
                limit: 1,
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].id, second.id);
    }

    #[test]
    fn timestamp_and_id_parts() {
        let (at, stem) = timestamp_parts(UNIX_EPOCH + std::time::Duration::new(1_771_000_000, 1));
        assert_eq!(stem, "2026-02-13T16-26-40Z");
        assert_eq!(at, "2026-02-13T16:26:40.000000001Z");
        assert_eq!(date_of(0), (1970, 1, 1));
        assert_eq!(date_of(-1), (1969, 12, 31));
        assert_eq!(date_of(19_782), (2024, 2, 29));

        // The id part never repeats in one process.
        let first = next_id_part();
        let second = next_id_part();
        assert_ne!(first, second);
        // Random ids are 16 hex characters, and two are different.
        assert_eq!(random_id().len(), 16);
        assert_ne!(random_id(), random_id());
    }

    /// How many images the store holds, as the retention counts them.
    fn images(root: &Path) -> usize {
        fs::read_dir(root.join(BLOB_DIR))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The margin is a quarter over the retention, counted in images: at 1.25 ×
    /// `max_operations` operations' worth — 500 images for the default — the
    /// check is still quiet, one image past it is not, and a retention with no
    /// operation ceiling never asks for a prune.
    #[test]
    fn the_prune_margin_is_a_quarter_over_the_retention() {
        let dir = TempDir::new("history-margin");
        let root = log_of(&dir);
        let blobs = root.join(BLOB_DIR);
        fs::create_dir_all(&blobs).unwrap();
        for index in 0..500 {
            fs::write(blobs.join(format!("{index:064x}")), b"x").unwrap();
        }
        let retention = Retention::default();
        assert!(!over_retention(&root, retention));
        fs::write(blobs.join(format!("{:064x}", 500)), b"x").unwrap();
        assert!(over_retention(&root, retention));
        assert!(!over_retention(
            &root,
            Retention {
                max_operations: 0,
                ..retention
            }
        ));
    }

    /// A write that leaves the log past the default retention's margin pays for
    /// the prune itself, since no caller ever asks for one: the crossing write
    /// trims the store back to the versions the retention's operations name.
    #[test]
    fn a_write_past_the_retention_margin_prunes() {
        let dir = TempDir::new("history-auto-prune");
        let first = ledger_file(&dir, "a.bean", "a0\n");
        let second = ledger_file(&dir, "b.bean", "b0\n");
        let root = log_of(&dir);

        // Two new images a write, so the 250th record is the one that leaves
        // 502 images — past the 500 the margin allows for 200 operations.
        let mut newest = None;
        for index in 1..=250u32 {
            let before_a = format!("a{}\n", index - 1);
            let after_a = format!("a{index}\n");
            let before_b = format!("b{}\n", index - 1);
            let after_b = format!("b{index}\n");
            newest = Some(
                record(
                    &root,
                    Origin::User,
                    "write",
                    &[
                        Change::new(
                            first.clone(),
                            Some(before_a.as_bytes()),
                            Some(after_a.as_bytes()),
                        ),
                        Change::new(
                            second.clone(),
                            Some(before_b.as_bytes()),
                            Some(after_b.as_bytes()),
                        ),
                    ],
                )
                .unwrap(),
            );
        }
        let newest = newest.unwrap();

        // The store kept what the newest 200 operations name — `a50`/`b50` on,
        // their versions shared — and gave back everything older.
        assert!(blob(&root, b"a50\n").exists());
        assert!(blob(&root, b"b50\n").exists());
        assert!(blob(&root, b"a250\n").exists());
        assert!(!blob(&root, b"a49\n").exists());
        assert!(!blob(&root, b"b0\n").exists());
        assert_eq!(images(&root), 402);

        // The lines are all still there, and the newest write still undoes.
        fs::write(&first, b"a250\n").unwrap();
        fs::write(&second, b"b250\n").unwrap();
        assert_eq!(list(&root, Filter::default()).unwrap().len(), 250);
        assert_eq!(revert(&root, &newest.id).unwrap()[0].1, b"a249\n".to_vec());

        // Prune is only paid for at the crossing: the next write sits well
        // inside the ceiling again.
        assert!(!over_retention(&root, Retention::default()));
        let next = record(
            &root,
            Origin::User,
            "write",
            &[Change::new(first.clone(), Some(b"a250\n"), Some(b"a251\n"))],
        )
        .unwrap();
        assert_eq!(list(&root, Filter::default()).unwrap().len(), 251);
        assert_eq!(list(&root, Filter::default()).unwrap()[0].id, next.id);
    }
}
