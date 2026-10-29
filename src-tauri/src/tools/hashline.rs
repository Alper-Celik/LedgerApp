// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hashline editing: `read` hands out a short hash of each line's own content,
//! and every edit names the lines it touches by those hashes instead of by
//! position.
//!
//! A hash is derived from one line's text alone, so it survives edits
//! elsewhere in the file: the anchor a caller holds for line 12 keeps working
//! while lines around it are inserted or removed. Anchors may also be written
//! `hash@line` (the line number `read` reported); the line number is then
//! checked as well, which is how a caller disambiguates two identical lines —
//! two blank lines, say — that hash the same.
//!
//! An edit is all-or-nothing: every anchor is resolved against the file as it
//! stands before anything is written, and one anchor that no longer matches
//! refuses the whole batch with the line that changed. What comes back is the
//! freshly hashed region the edit touched, so a caller can chain another edit
//! without re-reading the file.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The first line `read` returns when it is not told otherwise (lines are
/// 1-based, as every editor counts them).
pub const DEFAULT_OFFSET: usize = 1;
/// How many lines `read` returns when it is not told otherwise.
pub const DEFAULT_READ_LIMIT: usize = 2_000;
/// The most lines `read` returns, and the most an edit echoes back.
pub const MAX_READ_LIMIT: usize = 2_000;

/// One line as the tools see it: its number, the hash an edit anchors to, and
/// the text itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LineView {
    /// 1-based line number.
    pub n: usize,
    /// Four hex characters of the line's content.
    pub hash: String,
    pub text: String,
}

impl LineView {
    pub fn new(n: usize, text: &str) -> Self {
        Self {
            n,
            hash: hash_line(text),
            text: text.to_string(),
        }
    }
}

/// What [`read_file`] returns: the requested slice of the file, plus enough
/// context to ask for the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadOutcome {
    pub total_lines: usize,
    /// The number of the first line in `lines`.
    pub first: usize,
    pub lines: Vec<LineView>,
    /// More lines follow the ones returned.
    pub truncated: bool,
}

/// What [`apply_edits`] returns: the touched region, freshly hashed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditOutcome {
    pub total_lines: usize,
    /// The number of the first line in `lines`.
    pub first: usize,
    pub lines: Vec<LineView>,
    /// The touched region is longer than [`MAX_READ_LIMIT`] and was clipped.
    pub truncated: bool,
    /// The file on disk was rewritten (false when the edit was a no-op).
    pub written: bool,
}

/// One operation of an edit batch.
///
/// The variants are internally tagged by `op` and carry the shape of the JSON
/// the model sends:
///
/// ```json
/// {"op": "replace", "at": "9f2a", "lines": ["new text"]}
/// {"op": "insert_after", "after": "9f2a@12", "lines": ["new text"]}
/// {"op": "delete", "from": "9f2a", "to": "1c7d"}
/// {"op": "write", "content": "the whole file\n"}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditOp {
    /// Replace the whole file, creating it if it does not exist. It must be
    /// the only operation of its batch.
    Write { content: String },
    /// Replace the anchored line with `lines` (empty `lines` deletes it).
    Replace { at: String, lines: Vec<String> },
    /// Insert `lines` after the anchored line.
    InsertAfter { after: String, lines: Vec<String> },
    /// Remove the lines from `from` through `to`, inclusive.
    Delete { from: String, to: String },
}

/// The four hex characters a line's content hashes to.
///
/// FNV-1a over the line's bytes, keeping the high half: deterministic across
/// runs, machines and Rust versions, so an anchor minted by one `read` still
/// names the same line after an unrelated edit. Four hex characters are not a
/// checksum — the `hash@line` form and the ambiguity check below are what make
/// a collision harmless. This rule is an edit anchor and nothing else: no
/// value, amount or file identity is hashed this way, and a caller must not
/// read the result as one.
pub fn hash_line(text: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{:04x}", hash >> 16)
}

/// The lines of `path`, from `offset`, at most `limit` of them.
pub fn read_file(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ReadOutcome, String> {
    let offset = offset.unwrap_or(DEFAULT_OFFSET);
    if offset == 0 {
        return Err("offset is 1-based: the first line is 1".to_string());
    }
    let limit = limit.unwrap_or(DEFAULT_READ_LIMIT);
    if limit == 0 {
        return Err("limit must be at least 1".to_string());
    }
    let limit = limit.min(MAX_READ_LIMIT);

    let bytes = fs::read(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => format!("no such file: {}", path.display()),
        io::ErrorKind::IsADirectory => format!("{} is a directory", path.display()),
        _ => format!("cannot read {}: {error}", path.display()),
    })?;
    let text = decode(path, &bytes)?;

    let document = Document::parse(&text);
    let total = document.lines.len();
    let start = (offset - 1).min(total);
    let end = start.saturating_add(limit).min(total);
    Ok(ReadOutcome {
        total_lines: total,
        first: start + 1,
        lines: (start..end)
            .map(|index| LineView::new(index + 1, &document.lines[index]))
            .collect(),
        truncated: end < total,
    })
}

/// Apply `ops` to `path`, all or nothing.
///
/// Every anchor is resolved against the file as it is on disk first; a single
/// stale anchor returns [`Err`] and leaves the file byte-identical. Operations
/// are read in the coordinates of that same text and must be written in
/// ascending, non-overlapping order, which is how a `replace` and an
/// `insert_after` of the same line compose predictably.
pub fn apply_edits(path: &Path, ops: &[EditOp]) -> Result<EditOutcome, String> {
    if ops.is_empty() {
        return Err("an edit needs at least one operation".to_string());
    }

    let existing = match fs::read(path) {
        Ok(bytes) => Some(decode(path, &bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };

    // `write` is the whole file by definition: nothing else can be in the
    // batch, because there would be no text for the other anchors to name.
    if ops.iter().any(|op| matches!(op, EditOp::Write { .. })) {
        let [EditOp::Write { content }] = ops else {
            return Err(
                "`write` replaces the whole file and cannot be combined with other operations"
                    .to_string(),
            );
        };
        let written = write_whole(path, content)?;
        let document = Document::parse(content);
        let (first, lines, truncated) = region(&document, 0, document.lines.len());
        return Ok(EditOutcome {
            total_lines: document.lines.len(),
            first,
            lines,
            truncated,
            written,
        });
    }

    let Some(previous) = existing else {
        return Err(format!(
            "no such file: {} (a `write` operation creates it)",
            path.display()
        ));
    };

    let document = Document::parse(&previous);
    let planned = plan(&document, ops)?;

    let mut edited = document.clone();
    let mut shift: isize = 0;
    let mut first_new: Option<usize> = None;
    let mut last_end: Option<usize> = None;
    for planned_op in &planned {
        let start = (planned_op.start as isize + shift) as usize;
        let end = (planned_op.end as isize + shift) as usize;
        edited
            .lines
            .splice(start..end, planned_op.lines.iter().cloned());
        shift += planned_op.lines.len() as isize - (planned_op.end - planned_op.start) as isize;
        first_new.get_or_insert(start);
        last_end = Some(start + planned_op.lines.len());
    }

    let rendered = edited.render();
    let written = rendered != previous;
    if written {
        crate::fsutil::write_atomic(path, rendered.as_bytes(), None)?;
    }

    let total = edited.lines.len();
    let (start, end) = touched(total, first_new.unwrap_or(0), last_end.unwrap_or(0));
    let (first, lines, truncated) = region(&edited, start, end);
    Ok(EditOutcome {
        total_lines: total,
        first,
        lines,
        truncated,
        written,
    })
}

/// Where the touched region sits once every operation has landed: from the
/// first touched line through the last, widened by one line when the batch
/// only deleted (an empty region could not be anchored to).
fn touched(total: usize, first: usize, last_end: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let start = first.min(total - 1);
    let end = last_end.max(start + 1).min(total);
    (start, end)
}

/// One operation resolved into the coordinates of the file being edited.
struct Planned {
    /// 0-based, inclusive.
    start: usize,
    /// 0-based, exclusive.
    end: usize,
    lines: Vec<String>,
}

fn plan(document: &Document, ops: &[EditOp]) -> Result<Vec<Planned>, String> {
    let mut planned: Vec<Planned> = Vec::with_capacity(ops.len());
    let mut previous_end = 0;
    for (index, op) in ops.iter().enumerate() {
        let resolved = match op {
            EditOp::Write { .. } => {
                return Err(
                    "`write` replaces the whole file and cannot be combined with other operations"
                        .to_string(),
                );
            }
            EditOp::Replace { at, lines } => {
                let line = resolve(document, at).map_err(|error| operation(index, &error))?;
                Planned {
                    start: line,
                    end: line + 1,
                    lines: lines.clone(),
                }
            }
            EditOp::InsertAfter { after, lines } => {
                let line = resolve(document, after).map_err(|error| operation(index, &error))?;
                Planned {
                    start: line + 1,
                    end: line + 1,
                    lines: lines.clone(),
                }
            }
            EditOp::Delete { from, to } => {
                let first = resolve(document, from).map_err(|error| operation(index, &error))?;
                let last = resolve(document, to).map_err(|error| operation(index, &error))?;
                if last < first {
                    return Err(operation(
                        index,
                        &format!(
                            "delete runs from line {} up to the earlier line {}; give `from` first",
                            first + 1,
                            last + 1
                        ),
                    ));
                }
                Planned {
                    start: first,
                    end: last + 1,
                    lines: Vec::new(),
                }
            }
        };
        if resolved.start < previous_end {
            return Err(operation(
                index,
                &format!(
                    "operations overlap or are out of order at line {}; give them in ascending line order",
                    resolved.start + 1
                ),
            ));
        }
        previous_end = resolved.end;
        planned.push(resolved);
    }
    Ok(planned)
}

fn operation(index: usize, message: &str) -> String {
    format!("operation {}: {message}", index + 1)
}

/// An anchor as the caller wrote it: `hash`, or `hash@line`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Anchor {
    hash: String,
    line: Option<usize>,
}

fn parse_anchor(raw: &str) -> Result<Anchor, String> {
    let trimmed = raw.trim();
    let (hash, line) = match trimmed.split_once('@') {
        Some((hash, number)) => {
            let number = number
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("anchor `{raw}`: `{number}` is not a line number"))?;
            if number == 0 {
                return Err(format!("anchor `{raw}`: line numbers start at 1"));
            }
            (hash, Some(number))
        }
        None => (trimmed, None),
    };
    if hash.len() != 4 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "anchor `{raw}`: a hash is four hex characters, as `read` returned it"
        ));
    }
    Ok(Anchor {
        hash: hash.to_ascii_lowercase(),
        line,
    })
}

/// The 0-based line `anchor` names, or the reason it no longer names one.
fn resolve(document: &Document, raw: &str) -> Result<usize, String> {
    let anchor = parse_anchor(raw)?;
    if let Some(number) = anchor.line {
        let Some(text) = document.lines.get(number - 1) else {
            return Err(format!(
                "line changed at {number}: the file has {} lines",
                document.lines.len()
            ));
        };
        let actual = hash_line(text);
        if actual != anchor.hash {
            return Err(format!(
                "line changed at {number}: expected {}, found {actual}",
                anchor.hash
            ));
        }
        return Ok(number - 1);
    }

    let mut found = document
        .lines
        .iter()
        .enumerate()
        .filter(|(_, text)| hash_line(text) == anchor.hash)
        .map(|(index, _)| index);
    let Some(first) = found.next() else {
        return Err(format!(
            "line changed: no line has hash {} (re-read the file)",
            anchor.hash
        ));
    };
    let rest: Vec<String> = found.map(|index| (index + 1).to_string()).collect();
    if !rest.is_empty() {
        let mut lines = vec![(first + 1).to_string()];
        lines.extend(rest);
        return Err(format!(
            "ambiguous anchor {}: lines {} share it; write {}@{}",
            anchor.hash,
            lines.join(", "),
            anchor.hash,
            first + 1
        ));
    }
    Ok(first)
}

/// The file as lines, with what it takes to render them back byte for byte.
///
/// A file whose lines end in `\r\n` is edited and rendered that way. A file
/// with mixed endings keeps its text but is written back with the ending it
/// mostly used: CRLF if any line had one.
#[derive(Debug, Clone)]
struct Document {
    lines: Vec<String>,
    trailing_newline: bool,
    crlf: bool,
}

impl Document {
    fn parse(text: &str) -> Self {
        if text.is_empty() {
            return Self {
                lines: Vec::new(),
                trailing_newline: false,
                crlf: false,
            };
        }
        let crlf = text.contains("\r\n");
        let trailing_newline = text.ends_with('\n');
        let body = if trailing_newline {
            &text[..text.len() - 1]
        } else {
            text
        };
        let lines = body
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();
        Self {
            lines,
            trailing_newline,
            crlf,
        }
    }

    fn render(&self) -> String {
        let separator = if self.crlf { "\r\n" } else { "\n" };
        let mut out = self.lines.join(separator);
        if self.trailing_newline && !self.lines.is_empty() {
            out.push_str(separator);
        }
        out
    }
}

/// The 1-based number of the first line and the views of `start..end`.
fn region(document: &Document, start: usize, end: usize) -> (usize, Vec<LineView>, bool) {
    let last = end.min(document.lines.len());
    let first = start.min(last);
    let mut lines: Vec<LineView> = (first..last)
        .map(|index| LineView::new(index + 1, &document.lines[index]))
        .collect();
    let truncated = lines.len() > MAX_READ_LIMIT;
    lines.truncate(MAX_READ_LIMIT);
    (first + 1, lines, truncated)
}

fn decode(path: &Path, bytes: &[u8]) -> Result<String, String> {
    std::str::from_utf8(bytes).map(str::to_string).map_err(|_| {
        if bytes.contains(&0) {
            format!("{} is not a text file", path.display())
        } else {
            format!("{} is not UTF-8 text", path.display())
        }
    })
}

/// A whole-file write, reporting whether the bytes on disk actually changed:
/// a file already holding the same text keeps its mtime.
fn write_whole(path: &Path, text: &str) -> Result<bool, String> {
    if fs::read(path).is_ok_and(|existing| existing == text.as_bytes()) {
        return Ok(false);
    }
    crate::fsutil::write_atomic(path, text.as_bytes(), None)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Document, EditOp, LineView, apply_edits, hash_line, read_file};

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn seed(dir: &tempfile::TempDir, name: &str, text: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, text).unwrap();
        path
    }

    fn anchor(line: &LineView) -> String {
        format!("{}@{}", line.hash, line.n)
    }

    fn replace(text: &str, lines: &[&str]) -> EditOp {
        EditOp::Replace {
            at: text.to_string(),
            lines: lines.iter().map(|line| line.to_string()).collect(),
        }
    }

    fn insert_after(text: &str, lines: &[&str]) -> EditOp {
        EditOp::InsertAfter {
            after: text.to_string(),
            lines: lines.iter().map(|line| line.to_string()).collect(),
        }
    }

    fn delete(from: &str, to: &str) -> EditOp {
        EditOp::Delete {
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    #[test]
    fn hashes_are_content_only() {
        assert_eq!(
            hash_line("2026-01-01 * \"Groceries\""),
            hash_line("2026-01-01 * \"Groceries\"")
        );
        assert_ne!(hash_line("Assets:Bank"), hash_line("Assets:Cash"));
        assert_eq!(hash_line("").len(), 4);
    }

    #[test]
    fn document_round_trips_line_endings() {
        for text in ["", "\n", "a", "a\n", "a\nb", "a\nb\n", "a\r\nb\r\n", "\r\n"] {
            assert_eq!(
                Document::parse(text).render(),
                text,
                "round trip of {text:?}"
            );
        }
    }

    #[test]
    fn read_reports_lines_and_hashes() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "one\ntwo\nthree\n");
        let read = read_file(&path, None, None).unwrap();
        assert_eq!(read.total_lines, 3);
        assert_eq!(read.first, 1);
        assert!(!read.truncated);
        assert_eq!(
            read.lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "three"]
        );
        assert_eq!(read.lines[1], LineView::new(2, "two"));

        let tail = read_file(&path, Some(2), Some(1)).unwrap();
        assert_eq!(tail.first, 2);
        assert_eq!(tail.lines, vec![LineView::new(2, "two")]);
        assert!(tail.truncated);

        let past_end = read_file(&path, Some(9), None).unwrap();
        assert!(past_end.lines.is_empty());
        assert_eq!(past_end.first, 4, "one past the last line");
        assert!(!past_end.truncated);

        assert!(
            read_file(&path, Some(0), None)
                .unwrap_err()
                .contains("1-based")
        );
        assert!(
            read_file(&path, None, Some(0))
                .unwrap_err()
                .contains("at least 1")
        );
        assert!(
            read_file(&dir.path().join("nope.bean"), None, None)
                .unwrap_err()
                .contains("no such file")
        );
        assert_eq!(
            read_file(&path, None, Some(usize::MAX))
                .unwrap()
                .lines
                .len(),
            3
        );
    }

    #[test]
    fn hash_survives_an_unrelated_edit() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "one\ntwo\nthree\n");
        let before = read_file(&path, None, None).unwrap();

        // Rewrite the last line; the anchors of the lines above it stay valid.
        apply_edits(&path, &[replace(&anchor(&before.lines[2]), &["THREE"])]).unwrap();

        let after = read_file(&path, None, None).unwrap();
        assert_eq!(after.lines[1].hash, before.lines[1].hash);
        assert_eq!(after.lines[0].hash, before.lines[0].hash);

        // The stale anchor of the line that did change is refused, with the line.
        let error = apply_edits(&path, &[replace(&anchor(&before.lines[2]), &["x"])]).unwrap_err();
        assert!(error.contains("line changed at 3"), "{error}");

        // A hash-only anchor of an unchanged line still resolves.
        let outcome = apply_edits(&path, &[replace(&before.lines[1].hash, &["TWO"])]).unwrap();
        assert_eq!(outcome.lines[0].text, "TWO");
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\nTWO\nTHREE\n");
    }

    #[test]
    fn replace_insert_delete_by_anchor() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "one\ntwo\nthree\n");

        let read = read_file(&path, None, None).unwrap();
        let outcome = apply_edits(&path, &[replace(&anchor(&read.lines[1]), &["TWO"])]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\nTWO\nthree\n");
        assert_eq!(outcome.first, 2);
        assert_eq!(outcome.lines[0].text, "TWO");

        // Chain without re-reading: the returned hash anchors the next edit.
        let outcome = apply_edits(
            &path,
            &[insert_after(&anchor(&outcome.lines[0]), &["2.5", "2.75"])],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\nTWO\n2.5\n2.75\nthree\n"
        );
        assert_eq!(outcome.first, 3);
        assert_eq!(
            outcome
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["2.5", "2.75"]
        );

        // Delete an inclusive range: the two inserted lines.
        let current = read_file(&path, None, None).unwrap();
        apply_edits(
            &path,
            &[delete(
                &anchor(&current.lines[2]),
                &anchor(&current.lines[3]),
            )],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\nTWO\nthree\n");

        // Replace with no lines is a delete, and a batch may chain ordered ops.
        let current = read_file(&path, None, None).unwrap();
        let outcome = apply_edits(
            &path,
            &[
                replace(&anchor(&current.lines[1]), &["TWO!", "extra"]),
                delete(&anchor(&current.lines[2]), &anchor(&current.lines[2])),
            ],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\nTWO!\nextra\n");
        assert_eq!(outcome.first, 2);
        assert_eq!(
            outcome
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["TWO!", "extra"]
        );
    }

    #[test]
    fn write_creates_and_replaces() {
        let dir = scratch();
        let path = dir.path().join("new.bean");

        let outcome = apply_edits(
            &path,
            &[EditOp::Write {
                content: "a\nb\n".into(),
            }],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\nb\n");
        assert!(outcome.written);
        assert_eq!(outcome.total_lines, 2);
        assert_eq!(outcome.lines.len(), 2);

        // An identical write is not a write at all.
        let outcome = apply_edits(
            &path,
            &[EditOp::Write {
                content: "a\nb\n".into(),
            }],
        )
        .unwrap();
        assert!(!outcome.written);

        let error = apply_edits(
            &path,
            &[
                EditOp::Write {
                    content: "x".into(),
                },
                replace("0000", &["y"]),
            ],
        )
        .unwrap_err();
        assert!(error.contains("cannot be combined"), "{error}");

        // Anchor edits on a file that does not exist point at `write`.
        let missing = dir.path().join("missing.bean");
        let error = apply_edits(&missing, &[replace("0000", &["y"])]).unwrap_err();
        assert!(error.contains("no such file"), "{error}");
        assert!(!missing.exists());
    }

    #[test]
    fn a_stale_anchor_refuses_the_whole_batch() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "one\ntwo\nthree\n");
        let read = read_file(&path, None, None).unwrap();

        // Someone else rewrites line 2 outside the tool.
        fs::write(&path, "one\nTWO(edited)\nthree\n").unwrap();
        let untouched = fs::read(&path).unwrap();

        let error = apply_edits(
            &path,
            &[
                replace(&anchor(&read.lines[0]), &["ONE"]),
                replace(&anchor(&read.lines[1]), &["TWO"]),
            ],
        )
        .unwrap_err();
        assert!(error.contains("line changed at 2"), "{error}");
        assert!(error.contains("operation 2"), "{error}");
        assert_eq!(
            fs::read(&path).unwrap(),
            untouched,
            "the file must not change"
        );

        // The same, anchored by hash alone: the line that changed is gone from
        // the file, so the hash no longer names anything.
        let error = apply_edits(&path, &[replace(&read.lines[1].hash, &["TWO"])]).unwrap_err();
        assert!(error.contains("line changed"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), untouched);
    }

    #[test]
    fn duplicate_lines_need_the_line_number() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "a\n\nb\n\nc\n");
        let read = read_file(&path, None, None).unwrap();
        assert_eq!(
            read.lines[1].hash, read.lines[3].hash,
            "blank lines hash alike"
        );

        let error = apply_edits(&path, &[replace(&read.lines[1].hash, &["x"])]).unwrap_err();
        assert!(error.contains("ambiguous anchor"), "{error}");
        assert!(error.contains("write"), "{error}");

        let outcome =
            apply_edits(&path, &[replace(&anchor(&read.lines[3]), &["between"])]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\n\nb\nbetween\nc\n");
        assert_eq!(outcome.first, 4);
    }

    #[test]
    fn inserts_at_one_point_keep_their_order() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "a\nb\n");
        let read = read_file(&path, None, None).unwrap();
        apply_edits(
            &path,
            &[
                insert_after(&anchor(&read.lines[0]), &["first"]),
                insert_after(&anchor(&read.lines[0]), &["second"]),
            ],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\nfirst\nsecond\nb\n");
    }

    #[test]
    fn malformed_anchors_are_explained() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "a\n");
        for raw in ["zz", "abc", "abcd:x", "abcd@0", "abcd@x", ""] {
            let error = apply_edits(&path, &[replace(raw, &["b"])]).unwrap_err();
            assert!(error.contains("operation 1"), "{raw}: {error}");
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\n");
    }

    #[test]
    fn out_of_order_and_overlapping_ops_are_refused() {
        let dir = scratch();
        let path = seed(&dir, "main.bean", "a\nb\nc\n");
        let read = read_file(&path, None, None).unwrap();

        let error = apply_edits(
            &path,
            &[
                replace(&anchor(&read.lines[2]), &["C"]),
                replace(&anchor(&read.lines[0]), &["A"]),
            ],
        )
        .unwrap_err();
        assert!(error.contains("out of order"), "{error}");

        let error = apply_edits(
            &path,
            &[
                delete(&anchor(&read.lines[0]), &anchor(&read.lines[1])),
                replace(&anchor(&read.lines[1]), &["B"]),
            ],
        )
        .unwrap_err();
        assert!(error.contains("overlap"), "{error}");

        let error = apply_edits(
            &path,
            &[delete(&anchor(&read.lines[1]), &anchor(&read.lines[0]))],
        )
        .unwrap_err();
        assert!(error.contains("give `from` first"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\nb\nc\n");
    }

    #[test]
    fn trailing_newline_and_crlf_survive() {
        let dir = scratch();
        let no_newline = seed(&dir, "no.bean", "a\nb");
        let read = read_file(&no_newline, None, None).unwrap();
        apply_edits(&no_newline, &[replace(&anchor(&read.lines[0]), &["A"])]).unwrap();
        assert_eq!(fs::read_to_string(&no_newline).unwrap(), "A\nb");

        let crlf = seed(&dir, "crlf.bean", "a\r\nb\r\n");
        let read = read_file(&crlf, None, None).unwrap();
        assert_eq!(read.lines[0].text, "a");
        apply_edits(&crlf, &[replace(&anchor(&read.lines[1]), &["B", "C"])]).unwrap();
        assert_eq!(fs::read_to_string(&crlf).unwrap(), "a\r\nB\r\nC\r\n");

        let empty = seed(&dir, "empty.bean", "");
        let read = read_file(&empty, None, None).unwrap();
        assert_eq!(read.total_lines, 0);
        let outcome = apply_edits(
            &empty,
            &[EditOp::Write {
                content: "x\n".into(),
            }],
        )
        .unwrap();
        assert_eq!(outcome.total_lines, 1);
    }

    #[test]
    fn binary_files_are_refused() {
        let dir = scratch();
        let path = dir.path().join("blob.bin");
        fs::write(&path, [0u8, 159, 146, 150]).unwrap();
        let error = read_file(&path, None, None).unwrap_err();
        assert!(error.contains("not a text file"), "{error}");
    }
}
