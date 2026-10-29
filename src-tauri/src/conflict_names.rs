// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync conflicts: which files in a ledger folder are a cloud client's second
//! copy of another file, and which original each one belongs to.
//!
//! A SAF-mirrored ledger is synced by Drive, Dropbox, OneDrive, Nextcloud,
//! Syncthing, Seafile or iCloud, and those clients resolve a conflict by keeping
//! both versions: one file keeps its name, the other arrives under a name the
//! client invents. That name is all the evidence there is — the two files hold
//! two ordinary snapshots of the same journal — so detection is name parsing
//! plus the requirement that the original still sits in the same directory.
//!
//! This module is detection and nothing else: no merging, no rewriting, and no
//! reading of the operation log. A [`Candidate`] is a suggestion for the UI to
//! put in front of the user: this copy, that original, this client, this
//! timestamp.
//!
//! Two kinds of name are told apart ([`Evidence`]). The clients that stamp their
//! own token *and* a timestamp (Syncthing's `.sync-conflict-…`, Seafile's
//! `(SFConflict …)`, Nextcloud's `(conflicted copy …)`, Drive/Dropbox's
//! `…'s conflicted copy <date>`) prove themselves with no directory at hand, so
//! such a copy whose original is gone is reported as an [`Orphan`] rather than
//! dropped. The shapes that ordinary names also have (`(1)`, ` 2`, `-Device`)
//! are only reported when the original is really there — `notes (1).bean` on its
//! own is just a file.

use std::fs;
use std::path::{Path, PathBuf};

/// A cloud client whose conflict-copy naming this app knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncClient {
    /// Google Drive: `name (1).bean` and `name's conflicted copy 2026-01-30.bean`,
    /// plus the owner-qualified `name (Alper's conflicted copy 2026-01-30).bean`
    /// shape, which Dropbox writes too — see [`SyncClient::Dropbox`].
    Drive,
    /// Dropbox. Its documented conflict name is the owner-qualified
    /// `name (Alper's conflicted copy 2026-01-30).bean`, which Drive's clients
    /// also leave behind, so a name alone cannot say which of the two wrote it:
    /// [`classify_name`] reports that shape as [`SyncClient::Drive`], and this
    /// variant is for a caller that knows the client from somewhere else.
    Dropbox,
    /// OneDrive: `name-Alper.bean`, `name-Alper-PC.bean` — account and machine.
    OneDrive,
    /// Nextcloud (and ownCloud): `name (conflicted copy 2026-01-30 120000).bean`.
    Nextcloud,
    /// Syncthing: `name.sync-conflict-20260130-120000-ABCDEFG.bean`.
    Syncthing,
    /// Seafile: `name (SFConflict alper 2026-01-30-12-00-00).bean`.
    Seafile,
    /// iCloud Drive: `name 2.bean` — the lowest number a conflicted copy gets.
    // `snake_case` would spell this `i_cloud`; the client is one word.
    #[serde(rename = "icloud")]
    ICloud,
}

impl SyncClient {
    /// For the messages a merge prompt shows, not for wire values: the serde
    /// names above are the frontend's, these are the user's.
    fn human(self) -> &'static str {
        match self {
            Self::Drive => "Google Drive",
            Self::Dropbox => "Dropbox",
            Self::OneDrive => "OneDrive",
            Self::Nextcloud => "Nextcloud",
            Self::Syncthing => "Syncthing",
            Self::Seafile => "Seafile",
            Self::ICloud => "iCloud",
        }
    }
}

impl std::fmt::Display for SyncClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.human())
    }
}

/// A conflicted copy paired with the file it copies.
///
/// Both paths are absolute when the scan was handed an absolute root — they are
/// the root as given, joined with what the walk found. `original` is always a
/// file that is actually on disk: pairing never invents a name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct Candidate {
    /// The file the client kept under its own name.
    pub original: PathBuf,
    /// The second copy, under the name the client made up.
    pub copy: PathBuf,
    /// Which client's naming produced `copy`.
    pub client: SyncClient,
    /// The timestamp the name carries, exactly as written there, when it has
    /// one. The shapes (`(1)`, ` 2`, `-Alper`) carry none; clients differ in
    /// format (`2026-01-30`, `2026-01-30 120000`, `20260130-120000`, …), so this
    /// is the client's own spelling rather than a parsed date.
    pub stamp: Option<String>,
}

/// A copy whose name proves it is one, but whose original is not in the folder.
///
/// Reported apart from [`Candidate`] on purpose: an original can be deleted by
/// the user between the two devices, and a UI that offered to merge "into" a
/// file that does not exist would be inventing one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct Orphan {
    /// The copy, as the walk found it.
    pub copy: PathBuf,
    /// Which client's naming identifies it.
    pub client: SyncClient,
    /// The timestamp the name carries, when it has one.
    pub stamp: Option<String>,
}

/// One scan's answer: the pairs, and the copies that had no original to pair
/// with.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct Report {
    /// Sorted by `copy`.
    pub candidates: Vec<Candidate>,
    /// Sorted by `copy`.
    pub orphans: Vec<Orphan>,
}

/// What a file name alone proves about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// A client's own conflict token *and* a timestamp: the name is evidence on
    /// its own, with or without the original beside it.
    Token,
    /// A shape ordinary names have too (`(1)`, ` 2`, `-Device`). It is a
    /// conflicted copy only when the file it names is in the same directory.
    Shape,
}

/// What [`classify_name`] makes of one file name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct NameMatch {
    /// The client whose naming matched.
    pub client: SyncClient,
    /// The name of the file this copy belongs to, in the same directory, with
    /// the extension preserved. It may or may not exist; only a directory walk
    /// can say.
    pub original: String,
    /// The timestamp the name carries, exactly as written, when it has one.
    pub stamp: Option<String>,
    /// Whether the name proves the conflict by itself.
    pub evidence: Evidence,
}

/// The conflicted copies under `root`, each paired with an existing original.
///
/// The rest of the answer — copies whose original is gone — is
/// [`scan_report`]; this is the shorthand for the common case.
pub fn scan(root: &Path) -> Result<Vec<Candidate>, String> {
    Ok(scan_report(root)?.candidates)
}

/// Walk `root` and report every conflicted copy under it.
///
/// `.git` and `.ledger` are skipped at every depth: the first is not ledger
/// data, the second is this app's own operation log, and neither is anything a
/// sync conflict could be merged into. Ignore rules are git's own, as the
/// repository the ledger lives in states them, so a `.gitignore`d folder is not
/// walked.
///
/// A directory that cannot be listed is an error for `root` — that is the folder
/// the user picked — but only a gap for a subfolder: the conflicts elsewhere in
/// the tree are still worth reporting.
pub fn scan_report(root: &Path) -> Result<Report, String> {
    let mut report = Report::default();
    walk(root, &Ignore::open(root), &mut report)?;
    report.candidates.sort_by(|a, b| a.copy.cmp(&b.copy));
    report.orphans.sort_by(|a, b| a.copy.cmp(&b.copy));
    Ok(report)
}

/// The single-name half of the scan: which client's conflict naming `file_name`
/// carries, which name it belongs to, and that name's timestamp.
///
/// No directory is consulted, so a shape that needs an original beside it is
/// still recognised here; [`classify_name`] is the form that says so.
pub fn classify(file_name: &str) -> Option<(SyncClient, String, Option<String>)> {
    classify_name(file_name).map(|name| (name.client, name.original, name.stamp))
}

/// [`classify`] with the evidence attached, for callers that must know whether
/// the original has to exist before the name means anything.
pub fn classify_name(file_name: &str) -> Option<NameMatch> {
    let path = Path::new(file_name);
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension().and_then(|ext| ext.to_str());

    // Client tokens first, in the order of how much of the name they claim:
    // Syncthing's infix is unambiguous, then the three parenthesised tokens,
    // then the two Drive forms. A first match wins, so one copy can never be
    // reported for two clients — the owner-qualified shape is the case that
    // matters, since Drive and Dropbox both write it.
    let found = syncthing(stem)
        .or_else(|| nextcloud(stem))
        .or_else(|| seafile(stem))
        .or_else(|| owner_conflicted_inside(stem))
        .or_else(|| owner_conflicted(stem));
    if let Some((client, base, stamp)) = found {
        return Some(name_match(client, base, stamp, Evidence::Token, ext));
    }

    // A name that mentions a conflict token but did not parse as one is a
    // *broken* copy name (`a.sync-conflict-notatimestamp.bean`), not a device
    // suffix that happens to contain dashes. Refusing the shapes here is what
    // keeps that from being reported as an OneDrive copy of `a.sync.bean`.
    if mentions_conflict(stem) {
        return None;
    }

    let found = drive_numbered(stem)
        .or_else(|| icloud_numbered(stem))
        .or_else(|| onedrive_suffix(stem));
    found.map(|(client, base)| name_match(client, base, None, Evidence::Shape, ext))
}

/// A name parsed down to the original it belongs to.
fn name_match(
    client: SyncClient,
    base: &str,
    stamp: Option<String>,
    evidence: Evidence,
    ext: Option<&str>,
) -> NameMatch {
    NameMatch {
        client,
        original: with_extension(base, ext),
        stamp,
        evidence,
    }
}

// One matcher per client. Each answers the original's stem — the part of the
// copy's name the client left alone — plus the timestamp the name carries; the
// caller puts the extension back. An empty stem is no match: `(1).bean` is not a
// copy of `.bean`.

/// `name.sync-conflict-20260130-120000-ABCDEFG.bean`.
fn syncthing(stem: &str) -> Option<(SyncClient, &str, Option<String>)> {
    const TOKEN: &str = ".sync-conflict-";
    let lower = stem.to_ascii_lowercase();
    let at = lower.rfind(TOKEN)?;
    let base = &stem[..at];
    let rest = &stem[at + TOKEN.len()..];
    let mut parts = rest.split('-');
    let (date, time, device, extra) = (parts.next()?, parts.next()?, parts.next()?, parts.next());
    if extra.is_some() || !digits(date, 8) || !digits(time, 6) {
        return None;
    }
    // Syncthing's short device id is seven letters or digits.
    if !(device.len() == 7 && device.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return None;
    }
    Some((SyncClient::Syncthing, base, Some(format!("{date}-{time}"))))
}

/// `name (conflicted copy 2026-01-30 120000).bean`.
fn nextcloud(stem: &str) -> Option<(SyncClient, &str, Option<String>)> {
    let (base, inner) = trailing_paren(stem)?;
    let stamp = strip_token(inner, "conflicted copy ")?;
    let (date, time) = stamp.split_once(' ')?;
    if !dashed_date(date) || !digits(time, 6) {
        return None;
    }
    Some((SyncClient::Nextcloud, base, Some(format!("{date} {time}"))))
}

/// `name (SFConflict alper 2026-01-30-12-00-00).bean`.
fn seafile(stem: &str) -> Option<(SyncClient, &str, Option<String>)> {
    let (base, inner) = trailing_paren(stem)?;
    let rest = strip_token(inner, "SFConflict ")?;
    let (owner, stamp) = rest.split_once(' ')?;
    if owner.is_empty() || !dashed_datetime(stamp) {
        return None;
    }
    Some((SyncClient::Seafile, base, Some(stamp.to_string())))
}

/// `name (Alper's conflicted copy 2026-01-30).bean`.
///
/// Attributed to [`SyncClient::Drive`]. Dropbox writes this exact shape, and no
/// client tag in the name tells them apart, so one of the two has to win for the
/// "one copy, one client" rule; Drive is the one whose wordier conflict names
/// this repository sees (its `(1)` and `'s conflicted copy` forms), so its
/// attribution is consistent with the rest of the family. What a merge prompt
/// tells the user is where the copy came from — the merge itself is the same
/// either way.
fn owner_conflicted_inside(stem: &str) -> Option<(SyncClient, &str, Option<String>)> {
    let (base, inner) = trailing_paren(stem)?;
    let (_, stamp) = split_owner(inner)?;
    if !dashed_date(stamp) {
        return None;
    }
    Some((SyncClient::Drive, base, Some(stamp.to_string())))
}

/// `name's conflicted copy 2026-01-30.bean` — Drive's older naming.
fn owner_conflicted(stem: &str) -> Option<(SyncClient, &str, Option<String>)> {
    let (base, stamp) = split_owner(stem)?;
    if !dashed_date(stamp) {
        return None;
    }
    Some((SyncClient::Drive, base, Some(stamp.to_string())))
}

/// `name (1).bean`, `name (12).bean` — Drive's numeric copies, from 1 up.
fn drive_numbered(stem: &str) -> Option<(SyncClient, &str)> {
    let (base, inner) = trailing_paren(stem)?;
    if !(inner.len() <= 3 && inner.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    // `(0)` is not a copy Drive makes; catching it here keeps a hand-named
    // `notes (0).bean` out of the candidates.
    if inner.parse::<u32>().ok().filter(|n| *n >= 1).is_none() {
        return None;
    }
    Some((SyncClient::Drive, base))
}

/// `name 2.bean` — iCloud's conflicted copy, numbered from 2.
fn icloud_numbered(stem: &str) -> Option<(SyncClient, &str)> {
    let (base, number) = stem.rsplit_once(' ')?;
    if !(number.len() <= 2 && number.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    // A copy is the *second* file; `name 1.bean` would be the original itself.
    if number.parse::<u32>().ok().filter(|n| *n >= 2).is_none() {
        return None;
    }
    Some((SyncClient::ICloud, base))
}

/// `name-Alper.bean`, `name-Alper-PC.bean` — OneDrive's account or machine.
///
/// The split is the *first* dash whose tail can be a device or account name, so
/// `ledger-Alper-PC.bean` pairs with `ledger.bean` rather than `ledger-Alper.bean`.
fn onedrive_suffix(stem: &str) -> Option<(SyncClient, &str)> {
    for (at, _) in stem.match_indices('-') {
        let base = &stem[..at];
        if !base.is_empty() && device_suffix(&stem[at + 1..]) {
            return Some((SyncClient::OneDrive, base));
        }
    }
    None
}

/// Whether `tail` can be the account or machine name OneDrive appends.
///
/// Dashes are common in real names (`budget-2026-01.bean`), so the tail has to
/// look like a name: it starts with a letter and holds only letters, digits,
/// dashes and underscores, at least two characters of it. That is not proof on
/// its own — [`Evidence::Shape`] covers the rest.
fn device_suffix(tail: &str) -> bool {
    tail.len() >= 2
        && tail.starts_with(|c: char| c.is_ascii_alphabetic())
        && tail
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// Name surgery.

/// A stem's trailing `(…)`: the part before it, and the text inside.
///
/// The client writes a space before the bracket (`notes (1)`), so a name without
/// one is not a copy name at all.
fn trailing_paren(stem: &str) -> Option<(&str, &str)> {
    let inner = stem.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let base = inner[..open].strip_suffix(' ')?;
    if base.is_empty() {
        return None;
    }
    Some((base, &inner[open + 1..]))
}

/// `owner's conflicted copy <stamp>` split into its owner and stamp.
fn split_owner(text: &str) -> Option<(&str, &str)> {
    const TOKEN: &str = "'s conflicted copy ";
    let lower = text.to_ascii_lowercase();
    let at = lower.rfind(TOKEN)?;
    let owner = &text[..at];
    if owner.is_empty() {
        return None;
    }
    Some((owner, &text[at + TOKEN.len()..]))
}

/// `text` with `token` stripped off its front, case-insensitively.
fn strip_token<'a>(text: &'a str, token: &str) -> Option<&'a str> {
    let head = text.get(..token.len())?;
    if !head.eq_ignore_ascii_case(token) {
        return None;
    }
    Some(&text[token.len()..])
}

/// Whether a stem mentions any client's conflict vocabulary. Used to keep the
/// ambiguous shapes away from names that were *meant* to be a copy.
fn mentions_conflict(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    lower.contains(".sync-conflict-")
        || lower.contains("conflicted copy")
        || lower.contains("sfconflict")
}

/// The original's name: the client's leftover stem, extension preserved.
fn with_extension(base: &str, ext: Option<&str>) -> String {
    match ext {
        Some(ext) => format!("{base}.{ext}"),
        None => base.to_string(),
    }
}

fn digits(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_digit())
}

/// `YYYY-MM-DD`.
fn dashed_date(text: &str) -> bool {
    let mut parts = text.split('-');
    let (year, month, day, extra) = (parts.next(), parts.next(), parts.next(), parts.next());
    matches!(
        (year, month, day, extra),
        (Some(year), Some(month), Some(day), None)
            if digits(year, 4) && digits(month, 2) && digits(day, 2)
    )
}

/// `YYYY-MM-DD-HH-MM-SS`.
fn dashed_datetime(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    matches!(
        parts.as_slice(),
        [year, month, day, hour, minute, second]
            if digits(year, 4)
                && digits(month, 2)
                && digits(day, 2)
                && digits(hour, 2)
                && digits(minute, 2)
                && digits(second, 2)
    )
}

// The walk.

/// Git's ignore rules, as the repository the ledger lives in states them.
///
/// `root`'s `.gitignore` is honoured by asking libgit2 rather than by growing a
/// second matcher — nested `.gitignore` files, `**` patterns, negations,
/// `.git/info/exclude` and the user's global excludes all come out as git itself
/// applies them. Outside a repository there is no ignore set to honour, so only
/// `.git`/`.ledger` are skipped.
struct Ignore {
    repo: Option<git2::Repository>,
    workdir: Option<PathBuf>,
}

impl Ignore {
    fn open(root: &Path) -> Self {
        let repo = git2::Repository::discover(root).ok();
        let workdir = repo
            .as_ref()
            .and_then(|repo| repo.workdir())
            .map(Path::to_path_buf);
        Self { repo, workdir }
    }

    fn ignored(&self, path: &Path) -> bool {
        let (Some(repo), Some(workdir)) = (self.repo.as_ref(), self.workdir.as_deref()) else {
            return false;
        };
        let Ok(relative) = path.strip_prefix(workdir) else {
            return false;
        };
        // An error means "no answer" — a path outside the work tree, an
        // unreadable ignore file. A file git cannot rule on is scanned rather
        // than dropped: a missed candidate is recoverable, a silent omission is
        // not.
        repo.status_should_ignore(relative).unwrap_or(false)
    }
}

fn walk(dir: &Path, ignore: &Ignore, report: &mut Report) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        // Nowhere a conflicted ledger file can be: one is git's own bookkeeping,
        // the other this app's operation log.
        if name == ".git" || name == ".ledger" {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Symlinks are left alone rather than followed: a sync client
        // materialises real files, and following a link could leave the tree.
        if file_type.is_dir() {
            if ignore.ignored(&path) {
                continue;
            }
            // A subfolder that cannot be listed is a gap, not a failure: the
            // conflicts elsewhere in the tree still deserve reporting.
            let _ = walk(&path, ignore, report);
        } else if file_type.is_file() {
            if ignore.ignored(&path) {
                continue;
            }
            consider(dir, &path, &name, report);
        }
    }
    Ok(())
}

/// Decide what one file in `dir` is: a pair, an orphan, or nothing.
fn consider(dir: &Path, path: &Path, name: &std::ffi::OsStr, report: &mut Report) {
    // A name that is not UTF-8 cannot carry these patterns; there is nothing to
    // parse.
    let Some(file_name) = name.to_str() else {
        return;
    };
    let Some(found) = classify_name(file_name) else {
        return;
    };
    // Never its own original: `..`-free names make this unusual, but the rule
    // costs one comparison and a self-pair would be a merge into itself.
    if found.original == file_name {
        return;
    }
    let original = dir.join(&found.original);
    if original.is_file() {
        report.candidates.push(Candidate {
            original,
            copy: path.to_path_buf(),
            client: found.client,
            stamp: found.stamp,
        });
    } else if found.evidence == Evidence::Token {
        // The name proves the copy, the directory says the original is gone.
        report.orphans.push(Orphan {
            copy: path.to_path_buf(),
            client: found.client,
            stamp: found.stamp,
        });
    }
    // A shape with no original beside it is just a file called `notes (1).bean`.
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::TempDir;

    /// A ledger-ish file at `name`, parents created.
    fn write(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, "2026-01-30 * groceries\n").unwrap();
        path
    }

    fn base(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// `(copy, original, client, stamp)` per candidate, in the scan's order.
    fn pairs(dir: &Path) -> Vec<(String, String, SyncClient, Option<String>)> {
        scan(dir)
            .unwrap()
            .into_iter()
            .map(|c| (base(&c.copy), base(&c.original), c.client, c.stamp))
            .collect()
    }

    #[test]
    fn classify_covers_every_client() {
        // A parser's expectations, one row per name.
        type Expected = Option<(SyncClient, &'static str, Option<&'static str>)>;
        let cases: &[(&str, Expected)] = &[
            // Drive.
            (
                "ledger (1).bean",
                Some((SyncClient::Drive, "ledger.bean", None)),
            ),
            (
                "ledger (12).bean",
                Some((SyncClient::Drive, "ledger.bean", None)),
            ),
            (
                "ledger's conflicted copy 2026-01-30.bean",
                Some((SyncClient::Drive, "ledger.bean", Some("2026-01-30"))),
            ),
            (
                "ledger (Alper's conflicted copy 2026-01-30).bean",
                Some((SyncClient::Drive, "ledger.bean", Some("2026-01-30"))),
            ),
            // Nextcloud.
            (
                "ledger (conflicted copy 2026-01-30 120000).bean",
                Some((
                    SyncClient::Nextcloud,
                    "ledger.bean",
                    Some("2026-01-30 120000"),
                )),
            ),
            // Syncthing.
            (
                "ledger.sync-conflict-20260130-120000-ABCDEFG.bean",
                Some((
                    SyncClient::Syncthing,
                    "ledger.bean",
                    Some("20260130-120000"),
                )),
            ),
            // Seafile.
            (
                "ledger (SFConflict alper 2026-01-30-12-00-00).bean",
                Some((
                    SyncClient::Seafile,
                    "ledger.bean",
                    Some("2026-01-30-12-00-00"),
                )),
            ),
            // OneDrive.
            (
                "ledger-Alper.bean",
                Some((SyncClient::OneDrive, "ledger.bean", None)),
            ),
            (
                "ledger-Alper-PC.bean",
                Some((SyncClient::OneDrive, "ledger.bean", None)),
            ),
            // iCloud.
            (
                "ledger 2.bean",
                Some((SyncClient::ICloud, "ledger.bean", None)),
            ),
            // The extension is preserved, not assumed.
            (
                "ledger.2026 (1).bean",
                Some((SyncClient::Drive, "ledger.2026.bean", None)),
            ),
            ("notes (1)", Some((SyncClient::Drive, "notes", None))),
            (
                "ledger.sync-conflict-20260130-120000-ABCDEFG.txt",
                Some((SyncClient::Syncthing, "ledger.txt", Some("20260130-120000"))),
            ),
            // Ordinary names, and the lookalikes that must not flip a client on.
            ("ledger.bean", None),
            ("ledger 1.bean", None),
            ("ledger (0).bean", None),
            ("(1).bean", None),
            ("'s conflicted copy 2026-01-30.bean", None),
            ("ledger (conflicted copy 2026-01-30).bean", None),
            ("a.sync-conflict-notatimestamp.bean", None),
            ("ledger's conflicted copy.bean", None),
            ("budget-2026-01.bean", None),
            ("ledger-.bean", None),
        ];
        for (name, expected) in cases {
            let expected = expected.map(|(client, original, stamp)| {
                (client, original.to_string(), stamp.map(str::to_string))
            });
            assert_eq!(classify(name), expected, "classify({name:?})");
        }
    }

    #[test]
    fn classify_reports_the_evidence() {
        // A token names itself; a shape needs the directory.
        assert_eq!(
            classify_name("ledger.sync-conflict-20260130-120000-ABCDEFG.bean")
                .unwrap()
                .evidence,
            Evidence::Token
        );
        assert_eq!(
            classify_name("notes (1).bean").unwrap().evidence,
            Evidence::Shape
        );
        assert_eq!(
            classify_name("notes-Alper.bean").unwrap().evidence,
            Evidence::Shape
        );
        assert_eq!(
            classify_name("notes 2.bean").unwrap().evidence,
            Evidence::Shape
        );
        assert!(classify_name("notes.bean").is_none());
    }

    #[test]
    fn owner_qualified_copy_is_one_client() {
        // Drive and Dropbox both write this shape; one copy, one client.
        let (client, ..) = classify("ledger (Alper's conflicted copy 2026-01-30).bean").unwrap();
        assert_eq!(client, SyncClient::Drive);
        // And the whole family is Drive's, so a merge prompt never names two.
        for name in [
            "ledger (1).bean",
            "ledger's conflicted copy 2026-01-30.bean",
            "ledger (Alper's conflicted copy 2026-01-30).bean",
        ] {
            assert_eq!(classify(name).unwrap().0, SyncClient::Drive, "{name}");
        }
    }

    #[test]
    fn serde_names_are_snake_case() {
        let wire = |client| serde_json::to_string(&client).unwrap();
        assert_eq!(wire(SyncClient::Drive), "\"drive\"");
        assert_eq!(wire(SyncClient::Dropbox), "\"dropbox\"");
        assert_eq!(wire(SyncClient::OneDrive), "\"one_drive\"");
        assert_eq!(wire(SyncClient::Nextcloud), "\"nextcloud\"");
        assert_eq!(wire(SyncClient::Syncthing), "\"syncthing\"");
        assert_eq!(wire(SyncClient::Seafile), "\"seafile\"");
        assert_eq!(wire(SyncClient::ICloud), "\"icloud\"");
    }

    #[test]
    fn display_names_the_client_for_the_user() {
        assert_eq!(SyncClient::Drive.to_string(), "Google Drive");
        assert_eq!(SyncClient::ICloud.to_string(), "iCloud");
        assert_eq!(
            format!("{} copy of ledger.bean", SyncClient::OneDrive),
            "OneDrive copy of ledger.bean"
        );
    }

    #[test]
    fn scan_pairs_each_client_with_its_original() {
        let dir = TempDir::new("conflict-names-clients");
        let root = dir.dir();
        let copies = [
            ("ledger (1).bean", SyncClient::Drive, None),
            ("ledger (12).bean", SyncClient::Drive, None),
            (
                "ledger's conflicted copy 2026-01-30.bean",
                SyncClient::Drive,
                Some("2026-01-30"),
            ),
            (
                "ledger (Alper's conflicted copy 2026-01-30).bean",
                SyncClient::Drive,
                Some("2026-01-30"),
            ),
            ("ledger-Alper.bean", SyncClient::OneDrive, None),
            ("ledger-Alper-PC.bean", SyncClient::OneDrive, None),
            (
                "ledger (conflicted copy 2026-01-30 120000).bean",
                SyncClient::Nextcloud,
                Some("2026-01-30 120000"),
            ),
            (
                "ledger.sync-conflict-20260130-120000-ABCDEFG.bean",
                SyncClient::Syncthing,
                Some("20260130-120000"),
            ),
            (
                "ledger (SFConflict alper 2026-01-30-12-00-00).bean",
                SyncClient::Seafile,
                Some("2026-01-30-12-00-00"),
            ),
            ("ledger 2.bean", SyncClient::ICloud, None),
        ];
        write(root, "ledger.bean");
        for (name, ..) in copies {
            write(root, name);
        }

        let found = pairs(root);
        assert_eq!(found.len(), copies.len(), "{found:#?}");
        for (name, client, stamp) in copies {
            let candidate = found
                .iter()
                .find(|(copy, ..)| copy == name)
                .unwrap_or_else(|| panic!("{name} not reported"));
            assert_eq!(candidate.1, "ledger.bean", "{name}");
            assert_eq!(candidate.2, client, "{name}");
            assert_eq!(candidate.3.as_deref(), stamp, "{name}");
        }
        // Every original is the real file, not a name the scan assembled.
        for candidate in scan(root).unwrap() {
            assert_eq!(candidate.original, root.join("ledger.bean"));
        }
    }

    #[test]
    fn scan_keeps_ordinary_lookalikes_out() {
        let dir = TempDir::new("conflict-names-lookalikes");
        let root = dir.dir();
        // Each of these has no original beside it, and none of them proves a
        // conflict by itself.
        for name in [
            "notes (1).bean",
            "notes-Alper.bean",
            "notes 2.bean",
            "notes 1.bean",
            "notes (0).bean",
            "a.sync-conflict-notatimestamp.bean",
            "budget-2026-01.bean",
        ] {
            write(root, name);
        }

        assert_eq!(scan(root).unwrap(), Vec::new());
        assert_eq!(scan_report(root).unwrap().orphans, Vec::new());

        // A directory is not an original either: only a file can be merged into.
        fs::create_dir(root.join("folder.bean")).unwrap();
        write(root, "folder (1).bean");
        assert_eq!(scan(root).unwrap(), Vec::new());
    }

    #[test]
    fn scan_reports_orphans_only_for_names_that_prove_themselves() {
        let dir = TempDir::new("conflict-names-orphans");
        let root = dir.dir();
        let copies = [
            (
                "gone's conflicted copy 2026-01-30.bean",
                SyncClient::Drive,
                Some("2026-01-30"),
            ),
            (
                "gone (conflicted copy 2026-01-30 120000).bean",
                SyncClient::Nextcloud,
                Some("2026-01-30 120000"),
            ),
            (
                "gone.sync-conflict-20260130-120000-ABCDEFG.bean",
                SyncClient::Syncthing,
                Some("20260130-120000"),
            ),
            (
                "gone (SFConflict alper 2026-01-30-12-00-00).bean",
                SyncClient::Seafile,
                Some("2026-01-30-12-00-00"),
            ),
        ];
        for (name, ..) in copies {
            write(root, name);
        }

        let report = scan_report(root).unwrap();
        assert_eq!(report.candidates, Vec::new());
        assert_eq!(report.orphans.len(), copies.len(), "{:#?}", report.orphans);
        for (name, client, stamp) in copies {
            let orphan = report
                .orphans
                .iter()
                .find(|orphan| base(&orphan.copy) == name)
                .unwrap_or_else(|| panic!("{name} not reported"));
            assert_eq!(orphan.client, client, "{name}");
            assert_eq!(orphan.stamp.as_deref(), stamp, "{name}");
            // The orphan names no destination: there is no file to merge into.
            assert_eq!(orphan.copy, root.join(name));
        }

        // Once the original is there, the same name becomes a pair.
        write(root, "gone.bean");
        let report = scan_report(root).unwrap();
        assert_eq!(report.candidates.len(), copies.len());
        assert_eq!(report.orphans, Vec::new());
    }

    #[test]
    fn scan_pairs_within_one_directory_only() {
        let dir = TempDir::new("conflict-names-same-dir");
        let root = dir.dir();
        // The original is in `sub`, the copy in `sub/deep`: not a pair.
        write(root, "sub/ledger.bean");
        write(root, "sub/deep/ledger (1).bean");
        // Same directory: a pair.
        write(root, "sub/sibling.bean");
        write(root, "sub/sibling 2.bean");

        let found = pairs(root);
        assert_eq!(
            found,
            vec![(
                "sibling 2.bean".to_string(),
                "sibling.bean".to_string(),
                SyncClient::ICloud,
                None,
            )],
            "{found:#?}"
        );
        // The mismatched copy is a shape with no original: not even an orphan.
        assert_eq!(scan_report(root).unwrap().orphans, Vec::new());
    }

    #[test]
    fn scan_reads_nested_directories() {
        let dir = TempDir::new("conflict-names-nested");
        let root = dir.dir();
        write(root, "2026/01/ledger.bean");
        write(root, "2026/01/ledger (1).bean");
        write(root, "2026/02/books.bean");
        write(
            root,
            "2026/02/books.sync-conflict-20260130-120000-ABCDEFG.bean",
        );

        let found = pairs(root);
        assert_eq!(
            found,
            vec![
                (
                    "ledger (1).bean".to_string(),
                    "ledger.bean".to_string(),
                    SyncClient::Drive,
                    None,
                ),
                (
                    "books.sync-conflict-20260130-120000-ABCDEFG.bean".to_string(),
                    "books.bean".to_string(),
                    SyncClient::Syncthing,
                    Some("20260130-120000".to_string()),
                ),
            ],
            "{found:#?}"
        );
    }

    #[test]
    fn scan_skips_the_log_and_git_directories() {
        let dir = TempDir::new("conflict-names-skip");
        let root = dir.dir();
        // Both a pair and an orphan inside each skipped directory.
        for skipped in [".ledger", ".git"] {
            write(root, &format!("{skipped}/ledger.bean"));
            write(root, &format!("{skipped}/ledger (1).bean"));
            write(
                root,
                &format!("{skipped}/gone.sync-conflict-20260130-120000-ABCDEFG.bean"),
            );
            // And nested a level deeper, where the walk would find it too.
            write(root, &format!("sub/{skipped}/ledger.bean"));
            write(root, &format!("sub/{skipped}/ledger (1).bean"));
        }
        // A real pair beside them, so the scan itself is known to work.
        write(root, "ledger.bean");
        write(root, "ledger 2.bean");

        let report = scan_report(root).unwrap();
        assert_eq!(report.candidates.len(), 1, "{:#?}", report.candidates);
        assert_eq!(base(&report.candidates[0].copy), "ledger 2.bean");
        assert_eq!(report.orphans, Vec::new());
    }

    #[test]
    fn scan_honours_gitignore() {
        let dir = TempDir::new("conflict-names-gitignore");
        let root = dir.dir();
        git2::Repository::init(root).unwrap();
        fs::write(root.join(".gitignore"), "archive/\n*.bak\n").unwrap();
        // Ignored: a directory and a file, both holding a pair.
        write(root, "archive/ledger.bean");
        write(root, "archive/ledger (1).bean");
        write(root, "old.bak");
        write(root, "old (1).bak");
        // Not ignored: the ledger itself.
        write(root, "ledger.bean");
        write(root, "ledger (1).bean");

        let found = pairs(root);
        assert_eq!(
            found,
            vec![(
                "ledger (1).bean".to_string(),
                "ledger.bean".to_string(),
                SyncClient::Drive,
                None,
            )],
            "{found:#?}"
        );
    }

    #[test]
    fn scan_is_deterministic() {
        let dir = TempDir::new("conflict-names-order");
        let root = dir.dir();
        // Written out of order on purpose: directory order is the filesystem's.
        for (name, copy) in [
            ("zeta.bean", "zeta (1).bean"),
            ("alpha.bean", "alpha (1).bean"),
            ("mid.bean", "mid (1).bean"),
        ] {
            write(root, name);
            write(root, copy);
        }

        let first = scan(root).unwrap();
        let second = scan(root).unwrap();
        assert_eq!(first, second);
        let copies: Vec<PathBuf> = first.iter().map(|c| c.copy.clone()).collect();
        let mut sorted = copies.clone();
        sorted.sort();
        assert_eq!(copies, sorted, "candidates are sorted by copy path");
    }

    #[test]
    fn scan_refuses_an_unreadable_root() {
        let dir = TempDir::new("conflict-names-missing");
        let missing = dir.dir().join("not-there");
        let error = scan(&missing).unwrap_err();
        assert!(error.contains("not-there"), "{error}");
    }

    #[test]
    fn scan_ignores_a_name_with_no_original_stem() {
        let dir = TempDir::new("conflict-names-empty-base");
        let root = dir.dir();
        // `(1).bean` would name `.bean` if the empty stem were accepted, and a
        // copy is never its own original.
        write(root, "(1).bean");
        write(root, ".bean");
        assert!(scan(root).unwrap().is_empty());
    }
}
