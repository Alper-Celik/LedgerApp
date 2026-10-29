// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The glue between a sync client's conflicted copy and the rest of the app:
//! one call that turns (the copy on disk, the file it copies, the ledger's
//! operation log) into a merge a UI can show, and one that writes the answered
//! merge back through the loader-verified edit funnel.
//!
//! The merge itself is [`crate::conflict`]'s: it matches whole directives
//! across `base`, `ours` and `theirs` and settles everything it can. This
//! module only decides *what the three texts are* and turns its plan into
//! shapes the frontend can hold:
//!
//! - `ours` is the original on disk — the file the user is looking at.
//! - `theirs` is the conflicted copy on disk — the other device's version.
//! - `base` is the newest operation in the ledger's log that touched the
//!   original, its pre-image ([`crate::history::pre_image_for`]). The log lives
//!   inside the ledger and travels with it, so both devices' writes are in it.
//!   A ledger that was never written by this app has no such operation: the
//!   base is then empty, every directive reads as *added*, and
//!   [`ConflictView::base_found`] is `false` so the UI can say the merge is
//!   guesswork rather than present it as a three-way merge. Refusing instead
//!   would strand exactly the first sync conflict a ledger ever has.
//!
//! # Applying a merge
//!
//! [`resolve`] rebuilds the same plan, maps the caller's string picks onto
//! [`Choice`], renders, and hands the text to
//! [`edit_named_file_source`](crate::editing::edit_named_file_source) — the
//! same write, reload and rollback path every editor uses, so a merge that the
//! loader rejects leaves the file byte-identical instead of half-written.
//!
//! # Deleting the copy
//!
//! Removing the conflicted copy is the one destructive step here: if it goes
//! while it still holds text the merge did not take, that text is gone from
//! both devices. [`resolve`]'s signature carries no copy, so the only proof
//! that the copy still holds what the picks were made against is what
//! [`compare`] read — and that is why [`compare`] is not a pure read: it
//! records the copy's bytes for the pending merge. [`resolve`] merges against
//! *those* bytes and deletes the copy only when the bytes on disk still equal
//! them; a copy the sync client replaced in the meantime stays, to be compared
//! again rather than merged with stale picks. The record is process-local, so a
//! restart between the two calls refuses ("run the comparison first") instead
//! of guessing.
//!
//! Everything here is about one file. Which copies exist is
//! [`crate::conflict_names`]' answer, and applying the same merge to a whole
//! journal is the caller's loop.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::Serialize;

use crate::conflict::{self, Choice, Classification, Key, Resolution};
use crate::editing::edit_named_file_source;
use crate::history;
use crate::session::AppState;

/// What [`compare`] saw of a conflicted copy, kept until [`resolve`] has used
/// it — the bytes the picks belong to, and the path to delete only while it
/// still holds them.
#[derive(Debug, Clone)]
struct Compared {
    /// The copy, exactly as the caller named it.
    copy: PathBuf,
    /// The copy's bytes at comparison time.
    bytes: Vec<u8>,
}

/// Comparisons that have not been resolved yet, keyed by the original's
/// canonical path so two spellings of the same file share one record.
static COMPARED: LazyLock<Mutex<HashMap<PathBuf, Compared>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Forget a comparison once it has been resolved.
fn forget(original: &Path) {
    COMPARED
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&canonical(original));
}

/// One conflicted copy as the merge UI shows it: the three texts, where the
/// base came from, and the decisions to answer.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ConflictView {
    /// The file the copy belongs to, as the caller named it.
    pub original: String,
    /// The conflicted copy, as the caller named it.
    pub copy: String,
    /// The sync client whose naming produced the copy — "Google Drive",
    /// "Syncthing", … — or empty when the name is not one this app knows
    /// ([`crate::conflict_names`]).
    pub client: String,
    /// Whether a base was found in the ledger's operation log. When `false`
    /// every directive reads as added and the UI should warn.
    pub base_found: bool,
    /// The original's whole text — the merge's `ours`.
    pub ours: String,
    /// The copy's whole text — the merge's `theirs`.
    pub theirs: String,
    /// Counts for the header.
    pub summary: MergeSummaryDto,
    /// One entry per directive, in the order the merged file will have them.
    pub decisions: Vec<DecisionDto>,
}

/// [`crate::conflict::MergeSummary`] as the frontend reads it.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct MergeSummaryDto {
    /// Directives neither side touched.
    pub unchanged: u32,
    /// Directives the merge settled on its own, other than unchanged ones.
    pub auto: u32,
    /// Directives that need a pick.
    pub needs_user: u32,
    /// Decisions whose pairing rests on occurrence order rather than identity.
    pub ambiguous: u32,
    /// All decisions.
    pub total: u32,
    /// Decisions per directive kind, by kind name (`transaction`, `open`, …),
    /// in name order. `u32` because a JS number cannot carry `usize`.
    pub kinds: Vec<(String, u32)>,
}

/// One directive of the plan, as the merge UI shows it.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct DecisionDto {
    /// The identity picks are keyed by: pass it back verbatim in
    /// [`resolve`]'s `picks`.
    pub key: String,
    /// Directive kind: `transaction`, `open`, `balance`, … `option`.
    pub kind: String,
    /// One-line human description, e.g. `2026-01-10 * "Groceries"`.
    pub label: String,
    /// What happened to this directive, as a stable tag for the UI to style:
    /// `unchanged`, `only_ours`, `only_theirs`, `same_change`, `both_added`,
    /// `both_changed`, `deleted_ours`, `deleted_theirs`, `both_deleted`,
    /// `added_ours`, `added_theirs`.
    pub class: String,
    /// Whether [`resolve`] refuses until this key has a pick.
    pub needs_user: bool,
    /// The directive's text as the original has it.
    pub ours: Option<String>,
    /// The directive's text as the copy has it.
    pub theirs: Option<String>,
    /// The directive's text as the base has it — always `None` when
    /// [`ConflictView::base_found`] is `false`.
    pub base: Option<String>,
}

/// Compare a conflicted copy against the file it copies, with the ledger's
/// operation log as the merge base.
///
/// `ledger_dir` is the directory holding the ledger's root journal — the folder
/// `.ledger/log` lives in, which is the folder the app opens the ledger from.
/// `original` is the file on disk the user has; `copy` is the sync client's
/// second version. Both are read from disk, so the caller can compare before
/// opening the ledger.
///
/// Records the copy's bytes for the [`resolve`] that answers this comparison;
/// see the module docs for why.
pub fn compare(ledger_dir: &Path, original: &Path, copy: &Path) -> Result<ConflictView, String> {
    let ours_bytes =
        fs::read(original).map_err(|error| format!("{}: {error}", original.display()))?;
    let copy_bytes = fs::read(copy).map_err(|error| format!("{}: {error}", copy.display()))?;
    let ours = text_of(original, ours_bytes)?;
    let theirs = text_of(copy, copy_bytes.clone())?;

    let log_root = log_root(ledger_dir);
    let (base_found, base) = base_of(&log_root, original)?;
    let plan = conflict::plan(&base, &ours, &theirs, base_found)?;

    let summary = summary_of(&plan);
    let decisions = plan.directives.iter().map(decision_of).collect();
    remember(original, copy, copy_bytes);

    Ok(ConflictView {
        original: original.display().to_string(),
        copy: copy.display().to_string(),
        client: client_of(copy),
        base_found,
        ours,
        theirs,
        summary,
        decisions,
    })
}

/// Write the merge the picks answer and remove the resolved copy.
///
/// `original` must be a file of the open journal (its path is looked up there,
/// so a stale UI cannot write through this into an arbitrary file), and it must
/// have been [`compare`]d in this process — that comparison is what the picks
/// belong to and what proves the copy still holds the bytes the merge took.
///
/// `picks` is one `(key, choice)` per [`DecisionDto`] the UI answered: the
/// decision's `key` verbatim, and exactly one of `ours`, `theirs`, `both` or
/// `drop`. Keys the plan does not know are ignored (a stale selection is not an
/// error); a decision left `needs_user` without a pick makes the whole merge
/// refuse, with the labels listed. The merged text goes through
/// [`edit_named_file_source`](crate::editing::edit_named_file_source): if the
/// loader rejects it the file is put back byte-for-byte and the error is
/// returned.
///
/// The copy is deleted only when its bytes are still the ones [`compare`]
/// read; otherwise it stays for another comparison. Returns the original's
/// path as the journal holds it.
pub fn resolve(
    state: &AppState,
    original: &Path,
    picks: &[(String, String)],
) -> Result<String, String> {
    let compared = compared(original)?;
    let (file, log_root) = open_journal(state, original)?;

    let (base_found, base) = base_of(&log_root, &file)?;
    let ours_bytes = fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    let ours = text_of(&file, ours_bytes)?;
    let theirs = text_of(&compared.copy, compared.bytes.clone())?;
    let plan = conflict::plan(&base, &ours, &theirs, base_found)?;

    let mut choices = HashMap::with_capacity(picks.len());
    for (key, value) in picks {
        let choice = choice_of(value).ok_or_else(|| {
            format!("`{value}` is not a choice for {key}: use ours, theirs, both or drop")
        })?;
        choices.insert(Key::from_string(key.clone()), choice);
    }

    let merged = conflict::render(&plan, &choices)?;
    edit_named_file_source(state, &file.display().to_string(), &merged, None)?;
    // The edit reports success for a file that already held the merged text; the
    // copy may only go once the bytes are actually there, or a stale session
    // would delete the version the reader chose and leave the one they rejected.
    let on_disk = std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    if on_disk != merged.as_bytes() {
        return Err(format!(
            "{} does not hold the merged text; the conflict copy is kept",
            file.display()
        ));
    }
    drop_copy(&compared)?;
    forget(&file);
    Ok(file.display().to_string())
}

// ---------------------------------------------------------------------------
// Texts
// ---------------------------------------------------------------------------

/// The log directory beside a ledger: `.ledger/log` under the folder the ledger
/// was opened from, which is where the history commands write and read it.
fn log_root(ledger_dir: &Path) -> PathBuf {
    ledger_dir.join(crate::conversations::DIRECTORY).join("log")
}

/// The merge base: the newest logged pre-image of `file`, or an empty base and
/// `false` when the log has no operation touching it.
///
/// A base that is not UTF-8 or whose blob was pruned is an error rather than an
/// empty base: silently merging without it would present guesswork as a
/// three-way merge, which is exactly what `base_found: false` exists to avoid
/// claiming.
fn base_of(log_root: &Path, file: &Path) -> Result<(bool, String), String> {
    match history::pre_image_for(log_root, file)? {
        Some(bytes) => Ok((true, text_of(file, bytes)?)),
        None => Ok((false, String::new())),
    }
}

/// `bytes` as the UTF-8 text a merge needs, naming `path` when they are not
/// text — a ledger file that is binary is not something a directive merge can
/// speak about.
fn text_of(path: &Path, bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))
}

/// The sync client whose naming `copy`'s file name carries, as the user's name
/// for it; empty when the name is not a known conflicted-copy shape.
fn client_of(copy: &Path) -> String {
    copy.file_name()
        .and_then(|name| name.to_str())
        .and_then(crate::conflict_names::classify)
        .map(|(client, _, _)| client.to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The comparison record
// ---------------------------------------------------------------------------

/// The comparison recorded for `original`, or a refusal: without it there is
/// nothing that says what the picks were made against, and nothing the copy
/// could be proven unchanged against.
fn compared(original: &Path) -> Result<Compared, String> {
    let key = canonical(original);
    COMPARED
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&key)
        .cloned()
        .ok_or_else(|| {
            format!(
                "no comparison of {} is pending: compare the conflicted copy first, \
                 or its bytes cannot be checked before they are deleted",
                original.display()
            )
        })
}

/// Remember what a comparison read, under the original's canonical path so the
/// two calls agree however the caller spelled it.
///
/// One entry per original: a second comparison replaces the first, which is why
/// `compare` refuses to overwrite one that has not been resolved. The entry is
/// dropped by the resolve that uses it, so a panel left open and closed costs
/// nothing and no copy path outlives the session.
fn remember(original: &Path, copy: &Path, bytes: Vec<u8>) {
    let key = canonical(original);
    COMPARED
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            key,
            Compared {
                copy: copy.to_path_buf(),
                bytes,
            },
        );
}

/// `path` as the filesystem names it, falling back to the path itself when it
/// cannot be canonicalised (it may not exist yet — not true of the files here,
/// but matching on spelling is better than failing).
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Delete the copy the comparison read, but only while it still holds those
/// bytes: a version the sync client wrote after the comparison carries text
/// this merge never saw, and it must survive to be merged itself.
fn drop_copy(compared: &Compared) -> Result<(), String> {
    match fs::read(&compared.copy) {
        Ok(current) if current == compared.bytes => {
            fs::remove_file(&compared.copy).map_err(|error| {
                format!(
                    "the merge was written, but {} could not be removed: {error}",
                    compared.copy.display()
                )
            })
        }
        Ok(_) => Ok(()),
        // Gone already: the client or the user removed it, nothing to do.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "the merge was written, but {} could not be read: {error}",
            compared.copy.display()
        )),
    }
}

// ---------------------------------------------------------------------------
// The open journal
// ---------------------------------------------------------------------------

/// The open journal's own path for `original`, and its log directory.
///
/// The lookup goes through the journal's files rather than the path as given,
/// so the write can only ever land on a file this session loaded and the log is
/// read beside the journal's root (`.ledger` sits next to the root journal,
/// whatever subfolder the conflicted file lives in).
fn open_journal(state: &AppState, original: &Path) -> Result<(PathBuf, PathBuf), String> {
    let guard = state
        .loaded
        .read()
        .unwrap_or_else(|error| error.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is loaded")?;
    let files = loaded.ledger.source_map.files();

    let wanted = canonical(original);
    let found = files
        .iter()
        .find(|file| file.path == original || canonical(&file.path) == wanted)
        .map(|file| file.path.clone())
        .ok_or_else(|| format!("`{}` is not a file of the open journal", original.display()))?;

    let directory = loaded
        .root
        .parent()
        .ok_or_else(|| format!("{} has no directory", loaded.root.display()))?;
    Ok((found, log_root(directory)))
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

fn summary_of(plan: &conflict::MergePlan) -> MergeSummaryDto {
    let summary = conflict::summary(plan);
    MergeSummaryDto {
        unchanged: count(summary.unchanged),
        auto: count(summary.auto),
        needs_user: count(summary.needs_user),
        ambiguous: count(summary.ambiguous),
        total: count(summary.total),
        kinds: summary
            .kinds
            .into_iter()
            .map(|(kind, decisions)| (kind, count(decisions)))
            .collect(),
    }
}

fn decision_of(decision: &conflict::DirectiveDecision) -> DecisionDto {
    DecisionDto {
        key: decision.key.as_str().to_string(),
        kind: decision.kind.clone(),
        label: decision.label.clone(),
        class: class_tag(decision.class).to_string(),
        needs_user: decision.resolution == Resolution::NeedsUser,
        ours: decision.ours.clone(),
        theirs: decision.theirs.clone(),
        base: decision.base.clone(),
    }
}

/// The stable tag for a classification — see [`DecisionDto::class`]. Kept
/// exhaustive here so a new classification cannot slip into the UI unlabelled.
fn class_tag(class: Classification) -> &'static str {
    match class {
        Classification::Unchanged => "unchanged",
        Classification::OnlyOurs => "only_ours",
        Classification::OnlyTheirs => "only_theirs",
        Classification::SameChange => "same_change",
        Classification::BothAdded => "both_added",
        Classification::BothChanged => "both_changed",
        Classification::DeletedOurs => "deleted_ours",
        Classification::DeletedTheirs => "deleted_theirs",
        Classification::BothDeleted => "both_deleted",
        Classification::AddedOurs => "added_ours",
        Classification::AddedTheirs => "added_theirs",
    }
}

/// A count as the frontend carries it: a JS number cannot hold a `usize`.
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// One pick's value, as [`resolve`] accepts it.
fn choice_of(value: &str) -> Option<Choice> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ours" => Some(Choice::Ours),
        "theirs" => Some(Choice::Theirs),
        "both" => Some(Choice::Both),
        "drop" => Some(Choice::Drop),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{self, Change, Origin};
    use crate::testing::{TempDir, loaded_state};

    const BASE: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-01-10 * \"Groceries\"
  Assets:Bank       -50.00 USD
  Expenses:Food      50.00 USD

2026-01-11 * \"Salary\"
  Assets:Bank       200.00 USD
  Income:Salary    -200.00 USD
";

    const BASE_GROCERIES: &str = "2026-01-10 * \"Groceries\"\n  Assets:Bank       -50.00 USD\n  Expenses:Food      50.00 USD\n";
    const OUR_GROCERIES: &str = "2026-01-10 * \"Groceries\"\n  Assets:Bank       -55.00 USD\n  Expenses:Food      55.00 USD\n";
    const THEIR_GROCERIES: &str = "2026-01-10 * \"Groceries\"\n  Assets:Bank       -60.00 USD\n  Expenses:Food      60.00 USD\n";
    const COFFEE: &str =
        "2026-02-01 * \"Coffee\"\n  Assets:Bank       -3.00 USD\n  Expenses:Food      3.00 USD\n";
    const BROKEN: &str =
        "2026-02-01 * \"Broken\"\n  Assets:Bank       -10.00 USD\n  Expenses:Food      9.00 USD\n";

    /// The ledger as this device left it: `ours` on disk, and the log's newest
    /// pre-image for it is `base`.
    struct Case {
        dir: TempDir,
        /// The original, absolute.
        original: PathBuf,
        /// The conflicted copy beside it, under Drive's `name (1).bean` shape.
        copy: PathBuf,
        /// The log root the app writes to.
        log: PathBuf,
    }

    impl Case {
        /// A ledger whose original reads `ours`, whose log's newest pre-image
        /// for it is `base`, and whose conflicted copy reads `theirs`.
        fn new(name: &str, base: &str, ours: &str, theirs: &str) -> Self {
            let dir = TempDir::new(name);
            let original = dir.file("main.bean");
            let copy = dir.file("main (1).bean");
            fs::write(&original, ours).unwrap();
            fs::write(&copy, theirs).unwrap();
            let case = Self {
                log: dir.file(".ledger/log"),
                dir,
                original,
                copy,
            };
            history::record(
                &case.log,
                Origin::User,
                "an edit",
                &[Change::new(
                    &case.original,
                    Some(base.as_bytes()),
                    Some(ours.as_bytes()),
                )],
            )
            .unwrap();
            case
        }

        fn view(&self) -> ConflictView {
            compare(self.dir.dir(), &self.original, &self.copy).expect("compares")
        }

        /// The decision the UI would answer for `label`.
        fn key(&self, view: &ConflictView, label: &str) -> String {
            view.decisions
                .iter()
                .find(|decision| decision.label == label)
                .unwrap_or_else(|| panic!("no decision for {label}: {:?}", view.decisions))
                .key
                .clone()
        }

        fn state(&self) -> AppState {
            loaded_state(&self.original)
        }
    }

    #[test]
    fn compare_uses_the_newest_logged_pre_image_as_its_base() {
        let case = Case::new(
            "base",
            BASE,
            &BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1),
            BASE,
        );
        let view = case.view();

        assert!(view.base_found, "the log has a pre-image for the original");
        assert_eq!(view.client, "Google Drive");
        assert_eq!(view.ours, BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1));
        assert_eq!(view.theirs, BASE);
        assert_eq!(view.summary.total as usize, view.decisions.len());

        // The untouched salary keeps its base text, so the UI can show it.
        let salary = view
            .decisions
            .iter()
            .find(|decision| decision.label == "2026-01-11 * \"Salary\"")
            .unwrap();
        assert!(salary.base.is_some(), "base: {salary:?}");
        assert!(!salary.needs_user);
        assert_eq!(salary.class, "unchanged");

        let groceries = view
            .decisions
            .iter()
            .find(|decision| decision.label == "2026-01-10 * \"Groceries\"")
            .unwrap();
        assert_eq!(groceries.class, "only_ours");
        assert_eq!(groceries.ours.as_deref(), Some(OUR_GROCERIES.trim_end()));
        assert_eq!(groceries.theirs.as_deref(), Some(BASE_GROCERIES.trim_end()));
    }

    #[test]
    fn compare_without_a_logged_operation_has_no_base_and_says_so() {
        let dir = TempDir::new("no-base");
        let original = dir.file("main.bean");
        let copy = dir.file("main (1).bean");
        fs::write(&original, BASE).unwrap();
        fs::write(&copy, BASE).unwrap();

        let view = compare(dir.dir(), &original, &copy).expect("compares");
        assert!(!view.base_found);
        assert_eq!(view.client, "Google Drive");
        assert!(
            view.decisions
                .iter()
                .all(|decision| decision.base.is_none()),
            "decisions: {:?}",
            view.decisions
        );
        // Everything is "added" on both sides; identical directives need no pick.
        assert!(
            view.decisions
                .iter()
                .all(|decision| decision.class == "same_change"),
            "decisions: {:?}",
            view.decisions
        );
        assert_eq!(view.summary.needs_user, 0);
    }

    #[test]
    fn untouched_directives_and_each_sides_edit_merge_automatically() {
        let ours = BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1);
        let theirs = format!("{BASE}\n{COFFEE}");
        let case = Case::new("auto", BASE, &ours, &theirs);
        let view = case.view();

        assert!(
            view.decisions.iter().all(|decision| !decision.needs_user),
            "decisions: {:?}",
            view.decisions
        );
        assert_eq!(view.summary.needs_user, 0);
        assert_eq!(view.summary.auto, 2, "summary: {:?}", view.summary);

        let state = case.state();
        let written = resolve(&state, &case.original, &[]).expect("merges");
        assert_eq!(written, case.original.display().to_string());
        let merged = fs::read_to_string(&case.original).unwrap();
        assert!(merged.contains("-55.00 USD"), "merged: {merged}");
        assert!(merged.contains("Coffee"), "merged: {merged}");
        assert!(merged.contains("-200.00 USD"), "merged: {merged}");
        assert!(!case.copy.exists(), "the resolved copy is gone");

        // The write went through the funnel: the journal this session holds is
        // the reloaded one, and it read the merge without complaint.
        {
            let guard = state
                .loaded
                .read()
                .unwrap_or_else(|error| error.into_inner());
            let loaded = guard.as_ref().expect("the session still holds the ledger");
            assert!(
                loaded.ledger.errors.is_empty(),
                "errors: {:?}",
                loaded.ledger.errors
            );
            assert!(
                loaded.ledger.source_map.files()[0]
                    .source
                    .contains("Coffee")
            );
        }
    }

    /// `pushtag` / `poptag` are not directives: they live in the text between
    /// entries, so a merge that takes an entry from the copy has to bring the
    /// scope it was written in. The frames the renderer emits for that have to
    /// survive the write funnel's reload too — a scope that only exists in the
    /// plan would be worse than no merge at all.
    #[test]
    fn a_scope_the_copy_added_reaches_the_written_journal() {
        // We edited the salary; the copy put the groceries inside a tag scope
        // and changed that amount. (Something has to differ from the base for
        // the log to have an operation to record.)
        let ours = BASE.replace(
            "  Assets:Bank       200.00 USD\n  Income:Salary    -200.00 USD\n",
            "  Assets:Bank       210.00 USD\n  Income:Salary    -210.00 USD\n",
        );
        let scoped = format!("pushtag #trip\n{THEIR_GROCERIES}poptag #trip");
        let theirs = BASE.replacen(BASE_GROCERIES, &scoped, 1);
        let case = Case::new("scope", BASE, &ours, &theirs);
        let view = case.view();
        assert_eq!(
            view.summary.needs_user, 0,
            "decisions: {:?}",
            view.decisions
        );

        let state = case.state();
        resolve(&state, &case.original, &[]).expect("merges");

        let merged = fs::read_to_string(&case.original).unwrap();
        assert!(merged.contains("pushtag #trip"), "merged: {merged}");
        assert!(merged.contains("-60.00 USD"), "merged: {merged}");
        assert!(!case.copy.exists(), "the resolved copy is gone");

        // The reloaded journal, not just the text, carries the tag: that is what
        // the rest of the app reads.
        {
            let guard = state
                .loaded
                .read()
                .unwrap_or_else(|error| error.into_inner());
            let loaded = guard.as_ref().expect("the session still holds the ledger");
            assert!(
                loaded.ledger.errors.is_empty(),
                "errors: {:?}",
                loaded.ledger.errors
            );
            let tagged = loaded.ledger.directives.iter().any(|spanned| {
                matches!(&spanned.value, rustledger_core::Directive::Transaction(transaction)
                    if transaction.tags.iter().any(|tag| tag.as_str() == "trip"))
            });
            assert!(tagged, "the written journal's transaction lost the tag");
        }
    }

    #[test]
    fn a_directive_changed_on_both_sides_refuses_without_a_pick() {
        let ours = BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1);
        let theirs = BASE.replacen(BASE_GROCERIES, THEIR_GROCERIES, 1);
        let case = Case::new("needs-pick", BASE, &ours, &theirs);
        let view = case.view();

        let groceries = view
            .decisions
            .iter()
            .find(|decision| decision.label == "2026-01-10 * \"Groceries\"")
            .unwrap();
        assert_eq!(groceries.class, "both_changed");
        assert!(groceries.needs_user);
        assert_eq!(view.summary.needs_user, 1);

        let before = fs::read(&case.original).unwrap();
        let error = resolve(&case.state(), &case.original, &[]).expect_err("refuses");
        assert!(error.contains("Groceries"), "error: {error}");
        assert_eq!(
            fs::read(&case.original).unwrap(),
            before,
            "the refused merge left the file alone"
        );
        assert!(case.copy.exists(), "the copy survives a refused merge");
    }

    #[test]
    fn a_pick_settles_the_conflict_and_both_keeps_both_versions() {
        let ours = BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1);
        let theirs = BASE.replacen(BASE_GROCERIES, THEIR_GROCERIES, 1);
        let case = Case::new("both", BASE, &ours, &theirs);
        let view = case.view();
        let key = case.key(&view, "2026-01-10 * \"Groceries\"");

        resolve(&case.state(), &case.original, &[(key, "both".to_string())]).expect("merges");

        let merged = fs::read_to_string(&case.original).unwrap();
        assert!(merged.contains("-55.00 USD"), "merged: {merged}");
        assert!(merged.contains("-60.00 USD"), "merged: {merged}");
        assert_eq!(merged.matches("2026-01-10 * \"Groceries\"").count(), 2);
        assert!(rustledger_parser::parse(&merged).errors.is_empty());
        assert!(!case.copy.exists());
    }

    #[test]
    fn a_refused_write_leaves_the_original_byte_identical() {
        // The copy adds an entry that does not balance: the merge renders, the
        // loader rejects it, and the funnel must put the file back untouched.
        let ours = BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1);
        let theirs = format!("{BASE}\n{BROKEN}");
        let case = Case::new("refused", BASE, &ours, &theirs);
        let view = case.view();
        assert_eq!(
            view.summary.needs_user, 0,
            "decisions: {:?}",
            view.decisions
        );

        let before = fs::read(&case.original).unwrap();
        let error = resolve(&case.state(), &case.original, &[]).expect_err("the loader refuses");
        assert!(!error.is_empty());
        assert_eq!(
            fs::read(&case.original).unwrap(),
            before,
            "the refused write left the file byte-identical"
        );
        assert!(case.copy.exists(), "the copy survives a refused write");
    }

    #[test]
    fn the_copy_is_deleted_only_when_it_is_unchanged_since_the_comparison() {
        let ours = BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1);
        let theirs = format!("{BASE}\n{COFFEE}");
        let case = Case::new("copy-changed", BASE, &ours, &theirs);
        let view = case.view();
        assert_eq!(view.summary.needs_user, 0);

        // The sync client replaces the copy between the comparison and the
        // answer: those bytes were never merged, so they must stay.
        let replaced = format!(
            "{theirs}\n2026-03-01 * \"Tea\"\n  Assets:Bank       -1.00 USD\n  Expenses:Food      1.00 USD\n"
        );
        fs::write(&case.copy, &replaced).unwrap();

        let written =
            resolve(&case.state(), &case.original, &[]).expect("merges the compared bytes");
        assert_eq!(written, case.original.display().to_string());
        assert_eq!(fs::read_to_string(&case.copy).unwrap(), replaced);
        let merged = fs::read_to_string(&case.original).unwrap();
        assert!(merged.contains("Coffee"), "merged: {merged}");
        assert!(!merged.contains("Tea"), "merged: {merged}");
    }

    #[test]
    fn resolve_refuses_a_file_that_was_never_compared() {
        let dir = TempDir::new("uncompared");
        let original = dir.file("main.bean");
        let copy = dir.file("main (1).bean");
        fs::write(&original, BASE).unwrap();
        fs::write(&copy, BASE).unwrap();

        let error = resolve(&loaded_state(&original), &original, &[]).expect_err("refuses");
        assert!(error.contains("no comparison"), "error: {error}");
        assert!(copy.exists());
    }

    #[test]
    fn resolve_rejects_an_unknown_choice_value() {
        let ours = BASE.replacen(BASE_GROCERIES, OUR_GROCERIES, 1);
        let theirs = BASE.replacen(BASE_GROCERIES, THEIR_GROCERIES, 1);
        let case = Case::new("bad-pick", BASE, &ours, &theirs);
        let view = case.view();
        let key = case.key(&view, "2026-01-10 * \"Groceries\"");

        let error = resolve(&case.state(), &case.original, &[(key, "mine".to_string())])
            .expect_err("refuses");
        assert!(
            error.contains("ours, theirs, both or drop"),
            "error: {error}"
        );
    }

    #[test]
    fn pre_image_for_returns_the_newest_pre_image_and_none_for_an_unknown_path() {
        let dir = TempDir::new("pre-image");
        let log = dir.file(".ledger/log");
        let path = dir.file("main.bean");
        let other = dir.file("other.bean");

        history::record(
            &log,
            Origin::User,
            "first",
            &[Change::new(&path, Some(b"first\n"), Some(b"second\n"))],
        )
        .unwrap();
        history::record(
            &log,
            Origin::User,
            "second",
            &[Change::new(&path, Some(b"second\n"), Some(b"third\n"))],
        )
        .unwrap();
        history::record(
            &log,
            Origin::User,
            "elsewhere",
            &[Change::new(&other, Some(b"other\n"), Some(b"changed\n"))],
        )
        .unwrap();

        assert_eq!(
            history::pre_image_for(&log, &path).unwrap(),
            Some(b"second\n".to_vec()),
            "the newest operation touching the path wins"
        );
        assert_eq!(
            history::pre_image_for(&log, &other).unwrap(),
            Some(b"other\n".to_vec())
        );
        assert_eq!(
            history::pre_image_for(&log, &dir.file("nowhere.bean")).unwrap(),
            None
        );
    }
}
