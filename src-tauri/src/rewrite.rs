// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Surgical source rewriting: an edit replaces the bytes it addresses and
//! nothing else, so the file keeps its own formatting, annotations and comments.

use std::str::FromStr;

use rustledger_core::{Decimal, IncompleteAmount, Span, Transaction};

use crate::model::{PostingEdit, TransactionEdit};

/// One replacement of a byte range inside a file: `text` goes where `start..end`
/// was.
struct Replacement {
    start: usize,
    end: usize,
    text: String,
}

/// The file text with `edit` applied to the transaction at `span`: its header
/// line rewritten from the date through the narration, and the units figure on
/// each edited posting's line. Every other byte of the file is copied straight
/// through — including the tags, links, comments, costs and prices on the very
/// lines being rewritten.
pub(crate) fn rewrite_transaction(
    text: &str,
    span: Span,
    tx: &Transaction,
    edit: &TransactionEdit,
) -> Result<String, String> {
    let mut replacements = header_replacement(text, span, edit)?;
    for posting in &edit.postings {
        replacements.push(posting_replacement(text, tx, posting)?);
    }
    replacements.sort_by_key(|replacement| replacement.start);
    // Two edits answering to the same bytes would each be written against
    // offsets the other has already moved.
    for pair in replacements.windows(2) {
        if pair[1].start < pair[0].end {
            return Err("edit rewrites the same text twice".to_string());
        }
    }
    let mut rewritten = String::with_capacity(text.len());
    let mut cursor = 0;
    for replacement in &replacements {
        rewritten.push_str(&text[cursor..replacement.start]);
        rewritten.push_str(&replacement.text);
        cursor = replacement.end;
    }
    rewritten.push_str(&text[cursor..]);
    Ok(rewritten)
}

/// The header rewrite: `DATE FLAG "payee" "narration"`, with whatever follows on
/// the line — tags, links, a comment — left exactly as it was.
///
/// The edit is surgical: the date, the flag and the description's literals are
/// located as tokens in the line, and only the ones whose value the edit
/// changes have their bytes replaced. Every other byte keeps the file's own
/// spelling — the line's indent, the widths of the separators, a literal's
/// quoting and escapes. The single-literal `"payee|narration"` and two-literal
/// `"payee" "narration"` forms are converted into one another only when the
/// edit forces it (a distinct payee where the source spelled neither a payee
/// nor a `|`), never because a value was merely re-typed. A payee the narration
/// already covers, or none at all, collapses to the narration alone.
fn header_replacement(
    text: &str,
    span: Span,
    edit: &TransactionEdit,
) -> Result<Vec<Replacement>, String> {
    let start = text[..span.start.min(text.len())]
        .rfind('\n')
        .map_or(0, |at| at + 1);
    let end = text[start..].find('\n').map_or(text.len(), |at| start + at);
    let line = &text[start..end];
    let header = header_tokens(line)
        .ok_or_else(|| "the transaction's header line has no payee or narration".to_string())?;
    let date = checked_date(&edit.date)?;
    let flag = checked_flag(&edit.flag)?;
    let payee = edit.payee.as_deref().filter(|payee| !payee.is_empty());
    let distinct_payee = payee.filter(|payee| *payee != edit.narration);
    let narration = edit.narration.as_str();
    let replace = |range: (usize, usize), text: String| Replacement {
        start: start + range.0,
        end: start + range.1,
        text,
    };

    let mut replacements = Vec::new();

    // The date and the flag are single tokens: replace each only when the bytes
    // it spells are no longer the bytes the edit asks for.
    if &line[header.date.0..header.date.1] != date.as_str() {
        replacements.push(replace(header.date, date));
    }
    match header.flag {
        Some(flag_range) if &line[flag_range.0..flag_range.1] != flag => {
            replacements.push(replace(flag_range, flag.to_string()));
        }
        Some(_) => {}
        // No flag token to replace: put one after the date, leaving the
        // whitespace that already separated the date from the description.
        None => replacements.push(replace((header.date.1, header.date.1), format!(" {flag}"))),
    }

    // The description is either two literals (`"payee" "narration"`) or one
    // (`"payee|narration"`, or just the narration). Touch each literal whose
    // value changed; convert between the forms only when the edit's payee
    // forces a form the source did not spell.
    match header.strings.len() {
        2 => {
            let payee_range = header.strings[0];
            let narration_range = header.strings[1];
            let payee_literal = &line[payee_range.0..payee_range.1];
            let narration_literal = &line[narration_range.0..narration_range.1];
            match distinct_payee {
                Some(payee) => {
                    if unquote_beancount(payee_literal) != payee {
                        replacements.push(replace(payee_range, quote_beancount(payee)));
                    }
                    if unquote_beancount(narration_literal) != narration {
                        replacements.push(replace(narration_range, quote_beancount(narration)));
                    }
                }
                // The edit drops the payee, so the two-literal form has to give
                // way to one: withdraw the payee literal and the whitespace that
                // joined it to the narration, leaving an untouched narration its
                // own bytes.
                None => {
                    replacements.push(replace((payee_range.0, narration_range.0), String::new()));
                    if unquote_beancount(narration_literal) != narration {
                        replacements.push(replace(narration_range, quote_beancount(narration)));
                    }
                }
            }
        }
        1 => {
            let range = header.strings[0];
            let value = unquote_beancount(&line[range.0..range.1]);
            match distinct_payee {
                // A single literal that already spelled `payee|narration` keeps
                // that form; one spelled without a `|` has to grow into two.
                Some(payee) if value.contains('|') => {
                    let rewritten = format!("{payee}|{narration}");
                    if rewritten != value {
                        replacements.push(replace(range, quote_beancount(&rewritten)));
                    }
                }
                Some(payee) => replacements.push(replace(
                    range,
                    format!("{} {}", quote_beancount(payee), quote_beancount(narration)),
                )),
                None => {
                    if value != narration {
                        replacements.push(replace(range, quote_beancount(narration)));
                    }
                }
            }
        }
        // A header cannot describe three literals; whatever was loaded, rewrite
        // the whole description as the tokens the edit asks for.
        _ => {
            let last = *header
                .strings
                .last()
                .expect("header_tokens keeps at least one");
            let description = match distinct_payee {
                Some(payee) => {
                    format!("{} {}", quote_beancount(payee), quote_beancount(narration))
                }
                None => quote_beancount(narration),
            };
            replacements.push(replace((header.strings[0].0, last.1), description));
        }
    }

    Ok(replacements)
}

/// The units rewrite for one posting: the new figure spliced in where the amount
/// already is, or inserted after the account when the line never spelled the
/// amount out (the interpolated leg of a transaction).
fn posting_replacement(
    text: &str,
    tx: &Transaction,
    edit: &PostingEdit,
) -> Result<Replacement, String> {
    let posting = tx
        .postings
        .get(edit.index as usize)
        .ok_or_else(|| format!("transaction has no posting {}", edit.index))?;
    let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
        return Err(format!("posting {} has no amount to edit", edit.index));
    };
    let number = Decimal::from_str(edit.units.trim())
        .map_err(|e| format!("`{}` is not an amount: {e}", edit.units))?;
    let line = text
        .get(posting.span.start..posting.span.end)
        .ok_or_else(|| format!("posting {} is outside its file", edit.index))?;
    let (start, end, replacement) = units_replacement(
        line,
        posting.value.account.as_str(),
        amount.currency.as_str(),
        &number.to_string(),
    )?;
    Ok(Replacement {
        start: posting.span.start + start,
        end: posting.span.start + end,
        text: replacement,
    })
}

/// Where a posting line's units figure sits, and what to put in its place,
/// leaving the account, the currency, the `@`/`@@` price, the `{cost}` spec and
/// any trailing comment exactly as written.
///
/// The figure is everything between the account and the currency; beancount
/// allows an arithmetic expression there, so the whole run is replaced rather
/// than a single token. A posting whose line carries no currency had its amount
/// interpolated at load time and has to have it spelled out.
fn units_replacement(
    line: &str,
    account: &str,
    currency: &str,
    number: &str,
) -> Result<(usize, usize, String), String> {
    let account_at =
        line.find(account)
            .filter(|at| {
                line[..*at].chars().next_back().is_none_or(|before| {
                    !before.is_alphanumeric() && before != ':' && before != '_'
                })
            })
            .ok_or_else(|| format!("`{account}` is not on its posting line"))?;
    let after_account = account_at + account.len();
    // The units stop where the rest of the posting's annotation starts: a
    // comment, a price, or a cost spec.
    let units_end = [';', '@', '{']
        .into_iter()
        .filter_map(|marker| line[after_account..].find(marker))
        .min()
        .unwrap_or(line.len() - after_account);
    let units = &line[after_account..after_account + units_end];
    match units.find(currency) {
        Some(at) => {
            let figure = units[..at].trim_end();
            if figure.is_empty() {
                // The line names the currency but no figure: put the number in
                // front of the currency.
                let insert = after_account + at;
                Ok((insert, insert, format!("{number} ")))
            } else {
                let start = after_account + (units[..at].len() - units[..at].trim_start().len());
                Ok((start, after_account + figure.len(), number.to_string()))
            }
        }
        None => Ok((
            after_account,
            after_account,
            format!(" {number} {currency}"),
        )),
    }
}

/// Quote a string for a transaction header, escaping exactly what the lexer
/// reads as an escape so the value round-trips.
fn quote_beancount(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// A transaction header's editable tokens, as byte ranges within its line: the
/// date, the flag (when the line spells one) and the description literals.
struct HeaderTokens {
    /// The date token, past the line's indent.
    date: (usize, usize),
    /// The flag token, if the line has one.
    flag: Option<(usize, usize)>,
    /// The description: a single `"payee|narration"` (or narration) literal, or
    /// a payee literal and a narration literal.
    strings: Vec<(usize, usize)>,
}

/// Locate a header line's tokens so an edit can replace one and leave the rest
/// of the line's bytes alone.
///
/// `None` when the line carries no description literal — a header an edit
/// cannot address. The line's indent, the whitespace between tokens and
/// anything after the description (tags, links, a comment) are deliberately
/// absent from every range.
fn header_tokens(line: &str) -> Option<HeaderTokens> {
    let date_start = line.len() - line.trim_start().len();
    let date_end = date_start
        + line[date_start..]
            .find(char::is_whitespace)
            .unwrap_or(line.len() - date_start);
    let first_quote = date_end + line[date_end..].find('"')?;
    let between = &line[date_end..first_quote];
    let flag = between
        .find(|character: char| !character.is_whitespace())
        .map(|offset| {
            let from = date_end + offset;
            (from, from + between[offset..].trim_end().len())
        });
    // The header's quoted strings are its first tokens, and a header has at
    // most two; a quote further along the line — inside a comment, say — is not
    // part of the run, because something other than whitespace separates it.
    let mut strings = Vec::new();
    let mut at = first_quote;
    loop {
        let string_end = end_of_string(line, at)?;
        strings.push((at, string_end));
        let rest = &line[string_end..];
        match rest.find('"') {
            Some(next) if rest[..next].trim().is_empty() => at = string_end + next,
            _ => break,
        }
    }
    Some(HeaderTokens {
        date: (date_start, date_end),
        flag,
        strings,
    })
}

/// The value a header string literal spells, with beancount's escapes undone —
/// the inverse of `quote_beancount`, so an edit's value can be compared with a
/// literal without rewriting the literal when the two agree.
fn unquote_beancount(literal: &str) -> String {
    let inner = literal
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(literal);
    let mut value = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            value.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => value.push('\n'),
            Some('r') => value.push('\r'),
            Some('t') => value.push('\t'),
            Some(escaped) => value.push(escaped),
            None => value.push('\\'),
        }
    }
    value
}

/// The byte offset just past the quoted string that starts at `start`.
fn end_of_string(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

/// Validate a `YYYY-MM-DD` date and hand back its canonical spelling.
fn checked_date(date: &str) -> Result<String, String> {
    let invalid = || format!("`{date}` is not a YYYY-MM-DD date");
    let mut parts = date.split('-');
    let year: i32 = parts
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let month: u32 = parts
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let day: u32 = parts
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    if parts.next().is_some() {
        return Err(invalid());
    }
    rustledger_core::naive_date(year, month, day)
        .map(|date| date.to_string())
        .ok_or_else(invalid)
}

/// Validate a transaction flag: a single character, and not one that would end
/// the header token it is written in.
fn checked_flag(flag: &str) -> Result<&str, String> {
    let mut characters = flag.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None)
            if !character.is_whitespace() && character != '"' && character != ';' =>
        {
            Ok(flag)
        }
        _ => Err(format!("`{flag}` is not a transaction flag")),
    }
}
