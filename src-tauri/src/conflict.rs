// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Three-way merge of one Beancount file, resolved at *directive* granularity.
//!
//! Two devices edit the same journal and a sync step hands us `base`, `ours`
//! and `theirs`. Line-level merge markers are useless here: a directive spans
//! several lines that only mean anything together, so "our postings, their
//! header" is not a ledger — it is a corrupt file that the loader will reject
//! later. This module therefore merges whole directives:
//!
//! 1. [`plan`] parses all three texts, gives every directive a [`Key`] derived
//!    from its *meaning*, matches keys across the three versions, and records
//!    one [`DirectiveDecision`] per match with a [`Classification`] saying what
//!    happened and a [`Resolution`] saying whether the merge can settle it.
//! 2. Callers answer the `NeedsUser` decisions with a [`Choice`] per [`Key`].
//! 3. [`render`] replays `ours` in source order: every byte that no decision
//!    touches is copied verbatim (comments, blank lines, odd indentation and
//!    trailing spaces survive), and only the directives that differ are
//!    emitted from the side the decision picked.
//!
//! # Identity
//!
//! A key is a canonical string built from a directive's primary operands, not
//! from its bytes, so reformatting a file does not look like an edit:
//!
//! - `transaction`: date, flag, payee, narration and the sorted set of
//!   `account=units` pairs (amounts included — see the caveat below).
//! - `balance`: date, account, amount currency.
//! - `open` / `close` / `note`: date, account.
//! - `commodity`: date, currency.
//! - `pad`: date, account.
//! - `event`: date, event type.
//! - `query`: date, name.
//! - `document`: date, account, path.
//! - `price`: date, currency, quote currency.
//! - `custom`: date, custom type.
//! - `option` / `include` / `plugin`: name (these carry no date).
//!
//! When the same key occurs more than once on a side, occurrence order
//! disambiguates and the key gains a `#n` suffix. Identical duplicates are
//! genuinely indistinguishable — which of the two the user edited cannot be
//! recovered from the text — so such decisions are flagged
//! [`ambiguous`](DirectiveDecision::ambiguous) and, where picking the wrong
//! one could drop or duplicate text, escalated to
//! [`Resolution::NeedsUser`].
//!
//! Because amounts are part of a transaction's key, changing an amount changes
//! the key. Matching therefore runs in two passes: exact keys first, then a
//! *loose* key (a transaction's date/flag/payee/narration with the postings
//! dropped, an `open`'s account without its date, and so on) but only where
//! that pairing is unambiguous — one claimant, one candidate. That is what
//! makes "we changed the groceries amount, they added a salary entry" merge
//! automatically instead of looking like a delete plus an add.
//!
//! # Rendering
//!
//! [`render`] walks the plan in the order the merged file will have it and
//! copies `ours`' text between the decisions verbatim, so a directive nobody
//! touched comes out byte-identical to the file the user was looking at.
//!
//! - [`Choice::Ours`] emits `ours`' lines, [`Choice::Theirs`] `theirs`' lines,
//!   [`Choice::Drop`] neither.
//! - [`Choice::Both`] emits `ours`' lines first and then `theirs`' — `ours`
//!   first keeps the file's own order, since a directive that exists on both
//!   sides keeps its place in `ours` and the other version follows it — with a
//!   blank line between them when either original had one.
//! - A directive `ours` does not have (an addition, or a `theirs` version a
//!   pick reinstates) is inserted after the nearest preceding directive of
//!   `theirs` that `ours` also has, or before the first directive when there is
//!   none, so additions land among the neighbours they were written next to.
//! - Line endings and the final-newline habit follow `ours`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use rustledger_core::{Directive, IncompleteAmount, Transaction};
use rustledger_parser::{ParseErrorKind, parse};
use serde::{Deserialize, Serialize};

/// Fragment separator inside a [`Key`]. Escaped in the fragments themselves,
/// so a payee containing `|` cannot forge another directive's identity.
const FRAGMENT: char = '|';

/// What to keep for one directive the merge could not settle by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
pub enum Choice {
    /// Keep the directive as `ours` has it.
    Ours,
    /// Take the directive as `theirs` has it.
    Theirs,
    /// Keep both versions: `ours` first, `theirs` second, blank line between
    /// them when the originals had one.
    Both,
    /// Keep neither.
    Drop,
}

/// What happened to one directive's key across `(base, ours, theirs)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum Classification {
    /// Present in all three, byte-for-byte the same (modulo formatting).
    Unchanged,
    /// Only `ours` changed it; `theirs` still has the base version.
    OnlyOurs,
    /// Only `theirs` changed it; `ours` still has the base version.
    OnlyTheirs,
    /// Both sides made the same change, so it must be taken exactly once.
    SameChange,
    /// Neither side had it in `base` and both added different content.
    BothAdded,
    /// Both sides changed it, differently.
    BothChanged,
    /// `ours` dropped it while `theirs` touched it.
    DeletedOurs,
    /// `theirs` dropped it while `ours` touched it.
    DeletedTheirs,
    /// Neither side has it any more.
    BothDeleted,
    /// Neither side had it in `base`; only `ours` added it.
    AddedOurs,
    /// Neither side had it in `base`; only `theirs` added it.
    AddedTheirs,
}

impl Classification {
    /// One phrase a header or a prompt can use verbatim.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::OnlyOurs => "changed here only",
            Self::OnlyTheirs => "changed there only",
            Self::SameChange => "same change on both sides",
            Self::BothAdded => "added differently on both sides",
            Self::BothChanged => "changed differently on both sides",
            Self::DeletedOurs => "deleted here, changed there",
            Self::DeletedTheirs => "changed here, deleted there",
            Self::BothDeleted => "deleted on both sides",
            Self::AddedOurs => "added here",
            Self::AddedTheirs => "added there",
        }
    }
}

/// Whether a decision is settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum Resolution {
    /// The merge settled this one on its own.
    Auto(Choice),
    /// A human (or an AI suggestion the human accepts) has to pick.
    NeedsUser,
}

/// Identity of one directive within one [`MergePlan`].
///
/// Opaque to callers on purpose: the string is an implementation detail, but
/// it is stable, printable and usable as a map key, which is exactly what the
/// UI and the prompt need to answer a decision.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
#[serde(transparent)]
pub struct Key(String);

impl Key {
    /// The raw identity string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Rebuild a key from the string a caller holds.
    ///
    /// The UI answers decisions in a later call than the one that planned them,
    /// so the key travels as a plain string (see
    /// [`Key`]'s "opaque to callers" note): this is the way back, and the same
    /// identity a plan built from the same texts produces. A key the plan does
    /// not know is ignored by [`render`], so a stale selection costs nothing.
    #[must_use]
    pub fn from_string(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

impl fmt::Display for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One directive, as the merge sees it.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DirectiveDecision {
    /// Identity of the directive (see the module docs).
    pub key: Key,
    /// Directive kind: `transaction`, `open`, `balance`, … `option`,
    /// `include`, `plugin`.
    pub kind: String,
    /// What happened across the three versions.
    pub class: Classification,
    /// Settled, or needs a [`Choice`].
    pub resolution: Resolution,
    /// The key's identity repeated on some side, so occurrence order decided
    /// the pairing. Surfaced so a UI can warn instead of pretending certainty.
    pub ambiguous: bool,
    /// One-line human description, e.g. `2026-01-10 * "Groceries"`.
    pub label: String,
    /// ISO date (`2026-01-10`) for sorting; `None` for `option` / `include` /
    /// `plugin`.
    pub date: Option<String>,
    /// This directive exactly as `ours` has it.
    pub ours: Option<String>,
    /// This directive exactly as `theirs` has it.
    pub theirs: Option<String>,
    /// This directive exactly as `base` has it.
    pub base: Option<String>,
}

/// Counts for a UI header or a prompt preamble.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct MergeSummary {
    /// Directives neither side touched.
    pub unchanged: usize,
    /// Directives the merge settled on its own (so: auto *and* a change).
    pub auto: usize,
    /// Directives that need a [`Choice`].
    pub needs_user: usize,
    /// Decisions whose pairing rests on occurrence order.
    pub ambiguous: usize,
    /// All decisions in the plan.
    pub total: usize,
    /// Decisions per directive kind, so a header can say "3 transactions".
    pub kinds: BTreeMap<String, usize>,
}

/// One rendered piece of the merged file, in source order.
#[derive(Debug, Clone)]
enum Piece {
    /// Verbatim text from `ours`, by index into [`Layout::gaps`].
    Gap(usize),
    /// A decision to emit, by index into [`MergePlan::directives`].
    Decision(usize),
}

/// Everything [`render`] needs beyond the decisions themselves.
#[derive(Debug, Clone)]
struct Layout {
    /// Text from `ours` that no directive covers: comments, blank lines,
    /// `pushtag`/`poptag`, anything the parser does not turn into a directive.
    gaps: Vec<String>,
    /// The merged file, as a sequence of gaps and decisions.
    pieces: Vec<Piece>,
    /// Line ending the output uses: the one `ours` uses.
    newline: &'static str,
    /// Whether `ours` ended with a newline, so the merge mirrors it.
    trailing_newline: bool,
    /// Whether the originals had a blank line at this key, so `Choice::Both`
    /// keeps entries visually apart exactly when the file did.
    blank_before: HashMap<Key, bool>,
}

/// A merge in progress: the decisions, plus the untouched text they sit in.
#[derive(Debug, Clone)]
pub struct MergePlan {
    /// Every directive the merge looked at, in the order it will appear.
    pub directives: Vec<DirectiveDecision>,
    layout: Layout,
}

impl MergePlan {
    /// The decisions that still need a [`Choice`].
    #[must_use]
    pub fn unresolved(&self) -> Vec<&DirectiveDecision> {
        self.directives
            .iter()
            .filter(|decision| decision.resolution == Resolution::NeedsUser)
            .collect()
    }
}

/// Plan a merge of `base`, `ours` and `theirs`.
///
/// Errors only when a side cannot be parsed — the message names the side and
/// the position, because a merge built on a half-understood file would quietly
/// drop the directives the parser did not recognise.
pub fn plan(base: &str, ours: &str, theirs: &str, base_found: bool) -> Result<MergePlan, String> {
    let base = parse_side("base", base)?;
    let ours = parse_side("ours", ours)?;
    let theirs = parse_side("theirs", theirs)?;

    let groups = group(&base, &ours, &theirs);

    let mut group_of_ours: Vec<Option<usize>> = vec![None; ours.blocks.len()];
    for (index, group) in groups.iter().enumerate() {
        if let Some(ours_index) = group.ours {
            group_of_ours[ours_index] = Some(index);
        }
    }

    // A directive `ours` does not have has to be placed *somewhere*. Anchor it
    // after the preceding directive that `ours` does know, walking back through
    // `theirs` so an addition lands next to the neighbours it was written
    // among, or through `base` for a directive both sides deleted (which has no
    // `theirs` entry left to lean on).
    let mut head: Vec<usize> = Vec::new();
    let mut after: Vec<Vec<usize>> = vec![Vec::new(); ours.blocks.len()];
    for (index, group) in groups.iter().enumerate() {
        if group.ours.is_some() {
            continue;
        }
        let (side, position): (&[Block], usize) = match (group.theirs, group.base) {
            (Some(theirs_index), _) => (&theirs.blocks, theirs_index),
            (None, Some(base_index)) => (&base.blocks, base_index),
            (None, None) => continue,
        };
        let anchor = (0..position)
            .rev()
            .find_map(|previous| ours.by_key.get(&side[previous].key).copied());
        match anchor {
            Some(position) => after[position].push(index),
            None => head.push(index),
        }
    }
    for list in &mut after {
        // Group order is creation order: base order first, then additions in
        // the order their side has them, which is the order they must keep.
        list.sort_unstable();
    }
    head.sort_unstable();

    let mut directives = Vec::with_capacity(groups.len());
    let mut blank_before = HashMap::new();
    let mut pieces = Vec::with_capacity(groups.len() * 2 + 1);
    pieces.push(Piece::Gap(0));

    for index in &head {
        push_decision(
            *index,
            &groups,
            &base,
            &ours,
            &theirs,
            base_found,
            &mut directives,
            &mut pieces,
            &mut blank_before,
        );
    }
    for position in 0..ours.blocks.len() {
        if let Some(index) = group_of_ours[position] {
            push_decision(
                index,
                &groups,
                &base,
                &ours,
                &theirs,
                base_found,
                &mut directives,
                &mut pieces,
                &mut blank_before,
            );
        }
        for index in &after[position] {
            push_decision(
                *index,
                &groups,
                &base,
                &ours,
                &theirs,
                base_found,
                &mut directives,
                &mut pieces,
                &mut blank_before,
            );
        }
        pieces.push(Piece::Gap(position + 1));
    }

    Ok(MergePlan {
        directives,
        layout: Layout {
            gaps: ours.gaps,
            pieces,
            newline: ours.newline,
            trailing_newline: ours.trailing_newline,
            blank_before,
        },
    })
}

/// Render the merged file.
///
/// Every [`Resolution::NeedsUser`] decision must appear in `picks`; otherwise
/// nothing is rendered and the error names what is missing. Picks may also
/// override an `Auto` decision. Unknown keys are ignored, so a stale UI
/// selection is not an error.
pub fn render(plan: &MergePlan, picks: &HashMap<Key, Choice>) -> Result<String, String> {
    let unresolved: Vec<&DirectiveDecision> = plan
        .unresolved()
        .into_iter()
        .filter(|decision| !picks.contains_key(&decision.key))
        .collect();
    if !unresolved.is_empty() {
        let missing: Vec<String> = unresolved
            .iter()
            .map(|decision| {
                format!(
                    "{} ({}, {})",
                    decision.label,
                    decision.kind,
                    decision.class.label()
                )
            })
            .collect();
        return Err(format!(
            "{} conflict{} still need{} a choice: {}",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
            if missing.len() == 1 { "s" } else { "" },
            missing.join("; ")
        ));
    }

    let mut out = String::new();
    for piece in &plan.layout.pieces {
        match piece {
            Piece::Gap(index) => out.push_str(&plan.layout.gaps[*index]),
            Piece::Decision(index) => {
                let decision = &plan.directives[*index];
                let choice =
                    picks
                        .get(&decision.key)
                        .copied()
                        .unwrap_or(match decision.resolution {
                            Resolution::Auto(choice) => choice,
                            // Unreachable: NeedsUser without a pick returned above.
                            Resolution::NeedsUser => Choice::Drop,
                        });
                emit(&mut out, decision, choice, &plan.layout);
            }
        }
    }

    if !plan.layout.trailing_newline {
        if out.ends_with("\r\n") {
            out.truncate(out.len() - 2);
        } else if out.ends_with('\n') {
            out.pop();
        }
    }
    Ok(out)
}

/// Counts for a UI header, or the preamble of an AI merge prompt.
#[must_use]
pub fn summary(plan: &MergePlan) -> MergeSummary {
    let mut summary = MergeSummary::default();
    for decision in &plan.directives {
        summary.total += 1;
        *summary.kinds.entry(decision.kind.clone()).or_default() += 1;
        if decision.ambiguous {
            summary.ambiguous += 1;
        }
        match decision.resolution {
            Resolution::NeedsUser => summary.needs_user += 1,
            Resolution::Auto(_) => {
                if decision.class == Classification::Unchanged {
                    summary.unchanged += 1;
                } else {
                    summary.auto += 1;
                }
            }
        }
    }
    summary
}

// ---------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn push_decision(
    index: usize,
    groups: &[Group],
    base: &Side,
    ours: &Side,
    theirs: &Side,
    base_found: bool,
    directives: &mut Vec<DirectiveDecision>,
    pieces: &mut Vec<Piece>,
    blank_before: &mut HashMap<Key, bool>,
) {
    let group = &groups[index];
    let blank = group.ours.is_some_and(|i| ours.blocks[i].blank_before)
        || group.theirs.is_some_and(|i| theirs.blocks[i].blank_before);
    let decision = decision_of(group, base, ours, theirs, base_found);
    blank_before.insert(decision.key.clone(), blank);
    pieces.push(Piece::Decision(directives.len()));
    directives.push(decision);
}

fn decision_of(
    group: &Group,
    base: &Side,
    ours: &Side,
    theirs: &Side,
    base_found: bool,
) -> DirectiveDecision {
    let base_block = group.base.map(|index| &base.blocks[index]);
    let ours_block = group.ours.map(|index| &ours.blocks[index]);
    let theirs_block = group.theirs.map(|index| &theirs.blocks[index]);

    let class = classify(base_block, ours_block, theirs_block);
    let mut resolution = resolution_of(class);

    let identity = base_block
        .map(|block| block.identity.as_str())
        .or_else(|| ours_block.map(|block| block.identity.as_str()))
        .or_else(|| theirs_block.map(|block| block.identity.as_str()))
        .unwrap_or_default();
    let ambiguous = base.identity_counts.get(identity).copied().unwrap_or(0) > 1
        || ours.identity_counts.get(identity).copied().unwrap_or(0) > 1
        || theirs.identity_counts.get(identity).copied().unwrap_or(0) > 1;

    // An addition that may be a directive the matcher failed to pair would
    // duplicate the entry if it were auto-kept: ask instead.
    if group.uncertain {
        resolution = Resolution::NeedsUser;
    }

    // With no logged base there is nothing to compare against, so every
    // difference is a whole directive: taking `theirs` automatically could
    // duplicate an entry the log does not know was edited here.
    if !base_found
        && matches!(
            class,
            Classification::OnlyTheirs | Classification::AddedTheirs
        )
    {
        resolution = Resolution::NeedsUser;
    }

    // Every classification that could lose or duplicate data already needs a
    // human. The two that would silently pull text out of an ambiguous
    // neighbourhood are the ones taking `theirs`' version, so those escalate.
    if ambiguous
        && matches!(
            class,
            Classification::OnlyTheirs | Classification::AddedTheirs
        )
    {
        resolution = Resolution::NeedsUser;
    }

    DirectiveDecision {
        key: group.key.clone(),
        kind: group.kind.to_string(),
        class,
        resolution,
        ambiguous,
        label: group.label.clone(),
        date: group.date.clone(),
        ours: ours_block.map(|block| block.text.clone()),
        theirs: theirs_block.map(|block| block.text.clone()),
        base: base_block.map(|block| block.text.clone()),
    }
}

fn classify(base: Option<&Block>, ours: Option<&Block>, theirs: Option<&Block>) -> Classification {
    match (base, ours, theirs) {
        (None, Some(_), None) => Classification::AddedOurs,
        (None, None, Some(_)) => Classification::AddedTheirs,
        (None, Some(ours), Some(theirs)) => {
            if ours.canonical == theirs.canonical {
                Classification::SameChange
            } else {
                Classification::BothAdded
            }
        }
        (Some(_), Some(_), None) => Classification::DeletedTheirs,
        (Some(_), None, Some(_)) => Classification::DeletedOurs,
        (Some(_), None, None) => Classification::BothDeleted,
        (Some(base), Some(ours), Some(theirs)) => {
            let ours_changed = ours.canonical != base.canonical;
            let theirs_changed = theirs.canonical != base.canonical;
            match (ours_changed, theirs_changed) {
                (false, false) => Classification::Unchanged,
                (true, false) => Classification::OnlyOurs,
                (false, true) => Classification::OnlyTheirs,
                (true, true) => {
                    if ours.canonical == theirs.canonical {
                        Classification::SameChange
                    } else {
                        Classification::BothChanged
                    }
                }
            }
        }
        // A group always has at least one side; kept total so a future matching
        // change cannot panic here.
        (None, None, None) => Classification::BothDeleted,
    }
}

/// The auto-resolution table. Deletions always ask: dropping a directive is the
/// one outcome a merge cannot undo, so "they deleted it, we did not" is still a
/// question, not a default.
const fn resolution_of(class: Classification) -> Resolution {
    match class {
        Classification::Unchanged
        | Classification::OnlyOurs
        | Classification::SameChange
        | Classification::AddedOurs => Resolution::Auto(Choice::Ours),
        Classification::OnlyTheirs | Classification::AddedTheirs => {
            Resolution::Auto(Choice::Theirs)
        }
        Classification::BothDeleted => Resolution::Auto(Choice::Drop),
        Classification::BothAdded
        | Classification::BothChanged
        | Classification::DeletedOurs
        | Classification::DeletedTheirs => Resolution::NeedsUser,
    }
}

fn emit(out: &mut String, decision: &DirectiveDecision, choice: Choice, layout: &Layout) {
    match choice {
        Choice::Ours => push_block(out, decision.ours.as_deref(), layout.newline),
        Choice::Theirs => push_block(out, decision.theirs.as_deref(), layout.newline),
        Choice::Both => {
            push_block(out, decision.ours.as_deref(), layout.newline);
            if decision.ours.is_some()
                && decision.theirs.is_some()
                && layout
                    .blank_before
                    .get(&decision.key)
                    .copied()
                    .unwrap_or(false)
            {
                out.push_str(layout.newline);
            }
            push_block(out, decision.theirs.as_deref(), layout.newline);
        }
        Choice::Drop => {}
    }
}

fn push_block(out: &mut String, text: Option<&str>, newline: &str) {
    let Some(text) = text else {
        return;
    };
    if text.is_empty() {
        return;
    }
    // Block text is stored with `\n` endings; emit it with `ours`' own ending
    // so a CRLF file does not end up with mixed line terminators.
    let mut first = true;
    for line in text.split('\n') {
        if !first {
            out.push_str(newline);
        }
        out.push_str(line);
        first = false;
    }
    out.push_str(newline);
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// One directive the merge has to decide about, and where it sits.
#[derive(Debug, Clone)]
struct Group {
    key: Key,
    kind: &'static str,
    date: Option<String>,
    label: String,
    base: Option<usize>,
    ours: Option<usize>,
    theirs: Option<usize>,
    /// An addition whose pairing could not be established: it sits beside a
    /// same-kind directive one side dropped, so it may be that directive after an
    /// edit the matcher could not follow. Auto-keeping or auto-taking it could
    /// duplicate the entry, so it asks.
    uncertain: bool,
}

fn group(base: &Side, ours: &Side, theirs: &Side) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut ours_taken = vec![false; ours.blocks.len()];
    let mut theirs_taken = vec![false; theirs.blocks.len()];

    // 1. Each base directive opens a group and pulls in both exact-keyed sides.
    //    A key with several copies is matched by content first: pairing `#0` with
    //    `#0` purely by position mis-pairs the moment one side inserts a copy that
    //    differs in a field the key does not carry (a cost, a tag).
    for (index, block) in base.blocks.iter().enumerate() {
        let ours_index = same_key(&ours.blocks, &ours_taken, block);
        let theirs_index = same_key(&theirs.blocks, &theirs_taken, block);
        if let Some(ours_index) = ours_index {
            ours_taken[ours_index] = true;
        }
        if let Some(theirs_index) = theirs_index {
            theirs_taken[theirs_index] = true;
        }
        groups.push(Group {
            key: block.key.clone(),
            kind: block.kind,
            date: block.date.clone(),
            label: block.label.clone(),
            base: Some(index),
            ours: ours_index,
            theirs: theirs_index,
            uncertain: false,
        });
    }

    let mut free_ours: Vec<usize> = (0..ours.blocks.len())
        .filter(|index| !ours_taken[*index])
        .collect();
    let mut free_theirs: Vec<usize> = (0..theirs.blocks.len())
        .filter(|index| !theirs_taken[*index])
        .collect();

    // 2. Additions both sides made under the same key are one group, so an
    //    identical addition renders once instead of twice.
    let theirs_by_key: HashMap<&Key, usize> = free_theirs
        .iter()
        .map(|index| (&theirs.blocks[*index].key, *index))
        .collect();
    let mut taken_ours: HashSet<usize> = HashSet::new();
    let mut taken_theirs: HashSet<usize> = HashSet::new();
    for index in &free_ours {
        let Some(&theirs_index) = theirs_by_key.get(&ours.blocks[*index].key) else {
            continue;
        };
        taken_ours.insert(*index);
        taken_theirs.insert(theirs_index);
        groups.push(Group {
            key: ours.blocks[*index].key.clone(),
            kind: ours.blocks[*index].kind,
            date: ours.blocks[*index].date.clone(),
            label: ours.blocks[*index].label.clone(),
            base: None,
            ours: Some(*index),
            theirs: Some(theirs_index),
            uncertain: false,
        });
    }
    free_ours.retain(|index| !taken_ours.contains(index));
    free_theirs.retain(|index| !taken_theirs.contains(index));

    // 3. Loose pairing: an edit that changed the key (a new amount, a new date)
    //    still belongs to the same directive as long as exactly one base
    //    directive and exactly one side directive can claim each other.
    attach_loose(&mut groups, base, &ours.blocks, &mut free_ours, true);
    attach_loose(&mut groups, base, &theirs.blocks, &mut free_theirs, false);

    // 4. Additions only one side made: pair the two sides when a new directive
    //    of ours and one of theirs are the same entry written twice, otherwise
    //    leave them as independent additions.
    let claimants: Vec<(usize, String)> = free_ours
        .iter()
        .map(|index| (*index, ours.blocks[*index].loose.clone()))
        .collect();
    for (ours_index, theirs_index) in pair_by_loose(&claimants, &free_theirs, &theirs.blocks) {
        groups.push(Group {
            key: ours.blocks[ours_index].key.clone(),
            kind: ours.blocks[ours_index].kind,
            date: ours.blocks[ours_index].date.clone(),
            label: ours.blocks[ours_index].label.clone(),
            base: None,
            ours: Some(ours_index),
            theirs: Some(theirs_index),
            uncertain: false,
        });
        taken_ours.insert(ours_index);
        taken_theirs.insert(theirs_index);
    }
    free_ours.retain(|index| !taken_ours.contains(index));
    free_theirs.retain(|index| !taken_theirs.contains(index));

    for index in free_ours {
        groups.push(Group {
            key: ours.blocks[index].key.clone(),
            kind: ours.blocks[index].kind,
            date: ours.blocks[index].date.clone(),
            label: ours.blocks[index].label.clone(),
            base: None,
            ours: Some(index),
            theirs: None,
            uncertain: false,
        });
    }
    for index in free_theirs {
        groups.push(Group {
            key: theirs.blocks[index].key.clone(),
            kind: theirs.blocks[index].kind,
            date: theirs.blocks[index].date.clone(),
            label: theirs.blocks[index].label.clone(),
            base: None,
            ours: None,
            theirs: Some(index),
            uncertain: false,
        });
    }

    escalate_failed_pairings(&mut groups);
    groups
}

/// The side block that is the same directive as `base`: the untaken block with
/// the same key and the same folded text first, then the first untaken one with
/// that key — a copy whose text moved is still that directive, and only the
/// *order* of equally-spelled duplicates is a guess.
fn same_key(blocks: &[Block], taken: &[bool], base: &Block) -> Option<usize> {
    let mut by_content = None;
    let mut by_key = None;
    for (index, block) in blocks.iter().enumerate() {
        if taken[index] || block.key != base.key {
            continue;
        }
        by_key.get_or_insert(index);
        if block.canonical == base.canonical {
            by_content = Some(index);
            break;
        }
    }
    by_content.or(by_key)
}

/// Escalate the additions that could duplicate a directive.
///
/// The matcher pairs by identity, then by a loose key. Each side editing a
/// different field of one transaction defeats both: the base directive loses
/// both sides, and the two edits arrive as independent additions. Auto-keeping
/// them would count the entry twice with nothing on screen saying so, so an
/// addition that sits beside a same-kind directive one side dropped asks
/// instead.
fn escalate_failed_pairings(groups: &mut [Group]) {
    let kinds_in_base: HashSet<&'static str> = groups
        .iter()
        .filter(|group| group.base.is_some())
        .map(|group| group.kind)
        .collect();
    // Which kinds this side kept at all: a side that kept a same-kind base
    // directive and *also* added one is adding a new entry, not losing one.
    let kept_by_ours: HashSet<&'static str> = groups
        .iter()
        .filter(|group| group.base.is_some() && group.ours.is_some())
        .map(|group| group.kind)
        .collect();
    let kept_by_theirs: HashSet<&'static str> = groups
        .iter()
        .filter(|group| group.base.is_some() && group.theirs.is_some())
        .map(|group| group.kind)
        .collect();

    for group in groups.iter_mut().filter(|group| group.base.is_none()) {
        let known_kind = kinds_in_base.contains(group.kind);
        let suspect_ours = known_kind && group.ours.is_some() && !kept_by_ours.contains(group.kind);
        let suspect_theirs =
            known_kind && group.theirs.is_some() && !kept_by_theirs.contains(group.kind);
        if suspect_ours || suspect_theirs {
            group.uncertain = true;
        }
    }
}

/// Attach loose-keyed leftovers to the base groups that lost their exact match.
fn attach_loose(
    groups: &mut [Group],
    base: &Side,
    side: &[Block],
    free: &mut Vec<usize>,
    ours: bool,
) {
    let claimants: Vec<(usize, String)> = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| {
            group.base.is_some() && (if ours { group.ours } else { group.theirs }).is_none()
        })
        .filter_map(|(index, group)| {
            group
                .base
                .map(|base_index| (index, base.blocks[base_index].loose.clone()))
        })
        .collect();

    let mut taken: HashSet<usize> = HashSet::new();
    for (group_index, side_index) in pair_by_loose(&claimants, free, side) {
        if ours {
            groups[group_index].ours = Some(side_index);
        } else {
            groups[group_index].theirs = Some(side_index);
        }
        taken.insert(side_index);
    }
    free.retain(|index| !taken.contains(index));
}

/// Pair claimants with candidates one-to-one, and only where the loose key is
/// unique on both sides. Anything less certain is left to the caller, which
/// turns it into an explicit add or delete the user can see.
fn pair_by_loose(
    claimants: &[(usize, String)],
    free: &[usize],
    blocks: &[Block],
) -> Vec<(usize, usize)> {
    if claimants.is_empty() || free.is_empty() {
        return Vec::new();
    }

    let mut candidates: HashMap<&str, Vec<usize>> = HashMap::new();
    for index in free {
        candidates
            .entry(blocks[*index].loose.as_str())
            .or_default()
            .push(*index);
    }
    let mut demand: HashMap<&str, usize> = HashMap::new();
    for (_, loose) in claimants {
        *demand.entry(loose.as_str()).or_default() += 1;
    }

    let mut pairs = Vec::new();
    for (claimant, loose) in claimants {
        if demand.get(loose.as_str()).copied().unwrap_or(0) != 1 {
            continue;
        }
        let Some(candidates) = candidates.get(loose.as_str()) else {
            continue;
        };
        if let [only] = candidates.as_slice() {
            pairs.push((*claimant, *only));
        }
    }
    pairs
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// One directive of one side: where it is, what it means, how it reads.
#[derive(Debug, Clone)]
struct Raw {
    start: usize,
    end: usize,
    identity: String,
    loose: String,
    kind: &'static str,
    date: Option<String>,
    label: String,
}

/// A directive that has been cut out of its file.
#[derive(Debug, Clone)]
struct Block {
    key: Key,
    identity: String,
    loose: String,
    kind: &'static str,
    date: Option<String>,
    label: String,
    /// Source lines, line endings normalised to `\n`, trailing newline removed.
    text: String,
    /// [`text`](Self::text) folded for comparison only.
    canonical: String,
    /// Whether the original had a blank line right before this directive.
    blank_before: bool,
}

/// One side of the merge: its directives, and the text between them.
#[derive(Debug, Clone)]
struct Side {
    blocks: Vec<Block>,
    /// `gaps.len() == blocks.len() + 1`; always exactly as the file had it.
    gaps: Vec<String>,
    by_key: HashMap<Key, usize>,
    identity_counts: HashMap<String, usize>,
    newline: &'static str,
    trailing_newline: bool,
}

fn parse_side(name: &'static str, text: &str) -> Result<Side, String> {
    let parsed = parse(text);
    // A directive's span is only trustworthy when the parse reached it cleanly;
    // the recoverable diagnostics below describe tag bookkeeping and the
    // deprecated `|` separator, which leave spans intact.
    if let Some(error) = parsed
        .errors
        .iter()
        .find(|error| !is_span_safe(&error.kind))
    {
        let (line, column) = line_column(text, error.span.start);
        return Err(format!("{name} does not parse: {error} at {line}:{column}"));
    }

    let mut raw: Vec<Raw> = Vec::with_capacity(parsed.directives.len());
    for spanned in &parsed.directives {
        let value = &spanned.value;
        raw.push(Raw {
            start: spanned.span.start,
            end: spanned.span.end,
            identity: identity(value),
            loose: loose_identity(value),
            kind: value.type_name(),
            date: Some(value.date().to_string()),
            label: label(value),
        });
    }
    // `option`, `include` and `plugin` are not `Directive`s, but they are text
    // one device can add while the other edits elsewhere, so they merge too.
    for (option, value, span) in &parsed.options {
        raw.push(Raw {
            start: span.start,
            end: span.end,
            identity: sig("option", &[option]),
            loose: sig("option", &[option]),
            kind: "option",
            date: None,
            label: format!("option \"{option}\" \"{value}\""),
        });
    }
    for (path, span) in &parsed.includes {
        raw.push(Raw {
            start: span.start,
            end: span.end,
            identity: sig("include", &[path]),
            loose: sig("include", &[path]),
            kind: "include",
            date: None,
            label: format!("include \"{path}\""),
        });
    }
    for (plugin, module, span) in &parsed.plugins {
        let label = match module {
            Some(module) => format!("plugin \"{plugin}\" \"{module}\""),
            None => format!("plugin \"{plugin}\""),
        };
        raw.push(Raw {
            start: span.start,
            end: span.end,
            identity: sig("plugin", &[plugin]),
            loose: sig("plugin", &[plugin]),
            kind: "plugin",
            date: None,
            label,
        });
    }
    raw.sort_by_key(|raw| (raw.start, raw.end));

    let starts = line_starts(text);
    let mut blocks: Vec<Block> = Vec::with_capacity(raw.len());
    let mut gaps: Vec<String> = Vec::with_capacity(raw.len() + 1);
    let mut identity_counts: HashMap<String, usize> = HashMap::new();
    let mut by_key: HashMap<Key, usize> = HashMap::new();
    let mut cursor = 0usize;
    let mut next_line = 0usize;

    for raw in raw {
        // Spans point at content, not at line starts, and a directive node
        // carries the trivia around it. Widen to whole lines so the partition
        // never cuts a line, then trim that trivia back out: comments and blank
        // lines belong to the page, not to the entry, and must stay in the gap
        // where replacing the directive cannot swallow them.
        let mut start_line = line_of(&starts, raw.start);
        let mut end_line = line_exclusive(&starts, raw.end)
            .max(start_line + 1)
            .min(starts.len());
        while start_line + 1 < end_line && is_trivia_line(line_text(&starts, text, start_line)) {
            start_line += 1;
        }
        // Comments belong to the page as much as blank lines do: leaving them in
        // the block makes a comment-only edit look like an edit to the entry, and
        // a `Both` merge emit the same comment twice.
        while end_line > start_line + 1 && is_trivia_line(line_text(&starts, text, end_line - 1)) {
            end_line -= 1;
        }

        let start_line = start_line.max(next_line);
        if start_line >= starts.len() {
            continue;
        }
        let end_line = end_line.max(start_line + 1).min(starts.len());
        let mut start = starts[start_line];
        // A byte-order mark belongs to the file, not to its first directive:
        // keeping it in the block would duplicate it when a block from another
        // side is emitted at the head, and the parser rejects one mid-file.
        if start == 0 {
            if let Some(rest) = text.strip_prefix('\u{feff}') {
                start = text.len() - rest.len();
            }
        }
        let end = line_offset(&starts, text, end_line);
        if end <= start {
            continue;
        }

        let gap = slice(text, cursor, start)?;
        let blank_before = ends_with_blank_line(gap);
        let block_text = normalize_block(slice(text, start, end)?);
        gaps.push(gap.to_string());
        cursor = end;
        next_line = end_line;

        let ordinal = identity_counts.entry(raw.identity.clone()).or_insert(0);
        let key = Key(format!("{}#{ordinal}", raw.identity));
        *ordinal += 1;
        by_key.insert(key.clone(), blocks.len());
        blocks.push(Block {
            key,
            identity: raw.identity,
            loose: raw.loose,
            kind: raw.kind,
            date: raw.date,
            label: raw.label,
            canonical: canonical(&block_text),
            blank_before,
            text: block_text,
        });
    }
    gaps.push(slice(text, cursor, text.len())?.to_string());

    Ok(Side {
        blocks,
        gaps,
        by_key,
        identity_counts,
        newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
        trailing_newline: text.ends_with('\n'),
    })
}

/// Parser diagnostics that do not move a directive's span.
fn is_span_safe(kind: &ParseErrorKind) -> bool {
    match kind {
        ParseErrorKind::DeprecatedPipeSymbol
        | ParseErrorKind::MissingNewline
        | ParseErrorKind::UnclosedPushtag(_)
        | ParseErrorKind::InvalidPoptag(_)
        | ParseErrorKind::UnclosedPushmeta(_)
        | ParseErrorKind::InvalidPopmeta(_) => true,
        _ => false,
    }
}

/// `kind|fragment|fragment…`, with `|` and `\` escaped so a payee containing a
/// separator cannot address another directive's key.
fn sig(kind: &str, fragments: &[&str]) -> String {
    let mut out = String::with_capacity(kind.len() + fragments.len() * 24);
    out.push_str(kind);
    for fragment in fragments {
        out.push(FRAGMENT);
        for character in fragment.chars() {
            if character == '\\' || character == FRAGMENT {
                out.push('\\');
            }
            out.push(character);
        }
    }
    out
}

/// What makes two versions of a directive *the same* directive.
fn identity(value: &Directive) -> String {
    match value {
        Directive::Transaction(transaction) => {
            let mut postings: Vec<String> = transaction
                .postings
                .iter()
                .map(|posting| {
                    let units = posting
                        .value
                        .units
                        .as_ref()
                        .map_or_else(String::new, amount_fragment);
                    format!("{}={units}", posting.value.account)
                })
                .collect();
            postings.sort();
            postings.dedup();
            sig(
                "transaction",
                &[
                    &transaction.date.to_string(),
                    &transaction.flag.to_string(),
                    transaction
                        .payee
                        .as_ref()
                        .map_or("", |payee| payee.as_str()),
                    transaction.narration.as_str(),
                    &postings.join(","),
                ],
            )
        }
        Directive::Balance(balance) => sig(
            "balance",
            &[
                &balance.date.to_string(),
                balance.account.as_str(),
                balance.amount.currency.as_str(),
            ],
        ),
        Directive::Open(open) => sig("open", &[&open.date.to_string(), open.account.as_str()]),
        Directive::Close(close) => sig("close", &[&close.date.to_string(), close.account.as_str()]),
        Directive::Commodity(commodity) => sig(
            "commodity",
            &[&commodity.date.to_string(), commodity.currency.as_str()],
        ),
        Directive::Pad(pad) => sig("pad", &[&pad.date.to_string(), pad.account.as_str()]),
        Directive::Event(event) => sig("event", &[&event.date.to_string(), &event.event_type]),
        Directive::Query(query) => sig("query", &[&query.date.to_string(), &query.name]),
        Directive::Note(note) => sig("note", &[&note.date.to_string(), note.account.as_str()]),
        Directive::Document(document) => sig(
            "document",
            &[
                &document.date.to_string(),
                document.account.as_str(),
                &document.path,
            ],
        ),
        Directive::Price(price) => sig(
            "price",
            &[
                &price.date.to_string(),
                price.currency.as_str(),
                price.amount.currency.as_str(),
            ],
        ),
        Directive::Custom(custom) => {
            sig("custom", &[&custom.date.to_string(), &custom.custom_type])
        }
    }
}

/// The coarser identity used to recognise *edits*: the same directive with its
/// mutable part changed (an amount, a date) keeps the same loose identity.
fn loose_identity(value: &Directive) -> String {
    match value {
        Directive::Transaction(transaction) => sig(
            "transaction",
            &[
                &transaction.date.to_string(),
                &transaction.flag.to_string(),
                transaction
                    .payee
                    .as_ref()
                    .map_or("", |payee| payee.as_str()),
                transaction.narration.as_str(),
            ],
        ),
        Directive::Balance(balance) => sig(
            "balance",
            &[balance.account.as_str(), balance.amount.currency.as_str()],
        ),
        Directive::Open(open) => sig("open", &[open.account.as_str()]),
        Directive::Close(close) => sig("close", &[close.account.as_str()]),
        Directive::Commodity(commodity) => sig("commodity", &[commodity.currency.as_str()]),
        Directive::Pad(pad) => sig("pad", &[pad.account.as_str()]),
        Directive::Event(event) => sig("event", &[&event.event_type]),
        Directive::Query(query) => sig("query", &[&query.name]),
        Directive::Note(note) => sig("note", &[note.account.as_str()]),
        Directive::Document(document) => {
            sig("document", &[document.account.as_str(), &document.path])
        }
        Directive::Price(price) => sig(
            "price",
            &[price.currency.as_str(), price.amount.currency.as_str()],
        ),
        Directive::Custom(custom) => sig("custom", &[&custom.custom_type]),
    }
}

fn amount_fragment(units: &IncompleteAmount) -> String {
    match units {
        IncompleteAmount::Complete(amount) => {
            format!("{} {}", amount.number.normalize(), amount.currency)
        }
        IncompleteAmount::NumberOnly(number) => number.normalize().to_string(),
        IncompleteAmount::CurrencyOnly(currency) => currency.to_string(),
    }
}

/// One line a human reads when deciding, e.g. `2026-01-10 * "Groceries"`.
fn label(value: &Directive) -> String {
    match value {
        Directive::Transaction(transaction) => transaction_label(transaction),
        Directive::Balance(balance) => format!(
            "{} balance {} {}",
            balance.date, balance.account, balance.amount
        ),
        Directive::Open(open) => format!("{} open {}", open.date, open.account),
        Directive::Close(close) => format!("{} close {}", close.date, close.account),
        Directive::Commodity(commodity) => {
            format!("{} commodity {}", commodity.date, commodity.currency)
        }
        Directive::Pad(pad) => format!("{} pad {} {}", pad.date, pad.account, pad.source_account),
        Directive::Event(event) => format!("{} event \"{}\"", event.date, event.event_type),
        Directive::Query(query) => format!("{} query \"{}\"", query.date, query.name),
        Directive::Note(note) => format!("{} note {}", note.date, note.account),
        Directive::Document(document) => {
            format!(
                "{} document {} \"{}\"",
                document.date, document.account, document.path
            )
        }
        Directive::Price(price) => {
            format!("{} price {} {}", price.date, price.currency, price.amount)
        }
        Directive::Custom(custom) => format!("{} custom \"{}\"", custom.date, custom.custom_type),
    }
}

fn transaction_label(transaction: &Transaction) -> String {
    let narration = escape(&transaction.narration);
    match &transaction.payee {
        Some(payee) => format!(
            "{} {} {} | {}",
            transaction.date,
            transaction.flag,
            escape(payee.as_str()),
            narration
        ),
        None => format!("{} {} {}", transaction.date, transaction.flag, narration),
    }
}

fn escape(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "'"))
}

// ---------------------------------------------------------------------------
// Text handling
// ---------------------------------------------------------------------------

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// Index of the line containing `offset`.
fn line_of(starts: &[usize], offset: usize) -> usize {
    starts
        .partition_point(|start| *start <= offset)
        .saturating_sub(1)
}

/// Number of lines that begin before `offset`, i.e. the exclusive end line of a
/// span ending at `offset`.
fn line_exclusive(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|start| *start < offset)
}

fn line_offset(starts: &[usize], text: &str, line: usize) -> usize {
    starts.get(line).copied().unwrap_or(text.len())
}

/// The full text of one line, terminator included (empty at end of file).
fn line_text<'a>(starts: &[usize], text: &'a str, line: usize) -> &'a str {
    let Some(&start) = starts.get(line) else {
        return "";
    };
    let end = starts.get(line + 1).copied().unwrap_or(text.len());
    text.get(start..end).unwrap_or_default()
}

/// Comment lines, blank lines and the org-mode/emacs incantations: the kinds of
/// line a directive's CST node swallows but that carry no directive content.
fn is_trivia_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty()
        || trimmed.starts_with(';')
        || trimmed.starts_with('%')
        || trimmed.starts_with("#!")
        || trimmed.starts_with("#+")
}

fn slice(text: &str, start: usize, end: usize) -> Result<&str, String> {
    text.get(start..end).ok_or_else(|| {
        format!("internal: directive boundaries {start}..{end} are not on character boundaries")
    })
}

/// Directive text as the renderer emits it: `\n` line endings, no final newline
/// (the renderer adds `ours`' own line ending back).
fn normalize_block(raw: &str) -> String {
    let mut text = raw.replace("\r\n", "\n");
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

/// Whether the gap before a directive ends in a blank line.
fn ends_with_blank_line(gap: &str) -> bool {
    let Some(rest) = gap.strip_suffix("\r\n").or_else(|| gap.strip_suffix('\n')) else {
        return false;
    };
    rest.trim().is_empty() || rest.ends_with('\n')
}

/// Text folded for comparison only: indentation, runs of spaces and trailing
/// whitespace do not count as a change, because a formatter run on one device
/// must not turn into a conflict. Comments *do* count — a directive whose only
/// difference is a comment is surfaced instead of silently dropped.
fn canonical(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut first = true;
    for line in text.lines() {
        let folded = fold_line(line);
        if folded.is_empty() {
            continue;
        }
        if !first {
            out.push('\n');
        }
        out.push_str(&folded);
        first = false;
    }
    out
}

fn fold_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut pending_space = false;
    for character in line.trim().chars() {
        if in_string {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push(character);
                in_string = true;
            }
            ' ' | '\t' => pending_space = true,
            _ => {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push(character);
            }
        }
    }
    out
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = text.get(..offset).unwrap_or_default();
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before.len() + 1, |index| before.len() - index);
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "\
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

    const OURS_GROCERIES: &str = "2026-01-10 * \"Groceries\"\n  Assets:Bank       -55.00 USD\n  Expenses:Food      55.00 USD\n";
    const BASE_GROCERIES: &str = "2026-01-10 * \"Groceries\"\n  Assets:Bank       -50.00 USD\n  Expenses:Food      50.00 USD\n";
    const BASE_SALARY: &str =
        "2026-01-11 * \"Salary\"\n  Assets:Bank       200.00 USD\n  Income:Salary    -200.00 USD\n";
    const THEIRS_SALARY: &str =
        "2026-01-11 * \"Salary\"\n  Assets:Bank       210.00 USD\n  Income:Salary    -210.00 USD\n";

    const GROCERIES: &str = "2026-01-10 * \"Groceries\"";
    const SALARY: &str = "2026-01-11 * \"Salary\"";
    const COFFEE: &str = "2026-02-01 * \"Coffee\"";

    fn ours() -> String {
        BASE.replacen(BASE_GROCERIES, OURS_GROCERIES, 1)
    }

    fn theirs() -> String {
        BASE.replacen(BASE_SALARY, THEIRS_SALARY, 1)
    }

    fn picks(plan: &MergePlan, choices: &[(Classification, Choice)]) -> HashMap<Key, Choice> {
        let mut picks = HashMap::new();
        for (class, choice) in choices {
            let decision = plan
                .directives
                .iter()
                .find(|decision| decision.class == *class)
                .unwrap_or_else(|| panic!("no decision classified {class:?}"));
            picks.insert(decision.key.clone(), *choice);
        }
        picks
    }

    fn decision<'a>(plan: &'a MergePlan, label: &str) -> &'a DirectiveDecision {
        plan.directives
            .iter()
            .find(|decision| decision.label == label)
            .unwrap_or_else(|| panic!("no decision for {label}: {:?}", plan.directives))
    }

    #[test]
    fn one_side_untouched_renders_byte_identical() {
        let plan = plan(BASE, BASE, BASE, true).expect("plans");
        assert_eq!(summary(&plan).unchanged, plan.directives.len());
        assert_eq!(render(&plan, &HashMap::new()).expect("renders"), BASE);
    }

    #[test]
    fn different_transactions_edited_on_each_side_merge_automatically() {
        let plan = plan(BASE, &ours(), &theirs(), true).expect("plans");
        let summary = summary(&plan);

        assert_eq!(summary.needs_user, 0, "decisions: {:?}", plan.directives);
        assert_eq!(decision(&plan, GROCERIES).class, Classification::OnlyOurs);
        assert_eq!(decision(&plan, SALARY).class, Classification::OnlyTheirs);

        let merged = render(&plan, &HashMap::new()).expect("renders");
        assert_eq!(merged, ours().replacen(BASE_SALARY, THEIRS_SALARY, 1));
        assert!(merged.contains("-55.00 USD"));
        assert!(merged.contains("-210.00 USD"));
    }

    #[test]
    fn same_transaction_changed_differently_asks_and_honours_the_pick() {
        let their_groceries = "2026-01-10 * \"Groceries\"\n  Assets:Bank       -60.00 USD\n  Expenses:Food      60.00 USD\n";
        let theirs = BASE.replacen(BASE_GROCERIES, their_groceries, 1);
        let plan = plan(BASE, &ours(), &theirs, true).expect("plans");

        let groceries = decision(&plan, GROCERIES);
        assert_eq!(groceries.class, Classification::BothChanged);
        assert_eq!(groceries.resolution, Resolution::NeedsUser);

        let error = render(&plan, &HashMap::new()).expect_err("refuses without a pick");
        assert!(error.contains("Groceries"), "error: {error}");

        let merged = render(
            &plan,
            &picks(&plan, &[(Classification::BothChanged, Choice::Ours)]),
        )
        .expect("renders ours");
        assert_eq!(merged, ours());

        let merged = render(
            &plan,
            &picks(&plan, &[(Classification::BothChanged, Choice::Theirs)]),
        )
        .expect("renders theirs");
        assert_eq!(merged, BASE.replacen(BASE_GROCERIES, their_groceries, 1));
    }

    #[test]
    fn identical_addition_on_both_sides_is_emitted_once() {
        let coffee = "2026-02-01 * \"Coffee\"\n  Assets:Bank       -3.00 USD\n  Expenses:Food      3.00 USD\n";
        let with_coffee = format!("{BASE}\n{coffee}");
        let plan = plan(BASE, &with_coffee, &with_coffee, true).expect("plans");

        assert_eq!(decision(&plan, COFFEE).class, Classification::SameChange);
        let merged = render(&plan, &HashMap::new()).expect("renders");
        assert_eq!(merged, with_coffee);
        assert_eq!(merged.matches("Coffee").count(), 1);
    }

    /// Each side edited a *different* field of one transaction, so neither the
    /// identity nor the loose key pairs them. Auto-keeping both additions would
    /// count the entry twice with nothing on screen saying so.
    #[test]
    fn two_edits_of_one_entry_that_cannot_pair_ask_instead_of_duplicating() {
        let base = "\
option \"operating_currency\" \"USD\"

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking  -50.00 USD
  Expenses:Food          50.00 USD
";
        // Ours moves the date; theirs renames the narration.
        let ours = base.replace("2026-01-10", "2026-01-11");
        let theirs = base.replace("\"Groceries\"", "\"Groceries weekly\"");

        let plan = plan(base, &ours, &theirs, true).expect("plans");

        let groceries: Vec<_> = plan
            .directives
            .iter()
            .filter(|directive| directive.kind == "transaction")
            .collect();
        // The base entry, plus each side's unpaired edit.
        assert_eq!(
            groceries.len(),
            3,
            "the unmatched edits are their own decisions: {:?}",
            plan.directives
                .iter()
                .map(|directive| (directive.kind.clone(), directive.class))
                .collect::<Vec<_>>()
        );
        // No addition may be auto-kept: that is the double count.
        for directive in groceries
            .iter()
            .filter(|directive| directive.base.is_none())
        {
            assert!(
                matches!(directive.resolution, Resolution::NeedsUser),
                "{} would have been added without asking",
                directive.label
            );
        }
        // The render refuses an unanswered question instead of guessing.
        assert!(render(&plan, &HashMap::new()).is_err());
    }

    /// A ledger with no logged operation to compare against has no base: every
    /// difference is a whole directive, so taking theirs is a guess.
    #[test]
    fn without_a_logged_base_a_copy_side_addition_asks() {
        let ours = "\
option \"operating_currency\" \"USD\"

2026-01-10 * \"Coffee\"
  Assets:Bank:Checking  -3.00 USD
  Expenses:Food          3.00 USD
";
        let theirs = ours.replace("\"Coffee\"", "\"Cofee\"");

        let plan = plan("", ours, &theirs, false).expect("plans");

        let coffee: Vec<_> = plan
            .directives
            .iter()
            .filter(|directive| {
                directive.label.contains("Cofee") || directive.label.contains("Coffee")
            })
            .collect();
        assert_eq!(coffee.len(), 2, "two whole directives");
        // Taking the folder's version is the guess; keeping our own text is not.
        let copy_side = coffee
            .iter()
            .find(|directive| directive.label.contains("Cofee"))
            .expect("theirs' directive is in the plan");
        assert!(
            matches!(copy_side.resolution, Resolution::NeedsUser),
            "the copy's version was taken without asking"
        );
        let our_side = coffee
            .iter()
            .find(|directive| directive.label.contains("Coffee"))
            .expect("ours' directive is in the plan");
        assert!(
            matches!(our_side.resolution, Resolution::Auto(Choice::Ours)),
            "our own text should be kept without asking"
        );
    }

    #[test]
    fn same_key_added_differently_needs_a_pick_and_both_keeps_both() {
        let their_coffee = "2026-02-01 * \"Coffee\" #theirs\n  Assets:Bank       -3.00 USD\n  Expenses:Food      3.00 USD\n";
        let our_coffee = "2026-02-01 * \"Coffee\" #ours\n  Assets:Bank       -3.00 USD\n  Expenses:Food      3.00 USD\n";
        let ours = format!("{BASE}\n{our_coffee}");
        let theirs = format!("{BASE}\n{their_coffee}");
        let plan = plan(BASE, &ours, &theirs, true).expect("plans");

        let coffee = decision(&plan, COFFEE);
        assert_eq!(coffee.class, Classification::BothAdded);
        assert_eq!(coffee.resolution, Resolution::NeedsUser);
        assert!(render(&plan, &HashMap::new()).is_err());

        let merged = render(
            &plan,
            &picks(&plan, &[(Classification::BothAdded, Choice::Both)]),
        )
        .expect("renders both");
        assert!(merged.contains("#ours"));
        assert!(merged.contains("#theirs"));
        assert_eq!(merged.matches("2026-02-01 * \"Coffee\"").count(), 2);
        assert_eq!(
            merged,
            format!("{BASE}\n{our_coffee}\n{their_coffee}"),
            "ours first, blank line between, then theirs"
        );
        assert!(rustledger_parser::parse(&merged).errors.is_empty());
    }

    #[test]
    fn delete_against_modify_asks_for_that_key_only() {
        let ours = BASE.replacen(&format!("{BASE_GROCERIES}\n"), "", 1);
        let theirs = BASE.replacen(BASE_GROCERIES, OURS_GROCERIES, 1);
        let plan = plan(BASE, &ours, &theirs, true).expect("plans");

        let groceries = decision(&plan, GROCERIES);
        assert_eq!(groceries.class, Classification::DeletedOurs);
        assert_eq!(groceries.resolution, Resolution::NeedsUser);
        let summary = summary(&plan);
        assert_eq!(summary.needs_user, 1, "decisions: {:?}", plan.directives);
        assert_eq!(summary.auto, 0);

        let merged = render(
            &plan,
            &picks(&plan, &[(Classification::DeletedOurs, Choice::Drop)]),
        )
        .expect("renders");
        assert_eq!(merged, ours);
        assert!(!merged.contains("Groceries"));

        let merged = render(
            &plan,
            &picks(&plan, &[(Classification::DeletedOurs, Choice::Theirs)]),
        )
        .expect("renders");
        assert!(merged.contains(OURS_GROCERIES));
        assert!(merged.contains("Salary"));
        assert!(rustledger_parser::parse(&merged).errors.is_empty());
    }

    #[test]
    fn new_open_price_and_include_on_both_sides_survive() {
        let ours = BASE.replacen(
            "2026-01-01 open Assets:Bank\n",
            "include \"prices.bean\"\n2026-01-01 open Assets:Bank\n2026-01-02 open Assets:Savings\n",
            1,
        );
        let theirs = BASE.replacen(
            "2026-01-01 open Income:Salary\n",
            "2026-01-01 open Income:Salary\n2026-01-03 open Assets:Cash\n",
            1,
        );
        let theirs = theirs.replacen(
            BASE_SALARY,
            &format!("{BASE_SALARY}\n2026-01-05 price USD 1.10 EUR\n"),
            1,
        );
        let plan = plan(BASE, &ours, &theirs, true).expect("plans");

        let summary = summary(&plan);
        assert_eq!(summary.needs_user, 0, "decisions: {:?}", plan.directives);
        assert_eq!(summary.auto, 4);
        assert_eq!(summary.kinds.get("include"), Some(&1));
        assert_eq!(summary.kinds.get("price"), Some(&1));
        assert_eq!(summary.kinds.get("open"), Some(&5));

        let merged = render(&plan, &HashMap::new()).expect("renders");
        assert!(merged.contains("include \"prices.bean\""));
        assert!(merged.contains("2026-01-02 open Assets:Savings"));
        assert!(merged.contains("2026-01-03 open Assets:Cash"));
        assert!(merged.contains("2026-01-05 price USD 1.10 EUR"));
        assert!(rustledger_parser::parse(&merged).errors.is_empty());
    }

    #[test]
    fn untouched_text_keeps_odd_formatting_byte_for_byte() {
        let base = "\
; header comment, keep me   
option \"operating_currency\" \"USD\"   \n\
\n\
2026-01-01 open  Assets:Bank     \n\
\n\
; a comment that belongs to the transaction below
2026-01-10 * \"Groceries\"   
  Assets:Bank       -50.00 USD
\tExpenses:Food      50.00 USD

2026-01-11 balance Assets:Bank 100.00 USD  ; trailing comment
";
        let groceries = "2026-01-10 * \"Groceries\"   \n  Assets:Bank       -50.00 USD\n\tExpenses:Food      50.00 USD\n";
        let their_groceries = "2026-01-10 * \"Groceries\"   \n  Assets:Bank       -45.00 USD\n\tExpenses:Food      45.00 USD\n";
        let theirs = base.replacen(groceries, their_groceries, 1);
        let plan = plan(base, base, &theirs, true).expect("plans");

        assert_eq!(decision(&plan, GROCERIES).class, Classification::OnlyTheirs);
        let merged = render(&plan, &HashMap::new()).expect("renders");
        assert_eq!(merged, base.replacen(groceries, their_groceries, 1));

        assert!(merged.starts_with("; header comment, keep me   \n"));
        assert!(merged.contains("option \"operating_currency\" \"USD\"   \n"));
        assert!(merged.contains("2026-01-01 open  Assets:Bank     \n"));
        assert!(merged.contains("; a comment that belongs to the transaction below\n"));
        assert!(merged.contains("\tExpenses:Food"));
        assert!(
            merged.ends_with("2026-01-11 balance Assets:Bank 100.00 USD  ; trailing comment\n")
        );
    }

    #[test]
    fn unchanged_rendering_is_byte_identical_without_a_final_newline() {
        let base = "\
2026-01-01 open Assets:Bank

2026-01-10 * \"Groceries\"
  Assets:Bank       -50.00 USD
  Expenses:Food      50.00 USD";
        let plan = plan(base, base, base, true).expect("plans");
        assert_eq!(render(&plan, &HashMap::new()).expect("renders"), base);
    }

    #[test]
    fn crlf_sides_keep_their_line_endings() {
        let base = "2026-01-01 open Assets:Bank\r\n\r\n2026-01-10 * \"Groceries\"\r\n  Assets:Bank       -50.00 USD\r\n  Expenses:Food      50.00 USD\r\n";
        let ours =
            base.replacen("-50.00 USD", "-55.00 USD", 1)
                .replacen(" 50.00 USD", " 55.00 USD", 1);
        let plan = plan(base, &ours, base, true).expect("plans");

        assert_eq!(decision(&plan, GROCERIES).class, Classification::OnlyOurs);
        let merged = render(&plan, &HashMap::new()).expect("renders");
        assert_eq!(merged, ours);
        assert_eq!(merged.matches('\n').count(), merged.matches("\r\n").count());
    }

    #[test]
    fn a_side_that_does_not_parse_is_named() {
        let error = plan(BASE, BASE, "2026-13-45 * \"broken\"\n", true).expect_err("refuses");
        assert!(error.contains("theirs"), "error: {error}");
        assert!(error.contains("does not parse"), "error: {error}");

        let error = plan("nonsense (", BASE, BASE, true).expect_err("refuses");
        assert!(error.contains("base"), "error: {error}");
    }

    #[test]
    fn duplicate_identical_directives_still_merge_when_untouched() {
        let coffee = "2026-02-01 * \"Coffee\"\n  Assets:Bank       -3.00 USD\n  Expenses:Food      3.00 USD\n";
        let base = format!("{BASE}\n{coffee}\n{coffee}");
        let plan = plan(&base, &base, &base, true).expect("plans");

        assert_eq!(summary(&plan).needs_user, 0);
        assert!(plan.directives.iter().any(|decision| decision.ambiguous));
        assert_eq!(render(&plan, &HashMap::new()).expect("renders"), base);
    }

    #[test]
    fn a_pick_overrides_an_auto_decision_and_stale_picks_are_ignored() {
        let plan = plan(BASE, &ours(), &theirs(), true).expect("plans");

        // A UI may send a selection for a key this plan does not have any more.
        let mut picks = HashMap::new();
        picks.insert(Key("something-else#0".to_string()), Choice::Both);
        assert_eq!(
            render(&plan, &picks).expect("renders"),
            ours().replacen(BASE_SALARY, THEIRS_SALARY, 1)
        );

        // An explicit pick beats the automatic choice.
        picks.insert(decision(&plan, SALARY).key.clone(), Choice::Ours);
        assert_eq!(render(&plan, &picks).expect("renders"), ours());
    }

    #[test]
    fn deletion_on_both_sides_is_reported_and_dropped() {
        let coffee = "2026-02-01 * \"Coffee\"\n  Assets:Bank       -3.00 USD\n  Expenses:Food      3.00 USD\n";
        let base = format!("{BASE}\n{coffee}");
        let plan = plan(&base, BASE, BASE, true).expect("plans");

        let coffee = decision(&plan, COFFEE);
        assert_eq!(coffee.class, Classification::BothDeleted);
        assert_eq!(coffee.resolution, Resolution::Auto(Choice::Drop));
        assert!(coffee.ours.is_none());
        assert!(coffee.theirs.is_none());

        assert_eq!(render(&plan, &HashMap::new()).expect("renders"), BASE);
        assert_eq!(summary(&plan).auto, 1);
    }

    #[test]
    fn summary_counts_and_kinds_describe_the_plan() {
        let plan = plan(BASE, &ours(), &theirs(), true).expect("plans");
        let summary = summary(&plan);
        assert_eq!(summary.total, plan.directives.len());
        assert_eq!(
            summary.unchanged + summary.auto + summary.needs_user,
            summary.total
        );
        assert_eq!(summary.auto, 2);
        assert_eq!(summary.kinds.get("transaction"), Some(&2));
        assert_eq!(summary.kinds.get("open"), Some(&3));
        assert_eq!(summary.kinds.len(), 2);
    }
}
