// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a `balance_view` entry came from: its source directive, its file, and
//! the byte-offset lookups the editors and the change tracking need.

use std::collections::HashMap;

use rustledger_core::{Directive, Spanned, Transaction};
use rustledger_loader::{Ledger, SourceFile};

/// The index into `ledger.directives` of the source directive behind each entry
/// of `ledger.balance_view()`.
///
/// `balance_view` hands out bare `Directive`s, so a transaction's source
/// location has to be recovered from the directive stream. The two are the same
/// stream — both sorted by date — with synthesized pad transactions inserted, so
/// walking them in lockstep and advancing the source stream only on a match
/// pairs every entry of the view with the directive it came from.
pub(crate) fn source_directive_index(ledger: &Ledger, view: &[Directive]) -> Vec<Option<usize>> {
    let mut sources = Vec::with_capacity(view.len());
    let mut next = 0;
    for directive in view {
        let matched = ledger
            .directives
            .get(next)
            .filter(|source| source.value == *directive)
            .map(|_| next);
        if matched.is_some() {
            next += 1;
        }
        sources.push(matched);
    }
    sources
}

/// The transaction behind a `TransactionView.id` — the entry at `id` in the
/// ledger's transaction stream — together with its source span and the file it
/// was loaded from. Every edit path starts by locating its transaction this
/// way, so an unknown id fails the same way whichever editor asked.
pub(crate) fn source_of_transaction<'a>(
    ledger: &'a Ledger,
    id: u32,
) -> Result<(&'a Spanned<Directive>, &'a Transaction, &'a SourceFile), String> {
    let view = ledger.balance_view();
    let sources = source_directive_index(ledger, &view);
    let source = sources
        .get(id as usize)
        .copied()
        .flatten()
        .map(|index| &ledger.directives[index])
        .ok_or_else(|| format!("no transaction with id {id}"))?;
    let Directive::Transaction(tx) = &source.value else {
        return Err(format!("directive {id} is not a transaction"));
    };
    let file = ledger
        .source_map
        .get(source.file_id as usize)
        .ok_or_else(|| format!("no source file for transaction {id}"))?;
    Ok((source, tx, file))
}

/// The 1-based line and UTF-16 column of `byte` in `text` — Monaco's own
/// coordinates. A byte offset that is not a character boundary (or past the
/// end) answers for the largest boundary before it, so a stale offset can never
/// panic.
pub(crate) fn line_column(text: &str, byte: usize) -> (u32, u32) {
    let head = text.get(..byte.min(text.len())).unwrap_or(text);
    let line = head.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let line_start = head.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = text[line_start..head.len()]
        .chars()
        .map(char::len_utf16)
        .sum::<usize>() as u32
        + 1;
    (line, column)
}

/// Whether a directive's own source lines are among the working tree's changes
/// for its file. A span's trailing trivia — the blank line or comment after the
/// last posting — belongs to no transaction and is ignored.
pub(crate) fn touches_changes(
    ledger: &Ledger,
    changed: &HashMap<u16, Vec<(u32, u32)>>,
    line_starts: &mut HashMap<u16, Vec<usize>>,
    source: &Spanned<Directive>,
) -> bool {
    let Some(ranges) = changed.get(&source.file_id) else {
        return false;
    };
    let Some(file) = ledger.source_map.get(source.file_id as usize) else {
        return false;
    };
    let text: &str = &file.source;
    let starts = line_starts
        .entry(source.file_id)
        .or_insert_with(|| line_starts_of(text));
    let body = text.get(source.span.start..source.span.end).unwrap_or("");
    // The parser ends a directive's span at the *next* directive's first token,
    // so trailing blank lines and `;` comments — the next entry's leading ones,
    // or the file's own tail — sit inside this span. Ending at the last line
    // that is neither keeps a comment-only edit from marking the entry before it
    // modified, which is what the register and the sidebar draw.
    let mut own = 0;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let rest = line.trim_start();
        if !rest.trim().is_empty() && !rest.starts_with(';') {
            own = offset + line.len();
        }
        offset += line.len();
    }
    let last_byte = source.span.start + own;
    let first = line_of(starts, source.span.start);
    let last = line_of(starts, last_byte.saturating_sub(1).max(source.span.start));
    ranges.iter().any(|&(from, to)| from <= last && first <= to)
}

/// The byte offset at which each line of `text` starts, for offset → line lookup.
fn line_starts_of(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    starts
}

/// The 1-based line `offset` falls on, given the file's line starts.
fn line_of(starts: &[usize], offset: usize) -> u32 {
    u32::try_from(starts.partition_point(|&start| start <= offset)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::session::load_baselined;
    use crate::testing::TempDir;

    use super::{line_of, touches_changes};

    /// A comment between two entries belongs to neither: the parser's span ends
    /// at the next directive's first token, so the line after an entry is inside
    /// that entry's span unless the trailing trivia is cut off.
    #[test]
    fn a_trailing_comment_does_not_mark_the_entry_before_it_modified() {
        let dir = TempDir::new("source-comment");
        let path = dir.file("main.bean");
        std::fs::write(
            &path,
            "option \"operating_currency\" \"USD\"\n\n\
             2026-01-10 * \"Groceries\"\n  Assets:Bank:Checking  -20.00 USD\n  Expenses:Food         20.00 USD\n\n\
             2026-01-11 * \"Tea\"\n  Assets:Bank:Checking  -2.00 USD\n  Expenses:Food          2.00 USD\n",
        )
        .unwrap();
        let loaded = load_baselined(&path).unwrap();

        // The change is the comment line plus the second entry's posting.
        let text = loaded.ledger.source_map.files()[0].source.clone();
        let comment = text.find("2026-01-11").unwrap();
        let last = text.rfind("Expenses:Food").unwrap();
        let from = comment + "2026-01-11 * \"Tea\"".len();
        let mut changed: HashMap<u16, Vec<(u32, u32)>> = HashMap::new();
        changed.insert(
            0,
            vec![
                ({ line_of(&line_starts(&text), from) }, {
                    line_of(&line_starts(&text), from)
                }),
                (
                    line_of(&line_starts(&text), last),
                    line_of(&line_starts(&text), last),
                ),
            ],
        );

        let mut starts = HashMap::new();
        let touched: Vec<&str> = loaded
            .ledger
            .directives
            .iter()
            .filter(|directive| touches_changes(&loaded.ledger, &changed, &mut starts, directive))
            .filter_map(|directive| match &directive.value {
                rustledger_core::Directive::Transaction(txn) => Some(txn.narration.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(touched, ["Tea"], "{touched:?}");
    }

    fn line_starts(text: &str) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        starts
    }
}
