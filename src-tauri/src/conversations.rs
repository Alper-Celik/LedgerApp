// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Conversations with the ledger: one markdown file each, kept inside the
//! ledger directory so they are committed and synced with it.
//!
//! The store is `<ledger dir>/.ledger/sessions/`, one
//! `<created date>-<slug>-<id>.md` per conversation. A conversation belongs to
//! the ledger it argues about, so keeping it next to the journal means it
//! travels with the same git repository — no second store, no second sync, and
//! the text a conversation produced is one `git log` away from the entry it
//! changed. [`DIRECTORY`] and [`is_session_path`] exist so the rest of the app
//! can tell that corner apart from the journal's own files: a conversation is
//! not an entry, so the register, the balance views and the diff view must not
//! see one.
//!
//! # The file
//!
//! ```text
//! ---
//! id: 7f3a9c21
//! title: March budget shortfall
//! created: 2026-10-29T09:12:03Z
//! updated: 2026-10-29T09:15:41Z
//! model: gpt-5
//! tags: [budget, 2026]
//! ---
//!
//! ## user
//!
//! Why is March short?
//!
//! ## assistant
//!
//! The annual premium lands in March.
//! ```
//!
//! The frontmatter is deliberately not YAML proper: one `key: value` per line in
//! the fixed order above, `tags` an optional single-line flow list, every other
//! key ignored on the way back in so a future field cannot break an older
//! reader. `id`, `title`, `created` and `updated` are required — a file missing
//! one is malformed and [`list`] says so — while `model` may be empty. Timestamps
//! are RFC 3339 in UTC at second precision — always twenty characters, which is
//! what lets an append rewrite the `updated` value in place without shifting a
//! byte around it.
//!
//! Messages are markdown sections under a `## user`, `## assistant` or
//! `## tool` heading, in the order they were said. A tool message may end with
//! a fenced `json` block holding the structured payload the tool returned; that
//! block is lifted out of the text when the file is read. A body line that is
//! exactly one of those three headings would be read as the start of the next
//! message, so the writer escapes it with a leading backslash (and escapes an
//! already-escaped one with another), which the reader takes back off — the
//! round trip is exact for every body.
//!
//! # Appends are append-only
//!
//! Two devices appending different messages to one conversation must produce a
//! file git can merge, so [`append`] never rewrites what is already there: it
//! updates the single `updated:` value in place and then writes the new section
//! onto the end with one `O_APPEND` write, which the kernel places at the end
//! without a read-modify-write. The `updated:` line is the one exception, and
//! the one line a merge can conflict on — resolving it by taking either side
//! loses nothing, since it only says when the file was last touched.
//!
//! Creating a file, [`rename`], and the fallback for a hand-edited timestamp
//! whose width differs all write a temp file in the same directory, `sync_all`
//! it, and `rename` it over the target: a reader sees the old file or the new
//! one, never half of either.
//!
//! # Ids and paths
//!
//! A conversation is addressed by its `id`, never by a path: [`load`],
//! [`append`], [`rename`] and [`delete`] look the id up in the frontmatter of
//! the files in the store, and an id that is not plain `[a-z0-9-]` is refused
//! outright. Nothing the frontend sends is ever joined into a path.
//!
//! The filename records when the conversation was created and is never moved
//! again — [`rename`] changes the title in the frontmatter only. Renaming the
//! file would make a rename on one device and an append on another collide in
//! git, where the id-based path lets both land.

use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Any;

/// The app's own directory inside a ledger, beside the journal's files but not
/// part of them. Anything under it is the app's business, not the ledger's.
pub const DIRECTORY: &str = ".ledger";

/// The conversations inside [`DIRECTORY`].
const SESSIONS: &str = "sessions";

/// The one extension the store reads; anything else in the directory — a temp
/// file mid-write, the OS's litter — is not a conversation.
const EXTENSION: &str = "md";

/// How many characters a generated id holds. Eight base-36 characters is about
/// 41 random bits, which is a collision risk worth ignoring for one ledger.
const ID_LENGTH: usize = 8;

/// How much of the title the filename carries. A slug only has to be readable
/// and identifying; the full title stays in the frontmatter.
const SLUG_LIMIT: usize = 40;

/// How much of a message becomes the title when a conversation is created from
/// its first message — a subject line, not a transcript.
const TITLE_LIMIT: usize = 120;

/// Who said a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

impl Role {
    /// The role as it is written under the heading, and as it reads in the
    /// frontmatter-free part of the file.
    fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// One message of a conversation.
///
/// `data` is a tool message's structured payload — the rows a query returned,
/// say — kept beside the prose rather than in it, so the frontend can render the
/// result instead of re-reading a fenced block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// Arbitrary JSON for a [`Role::Tool`] message. Described to TypeScript as
    /// `any`: specta's own `serde_json::Value` support is off in this crate (it
    /// is a feature of `specta` that nothing else here needs), and the payload
    /// is a tool's own shape, which no generated type could say better.
    #[specta(type = Option<Any>)]
    pub data: Option<serde_json::Value>,
}

/// A conversation as the list shows it: enough to draw a row and to find the
/// file, without reading every message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Summary {
    pub id: String,
    pub title: String,
    /// RFC 3339, UTC.
    pub created: String,
    /// RFC 3339, UTC — the field the listing is ordered by.
    pub updated: String,
    /// How many messages the file holds.
    pub messages: u32,
    /// The file itself, as a display path.
    pub path: String,
}

/// A conversation in full: the summary's fields, the frontmatter's remaining
/// fields, and every message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created: String,
    pub updated: String,
    /// The model the conversation was started with, or empty when the caller
    /// did not say.
    pub model: String,
    pub tags: Vec<String>,
    pub messages: Vec<Message>,
    /// The file itself, as a display path.
    pub path: String,
}

/// A file the store could not read. One bad file must not hide the good ones,
/// so [`list`] hands these back beside the conversations it did read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MalformedFile {
    pub path: String,
    /// What is wrong with it, in the words a person can act on.
    pub error: String,
}

/// Everything [`list`] found: the conversations, newest-updated first, and the
/// files it had to skip.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Listing {
    pub sessions: Vec<Summary>,
    pub errors: Vec<MalformedFile>,
}

/// Where a ledger's conversations live.
pub fn sessions_dir(ledger_dir: &Path) -> PathBuf {
    ledger_dir.join(DIRECTORY).join(SESSIONS)
}

/// Whether `path` names something inside the app's own [`DIRECTORY`] — a
/// conversation, the settings beside it — rather than the journal's own files.
///
/// The test is the directory *component*, not the text: `.ledger/sessions/x.md`
/// is the app's, and so is `assets/.ledger/x`, while a journal called
/// `books/.ledgernote.md` is not.
pub fn is_session_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == DIRECTORY)
}

/// A fresh id: eight lowercase base-36 characters.
///
/// `RandomState` is seeded from the OS, so hashing anything with a new one is
/// as random as asking the OS for bytes — without pulling a random-number crate
/// in for eight characters. Two ids colliding inside one ledger is not a case
/// worth handling further: a caller that mints an id it already has will be
/// told the conversation exists.
pub fn new_id() -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut value = random_u64();
    let mut id = String::with_capacity(ID_LENGTH);
    for _ in 0..ID_LENGTH {
        id.push(char::from(ALPHABET[(value % 36) as usize]));
        value /= 36;
    }
    id
}

/// Every conversation the store holds, newest `updated` first, with the files
/// that could not be read reported beside them.
///
/// Files that end in anything but `.md` are not conversations and are ignored —
/// that is what keeps a temp file mid-write out of the listing. Two files
/// claiming one id (git merged two copies of a conversation) are resolved by
/// keeping the newer and reporting the older, so the listing never shows the
/// same conversation twice.
pub fn list(ledger_dir: &Path) -> Result<Listing, String> {
    let dir = sessions_dir(ledger_dir);
    let mut found = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        // A ledger nobody has talked to yet has no store, which is not a
        // failure to list it.
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Listing::default()),
        Err(error) => return Err(format!("read {}: {error}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some(EXTENSION) {
            continue;
        }
        match read_summary(&path) {
            Ok(summary) => found.push(summary),
            Err(error) => errors.push(MalformedFile {
                path: path.display().to_string(),
                error,
            }),
        }
    }
    found.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    let mut sessions: Vec<Summary> = Vec::with_capacity(found.len());
    for summary in found {
        match sessions.iter().find(|kept| kept.id == summary.id) {
            Some(kept) => errors.push(MalformedFile {
                path: summary.path,
                error: format!(
                    "duplicate id `{}`: {} already holds it",
                    summary.id, kept.path
                ),
            }),
            None => sessions.push(summary),
        }
    }
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Listing { sessions, errors })
}

/// One conversation, every message of it.
pub fn load(ledger_dir: &Path, id: &str) -> Result<Conversation, String> {
    let (path, _) = locate(&sessions_dir(ledger_dir), id)?
        .ok_or_else(|| format!("no conversation with id `{id}`"))?;
    let text = read_text(&path)?;
    let (meta, messages) = parse(&text)?;
    Ok(Conversation {
        id: meta.id,
        title: meta.title,
        created: meta.created,
        updated: meta.updated,
        model: meta.model,
        tags: meta.tags,
        messages,
        path: path.display().to_string(),
    })
}

/// Append a message, creating the conversation when the id is new, and answer
/// with the file as it now stands.
///
/// The file is created with the message already in it, in one atomic write, so
/// there is no moment at which an empty conversation exists. A conversation
/// created this way is titled after its first message and carries no model;
/// [`create`] is the call for a caller that knows better.
pub fn append(ledger_dir: &Path, id: &str, message: &Message) -> Result<Summary, String> {
    validate_id(id)?;
    let dir = sessions_dir(ledger_dir);
    let now = now_rfc3339();
    match locate(&dir, id)? {
        Some((path, meta)) => {
            // Only the timestamp moves; every other byte of the file stays as
            // the other device left it, so git can merge the two appends.
            replace_updated(&path, &meta, &now)?;
            append_block(&path, &message_block(message))?;
            // Another device may have appended while this one wrote, so the
            // count comes from the file rather than from the number of appends
            // this process happens to know about.
            read_summary(&path)
        }
        None => {
            let meta = Meta {
                id: id.to_string(),
                title: title_from(message),
                created: now.clone(),
                updated: now,
                model: String::new(),
                tags: Vec::new(),
                title_range: 0..0,
                updated_range: 0..0,
            };
            let path = dir.join(file_name(&meta.created, &meta.title, id));
            fs::create_dir_all(&dir)
                .map_err(|error| format!("create {}: {error}", dir.display()))?;
            let text = render(&meta, std::slice::from_ref(message));
            write_atomic(&path, text.as_bytes())?;
            read_summary(&path)
        }
    }
}

/// Start a conversation with its frontmatter filled in and no messages, for a
/// caller that knows the model and the tags before the first message.
///
/// Fails when the id is already taken rather than writing over a conversation
/// someone else may have had.
pub fn create(
    ledger_dir: &Path,
    id: &str,
    title: &str,
    model: &str,
    tags: &[String],
) -> Result<Summary, String> {
    validate_id(id)?;
    let dir = sessions_dir(ledger_dir);
    if locate(&dir, id)?.is_some() {
        return Err(format!("a conversation with id `{id}` already exists"));
    }
    let now = now_rfc3339();
    let meta = Meta {
        id: id.to_string(),
        title: flatten_title(title),
        created: now.clone(),
        updated: now,
        model: model.trim().to_string(),
        tags: clean_tags(tags),
        title_range: 0..0,
        updated_range: 0..0,
    };
    let path = dir.join(file_name(&meta.created, &meta.title, id));
    fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    write_atomic(&path, render(&meta, &[]).as_bytes())?;
    read_summary(&path)
}

/// Change a conversation's title.
///
/// Only the frontmatter changes — the file keeps its name, because the name is
/// the record of when the conversation started and the id is what finds it.
/// Everything else in the file is copied across byte for byte, so a rename does
/// not disturb what an append on another device wrote.
pub fn rename(ledger_dir: &Path, id: &str, title: &str) -> Result<Summary, String> {
    let (path, meta) = locate(&sessions_dir(ledger_dir), id)?
        .ok_or_else(|| format!("no conversation with id `{id}`"))?;
    let text = read_text(&path)?;
    let title = flatten_title(title);
    let now = now_rfc3339();
    let next = splice(
        &text,
        &[
            (meta.title_range.clone(), title.as_str()),
            (meta.updated_range.clone(), now.as_str()),
        ],
    )?;
    write_atomic(&path, next.as_bytes())?;
    read_summary(&path)
}

/// Remove a conversation's file.
///
/// There is no archive and no undo here: the file lives in the ledger's git
/// repository, so the history — and the way back — is git's.
pub fn delete(ledger_dir: &Path, id: &str) -> Result<(), String> {
    let (path, _) = locate(&sessions_dir(ledger_dir), id)?
        .ok_or_else(|| format!("no conversation with id `{id}`"))?;
    fs::remove_file(&path).map_err(|error| format!("remove {}: {error}", path.display()))
}

/// Whether `id` is one the store will address: a short, plain, lowercase name.
///
/// This is the whole of the path safety: an id is never turned into a path, and
/// anything that is not `[a-z0-9-]` is refused before the store is even asked,
/// so no separator, no `..`, no absolute path and no case-mangled filesystem
/// trick can reach a file.
fn validate_id(id: &str) -> Result<(), String> {
    if is_valid_id(id) {
        Ok(())
    } else {
        Err(format!(
            "`{id}` is not a conversation id: ids are 1 to 64 characters of a-z, 0-9 and `-`"
        ))
    }
}

/// [`validate_id`]'s rule, as a question, for the frontmatter's own ids.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The meta a file's frontmatter holds, with the byte ranges an in-place
/// rewrite splices. The ranges are only meaningful for the text it was parsed
/// from.
struct Meta {
    id: String,
    title: String,
    created: String,
    updated: String,
    model: String,
    tags: Vec<String>,
    title_range: Range<usize>,
    updated_range: Range<usize>,
}

/// The file a conversation id lives in, parsed far enough to know it is the
/// right one.
///
/// A file whose frontmatter cannot be read is normally just skipped — [`list`]
/// reports it — but a broken file whose *name* claims `id` is an error rather
/// than an absence: answering "no such conversation" would let [`append`]
/// create a second file beside the one the user is looking at.
fn locate(dir: &Path, id: &str) -> Result<Option<(PathBuf, Meta)>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", dir.display())),
    };
    let mut found: Vec<(PathBuf, Meta)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some(EXTENSION) {
            continue;
        }
        match read_text(&path).and_then(|text| parse(&text)) {
            Ok((meta, _)) => {
                if meta.id == id {
                    found.push((path, meta));
                }
            }
            Err(error) => {
                if names_id(&path, id) {
                    return Err(format!("{} is unreadable: {error}", path.display()));
                }
            }
        }
    }
    // Two files with one id should not happen; the newer one is the answer so
    // the store behaves like the listing that hides the older one.
    found.sort_by(|a, b| {
        b.1.updated
            .cmp(&a.1.updated)
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(found.into_iter().next())
}

/// Whether the file's name ends in `-<id>.md`, the shape the store writes.
fn names_id(path: &Path, id: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(&format!(".{EXTENSION}")))
        .is_some_and(|stem| stem.ends_with(&format!("-{id}")))
}

fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))
}

/// A file's summary, re-read rather than remembered: the count and the
/// timestamp are the file's, not this process's guess.
fn read_summary(path: &Path) -> Result<Summary, String> {
    let text = read_text(path)?;
    let (meta, messages) = parse(&text)?;
    Ok(Summary {
        id: meta.id,
        title: meta.title,
        created: meta.created,
        updated: meta.updated,
        messages: u32::try_from(messages.len()).unwrap_or(u32::MAX),
        path: path.display().to_string(),
    })
}

/// A text's lines as `(offset, length including the newline, line without it)`.
fn lines_of(text: &str) -> Vec<(usize, usize, &str)> {
    let mut lines = Vec::new();
    let mut start = 0;
    for raw in text.split_inclusive('\n') {
        lines.push((start, raw.len(), raw.trim_end_matches(['\n', '\r'])));
        start += raw.len();
    }
    lines
}

/// A frontmatter line's `key`, and its value as a byte range in the whole text
/// plus the value itself — the range is what an in-place rewrite splices.
fn split_pair(text_start: usize, line: &str) -> Option<(&str, Range<usize>, &str)> {
    let colon = line.find(':')?;
    let after = &line[colon + 1..];
    let lead = after.len() - after.trim_start().len();
    let value = after.trim();
    let start = text_start + colon + 1 + lead;
    Some((line[..colon].trim(), start..start + value.len(), value))
}

/// Read a conversation file: its frontmatter and its messages.
fn parse(text: &str) -> Result<(Meta, Vec<Message>), String> {
    let mut lines = lines_of(text).into_iter();
    match lines.next() {
        Some((_, _, first)) if first.trim() == "---" => {}
        Some(_) => {
            return Err("the file does not open with a `---` frontmatter block".to_string());
        }
        None => return Err("the file is empty".to_string()),
    }
    let mut id = None;
    let mut title = None;
    let mut created = None;
    let mut updated = None;
    let mut model = String::new();
    let mut tags = Vec::new();
    let mut title_range = 0..0;
    let mut updated_range = 0..0;
    let mut body_start = None;
    for (start, length, line) in lines {
        if line.trim() == "---" {
            body_start = Some(start + length);
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, range, value)) = split_pair(start, line) else {
            return Err(format!("`{line}` in the frontmatter is not a `key: value` pair"));
        };
        match key {
            "id" => {
                if !is_valid_id(value) {
                    return Err(format!("`{value}` in the frontmatter is not a conversation id"));
                }
                id = Some(value.to_string());
            }
            "title" => {
                title = Some(value.to_string());
                title_range = range;
            }
            "created" => {
                if !is_timestamp(value) {
                    return Err(format!("`{value}` is not an RFC 3339 timestamp"));
                }
                created = Some(value.to_string());
            }
            "updated" => {
                if !is_timestamp(value) {
                    return Err(format!("`{value}` is not an RFC 3339 timestamp"));
                }
                updated = Some(value.to_string());
                updated_range = range;
            }
            "model" => model = value.to_string(),
            "tags" => tags = parse_tags(value),
            // A field from a newer writer is the one thing an older reader must
            // not choke on: it is ignored, and the file keeps it untouched.
            _ => {}
        }
    }
    let Some(body_start) = body_start else {
        return Err("the frontmatter is not closed by `---`".to_string());
    };
    let Some(id) = id else {
        return Err("the frontmatter names no id".to_string());
    };
    let Some(title) = title else {
        return Err("the frontmatter carries no `title`".to_string());
    };
    let Some(created) = created else {
        return Err("the frontmatter carries no `created` timestamp".to_string());
    };
    let Some(updated) = updated else {
        return Err("the frontmatter carries no `updated` timestamp".to_string());
    };
    let messages = parse_messages(&text[body_start..])?;
    Ok((
        Meta {
            id,
            title,
            created,
            updated,
            model,
            tags,
            title_range,
            updated_range,
        },
        messages,
    ))
}

/// The frontmatter's `tags: [a, b]` line, if it has one.
fn parse_tags(value: &str) -> Vec<String> {
    let Some(inner) = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return clean_tags(&[value.to_string()]);
    };
    clean_tags(&inner.split(',').map(str::to_string).collect::<Vec<_>>())
}

/// Tags as the store keeps them: trimmed, without commas or line breaks, and
/// without the empties a trailing comma leaves behind.
fn clean_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|tag| tag.trim().replace([',', '\n', '\r'], " "))
        .map(|tag| tag.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// The messages of a file body, in the order the headings put them.
fn parse_messages(body: &str) -> Result<Vec<Message>, String> {
    let mut messages = Vec::new();
    let mut current: Option<(Role, Vec<&str>)> = None;
    for (_, _, line) in lines_of(body) {
        if let Some(role) = heading_role(line) {
            if let Some((role, body)) = current.take() {
                messages.push(build_message(role, body)?);
            }
            current = Some((role, Vec::new()));
            continue;
        }
        match &mut current {
            Some((_, body)) => body.push(line),
            None => {
                if !line.trim().is_empty() {
                    return Err(format!(
                        "`{line}` sits before the first message heading"
                    ));
                }
            }
        }
    }
    if let Some((role, body)) = current {
        messages.push(build_message(role, body)?);
    }
    Ok(messages)
}

/// The role a line opens, if it opens one. Exact, not trimmed: a body line
/// indented under a list is that body's text, not the next message.
fn heading_role(line: &str) -> Option<Role> {
    match line {
        "## user" => Some(Role::User),
        "## assistant" => Some(Role::Assistant),
        "## tool" => Some(Role::Tool),
        _ => None,
    }
}

/// One message's body: blank framing off both ends, a tool's trailing `json`
/// fence lifted out, and the writer's heading escapes taken back off.
fn build_message(role: Role, lines: Vec<&str>) -> Result<Message, String> {
    let first = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(first, |last| last + 1);
    let mut lines = &lines[first..last];
    let mut data = None;
    if role == Role::Tool {
        if let Some((value, rest)) = trailing_json(lines)? {
            data = Some(value);
            lines = rest;
        }
    }
    let text = lines
        .iter()
        .map(|line| unescape(line))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Message { role, text, data })
}

/// A tool message's payload: the fenced `json` block at the very end of its
/// body, and the rest of the body without it. A body that ends in some other
/// fence is text, not a payload.
fn trailing_json<'a>(
    lines: &'a [&'a str],
) -> Result<Option<(serde_json::Value, &'a [&'a str])>, String> {
    let Some((last, rest)) = lines.split_last() else {
        return Ok(None);
    };
    if last.trim() != "```" {
        return Ok(None);
    }
    let Some(opener) = rest.iter().rposition(|line| line.trim() == "```json") else {
        return Ok(None);
    };
    let payload = rest[opener + 1..].join("\n");
    let value = serde_json::from_str(&payload)
        .map_err(|error| format!("a tool message's payload is not JSON: {error}"))?;
    let mut remaining = &rest[..opener];
    while remaining
        .last()
        .is_some_and(|line| line.trim().is_empty())
    {
        remaining = &remaining[..remaining.len() - 1];
    }
    Ok(Some((value, remaining)))
}

/// A body line as the writer stores it: one that would be read back as a
/// heading gets a backslash, and one that already starts with one gets another,
/// so the reader's single unescape is exact for either.
fn escape(line: &str) -> String {
    if heading_role(line.trim_start_matches('\\')).is_some() {
        format!("\\{line}")
    } else {
        line.to_string()
    }
}

/// The inverse of [`escape`] for one body line: take off exactly one backslash,
/// but only from a line the writer escaped — a line that would be a heading once
/// its own backslashes are set aside.
fn unescape(line: &str) -> String {
    match line.strip_prefix('\\') {
        Some(rest) if heading_role(rest.trim_start_matches('\\')).is_some() => rest.to_string(),
        _ => line.to_string(),
    }
}

/// A whole file: frontmatter, then a block per message.
fn render(meta: &Meta, messages: &[Message]) -> String {
    let mut text = String::new();
    text.push_str("---\n");
    text.push_str(&format!("id: {}\n", meta.id));
    text.push_str(&format!("title: {}\n", meta.title));
    text.push_str(&format!("created: {}\n", meta.created));
    text.push_str(&format!("updated: {}\n", meta.updated));
    // A model name is user input (typed in settings); one newline in it would
    // add a frontmatter key or close the block early.
    text.push_str(&format!("model: {}\n", flatten_title(&meta.model)));
    if !meta.tags.is_empty() {
        text.push_str(&format!("tags: [{}]\n", meta.tags.join(", ")));
    }
    text.push_str("---\n");
    for message in messages {
        text.push_str(&message_block(message));
    }
    text
}

/// One message as it is appended: a blank line, its heading, its body, and the
/// payload fence when it has one.
///
/// The block starts with the newline that separates it from whatever is already
/// in the file, so appending never has to look at how the file ends — even a
/// hand-edited file missing its last newline keeps its last line intact.
fn message_block(message: &Message) -> String {
    let mut text = String::new();
    text.push('\n');
    text.push_str("## ");
    text.push_str(message.role.as_str());
    text.push_str("\n\n");
    let body = message.text.trim_matches(['\n', '\r']);
    if !body.is_empty() {
        for line in body.split('\n') {
            text.push_str(&escape(line));
            text.push('\n');
        }
    }
    if let Some(data) = &message.data {
        if !body.is_empty() {
            text.push('\n');
        }
        // Pretty, because this file is meant to be read and diffed by a person
        // as much as by the app; a tool's payload is stable enough to print.
        let payload = serde_json::to_string_pretty(data).unwrap_or_else(|_| "null".to_string());
        text.push_str("```json\n");
        text.push_str(&payload);
        text.push_str("\n```\n");
    }
    text
}

/// Replace only the `updated` value, at its offset.
///
/// The store always writes a twenty-character UTC timestamp, so the usual case
/// is a fixed-width overwrite that leaves every other byte where it was; the
/// fallback is for a hand-edited value with an offset or a fraction, whose width
/// differs.
fn replace_updated(path: &Path, meta: &Meta, now: &str) -> Result<(), String> {
    let range = &meta.updated_range;
    if range.len() == now.len() {
        let mut file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        file.seek(SeekFrom::Start(range.start as u64))
            .map_err(|error| format!("seek {}: {error}", path.display()))?;
        file.write_all(now.as_bytes())
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        file.sync_all()
            .map_err(|error| format!("flush {}: {error}", path.display()))?;
        return Ok(());
    }
    let text = read_text(path)?;
    let next = splice(&text, &[(range.clone(), now)])?;
    write_atomic(path, next.as_bytes())
}

/// Add `block` to the end of the file with one append write.
///
/// `O_APPEND` puts the write at the file's current end inside the kernel, so two
/// appenders — two windows, two devices syncing through one checkout — both
/// land, each block whole. A short write (the kernel taking less than the whole
/// buffer) is completed rather than dropped.
fn append_block(path: &Path, block: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    let bytes = block.as_bytes();
    let written = file
        .write(bytes)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    if written < bytes.len() {
        file.write_all(&bytes[written..])
            .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    file.sync_all()
        .map_err(|error| format!("flush {}: {error}", path.display()))
}

/// Replace byte ranges in `text`, keeping everything between them.
fn splice(text: &str, edits: &[(Range<usize>, &str)]) -> Result<String, String> {
    let mut edits = edits.to_vec();
    edits.sort_by_key(|(range, _)| range.start);
    let mut next = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, replacement) in edits {
        if range.start < cursor || range.end > text.len() || range.start > range.end {
            return Err("the frontmatter moved under the edit".to_string());
        }
        next.push_str(&text[cursor..range.start]);
        next.push_str(replacement);
        cursor = range.end;
    }
    next.push_str(&text[cursor..]);
    Ok(next)
}

/// Write `bytes` to `path` whole or not at all: a temp file beside it, flushed
/// to the disk, renamed over it.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("conversation");
    // A dotted name that does not end in `.md`: a listing or a git status that
    // ran mid-write must not take the temp file for a conversation.
    let temp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        // The rename is durable only once the directory itself is flushed.
        if let Ok(dir) = File::open(dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    };
    match write() {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(format!("write {}: {error}", path.display()))
        }
    }
}

/// The name a conversation is created under: the creation date, the title as a
/// slug, and the id that finds it.
fn file_name(created: &str, title: &str, id: &str) -> String {
    let date = created.get(..10).unwrap_or("1970-01-01");
    format!("{date}-{}-{id}.{EXTENSION}", slug_of(title))
}

/// A title reduced to something a filename can hold: ASCII lowercase words,
/// single dashes between them, at most [`SLUG_LIMIT`] characters.
fn slug_of(title: &str) -> String {
    let mut slug = String::new();
    for word in title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
    {
        if !slug.is_empty() {
            if slug.len() + 1 + word.len() > SLUG_LIMIT {
                break;
            }
            slug.push('-');
        }
        let room = SLUG_LIMIT - slug.len();
        slug.push_str(&word[..word.len().min(room)]);
    }
    if slug.is_empty() {
        // A title with nothing ASCII in it still has to make a filename.
        "conversation".to_string()
    } else {
        slug
    }
}

/// A title as the frontmatter holds it: one line, no runs of whitespace.
fn flatten_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The title a conversation created from a message is given: the message's
/// first line, a markdown heading's text, cut at [`TITLE_LIMIT`] characters.
fn title_from(message: &Message) -> String {
    let line = message
        .text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let line = line.trim_start_matches('#').trim();
    let mut title: String = line.chars().take(TITLE_LIMIT).collect();
    if line.chars().count() > TITLE_LIMIT {
        title.push('…');
    }
    flatten_title(&title)
}

/// 64 bits from the OS, through a freshly seeded hasher.
fn random_u64() -> u64 {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(now_seconds().unsigned_abs());
    hasher.write_u32(std::process::id());
    let first = hasher.finish();
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u64(first);
    second.finish()
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn now_rfc3339() -> String {
    timestamp_of(now_seconds())
}

/// `YYYY-MM-DDTHH:MM:SSZ` — RFC 3339 in UTC, at second precision, always
/// twenty characters so that rewriting one in place never moves a byte.
fn timestamp_of(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = date_of(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

/// The calendar date `days` days after 1970-01-01 — Howard Hinnant's
/// `civil_from_days`, which counts from a March-based era so leap days fall at
/// the end of a year and the arithmetic has no special case.
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

/// Whether `value` is an RFC 3339 timestamp: the shape dates and times take in
/// the frontmatter, with any offset — the store writes `Z`, and a hand edit
/// elsewhere is read but not rewritten in place.
fn is_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    let shaped = bytes.iter().enumerate().all(|(index, byte)| match index {
        4 | 7 => *byte == b'-',
        10 => *byte == b'T',
        13 | 16 => *byte == b':',
        0..=3 | 5..=6 | 8..=9 | 11..=12 | 14..=15 | 17..=18 => byte.is_ascii_digit(),
        _ => true,
    });
    shaped && is_zone(&value[19..])
}

/// A timestamp's zone: `Z`, or an offset in `+HH:MM` form, after an optional
/// fractional second.
fn is_zone(rest: &str) -> bool {
    if rest.eq_ignore_ascii_case("z") {
        return true;
    }
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let after_digits = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
            if after_digits.len() == fraction.len() {
                return false;
            }
            after_digits
        }
        None => rest,
    };
    let Some(offset) = rest.strip_prefix(['+', '-']) else {
        return false;
    };
    let bytes = offset.as_bytes();
    bytes.len() == 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn message(role: Role, text: &str) -> Message {
        Message {
            role,
            text: text.to_string(),
            data: None,
        }
    }

    /// The file a conversation id was written to, through the store's own
    /// listing — the tests never join an id into a path either.
    fn path_of(dir: &TempDir, id: &str) -> PathBuf {
        PathBuf::from(
            list(dir.dir())
                .unwrap()
                .sessions
                .into_iter()
                .find(|session| session.id == id)
                .expect("the conversation is listed")
                .path,
        )
    }

    /// A file's text without its `updated:` line — what an append must leave
    /// alone in full.
    fn without_updated(text: &str) -> String {
        text.lines()
            .filter(|line| !line.starts_with("updated: "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn append_creates_then_loads_the_conversation() {
        let dir = TempDir::new("conversations-round-trip");
        let id = new_id();
        assert_eq!(id.len(), ID_LENGTH);
        assert!(is_valid_id(&id));

        let first = append(
            dir.dir(),
            &id,
            &message(Role::User, "Why is the budget short in March?"),
        )
        .unwrap();
        assert_eq!(first.id, id);
        assert_eq!(first.messages, 1);
        assert_eq!(first.title, "Why is the budget short in March?");
        assert!(is_timestamp(&first.created));
        assert!(is_timestamp(&first.updated));

        let name = Path::new(&first.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_string();
        assert!(name.starts_with(&first.created[..10]), "{name}");
        assert!(name.contains("-why-is-the-budget-short-in-march-"));
        assert!(name.ends_with(&format!("-{id}.md")), "{name}");

        let second = append(
            dir.dir(),
            &id,
            &message(Role::Assistant, "The annual premium lands in March."),
        )
        .unwrap();
        assert_eq!(second.messages, 2);
        assert_eq!(second.path, first.path);

        let loaded = load(dir.dir(), &id).unwrap();
        assert_eq!(
            loaded
                .messages
                .iter()
                .map(|message| (message.role, message.text.as_str()))
                .collect::<Vec<_>>(),
            [
                (Role::User, "Why is the budget short in March?"),
                (Role::Assistant, "The annual premium lands in March."),
            ]
        );
        assert_eq!(loaded.title, first.title);
        assert_eq!(loaded.path, first.path);
        assert_eq!(loaded.messages.len(), 2);

        let listing = list(dir.dir()).unwrap();
        assert_eq!(listing.sessions.len(), 1);
        assert!(listing.errors.is_empty());
    }

    #[test]
    fn appending_leaves_every_earlier_byte_alone() {
        let dir = TempDir::new("conversations-append-only");
        let id = new_id();
        append(dir.dir(), &id, &message(Role::User, "First")).unwrap();
        let path = path_of(&dir, &id);
        let before = fs::read_to_string(&path).unwrap();
        // The first append's own text is a byte prefix of the second's, minus
        // the one line an append is allowed to move.
        assert!(before.ends_with("## user\n\nFirst\n"));

        append(dir.dir(), &id, &message(Role::Assistant, "Second")).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        let kept = without_updated(&before);
        assert!(
            without_updated(&after).starts_with(&kept),
            "the earlier text was disturbed:\n{after}"
        );
        assert!(after.ends_with("## assistant\n\nSecond\n"));
        assert!(after.len() > before.len());
    }

    #[test]
    fn a_malformed_file_is_reported_beside_the_readable_ones() {
        let dir = TempDir::new("conversations-malformed");
        let id = new_id();
        append(dir.dir(), &id, &message(Role::User, "Readable")).unwrap();
        let broken = sessions_dir(dir.dir()).join("2026-10-29-broken-broken.md");
        fs::write(&broken, "---\nid: broken\n").unwrap();

        let listing = list(dir.dir()).unwrap();
        assert_eq!(listing.sessions.len(), 1);
        assert_eq!(listing.sessions[0].id, id);
        assert_eq!(listing.errors.len(), 1);
        assert!(listing.errors[0].path.ends_with("-broken.md"));
        assert!(!listing.errors[0].error.is_empty());
        assert_eq!(load(dir.dir(), &id).unwrap().messages.len(), 1);
        // The broken file is not mistaken for a conversation, and appending to
        // it is refused rather than done to a second file.
        assert!(load(dir.dir(), "broken").is_err());
        assert!(append(dir.dir(), "broken", &message(Role::User, "x")).is_err());
        assert_eq!(fs::read_dir(sessions_dir(dir.dir())).unwrap().count(), 2);
    }

    #[test]
    fn two_appends_at_once_both_survive() {
        let dir = TempDir::new("conversations-concurrent");
        let id = new_id();
        append(dir.dir(), &id, &message(Role::User, "Start")).unwrap();

        let ledger = dir.dir().to_path_buf();
        let handles: Vec<_> = ["first device", "second device"]
            .into_iter()
            .map(|text| {
                let ledger = ledger.clone();
                let id = id.clone();
                std::thread::spawn(move || append(&ledger, &id, &message(Role::Assistant, text)))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let loaded = load(dir.dir(), &id).unwrap();
        assert_eq!(loaded.messages.len(), 3);
        let texts: Vec<&str> = loaded
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert!(texts.contains(&"first device"), "{texts:?}");
        assert!(texts.contains(&"second device"), "{texts:?}");
        // Neither block was cut into: each is still a whole section.
        let text = fs::read_to_string(path_of(&dir, &id)).unwrap();
        assert_eq!(text.matches("## assistant\n\nfirst device\n").count(), 1);
        assert_eq!(text.matches("## assistant\n\nsecond device\n").count(), 1);
    }

    #[test]
    fn ids_that_are_paths_are_refused() {
        let dir = TempDir::new("conversations-ids");
        let ids = [
            "",
            "../escape",
            "..",
            "a/b",
            "Upper",
            "with space",
            ".hidden",
            "a\\b",
            "a:b",
            "a\0b",
            "çay",
        ]
        .map(str::to_string)
        .into_iter()
        .chain(std::iter::once("a".repeat(65)));
        for id in ids {
            let id = id.as_str();
            assert!(load(dir.dir(), id).is_err(), "{id:?} should be refused");
            assert!(
                append(dir.dir(), id, &message(Role::User, "hi")).is_err(),
                "{id:?} should be refused"
            );
            assert!(delete(dir.dir(), id).is_err(), "{id:?} should be refused");
        }
        // Nothing was created anywhere for any of them.
        assert!(
            !sessions_dir(dir.dir()).exists()
                || fs::read_dir(sessions_dir(dir.dir())).unwrap().next().is_none()
        );
        // The ledger directory itself is untouched.
        assert_eq!(fs::read_dir(dir.dir()).unwrap().count(), 0);
    }

    #[test]
    fn session_paths_are_recognised_by_their_directory_component() {
        assert!(is_session_path(Path::new(".ledger/sessions/x.md")));
        assert!(is_session_path(Path::new("assets/.ledger/x")));
        assert!(is_session_path(Path::new("/home/me/books/.ledger")));
        assert!(is_session_path(Path::new(".ledger")));
        assert!(!is_session_path(Path::new("books/.ledgernote.md")));
        assert!(!is_session_path(Path::new("books/.ledgers/x.md")));
        assert!(!is_session_path(Path::new("books/ledger.md")));
        assert!(!is_session_path(Path::new("")));
    }

    #[test]
    fn rename_changes_the_title_and_delete_removes_the_file() {
        let dir = TempDir::new("conversations-rename");
        let id = new_id();
        append(dir.dir(), &id, &message(Role::User, "Old title")).unwrap();
        append(dir.dir(), &id, &message(Role::Assistant, "Kept")).unwrap();
        let path = path_of(&dir, &id);
        let before = fs::read_to_string(&path).unwrap();

        let renamed = rename(dir.dir(), &id, "A better title").unwrap();
        assert_eq!(renamed.title, "A better title");
        assert_eq!(renamed.path, path.display().to_string());
        assert_eq!(renamed.messages, 2);
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("title: A better title\n"));
        // The messages themselves were copied across untouched.
        assert!(after.ends_with("## assistant\n\nKept\n"));
        assert_ne!(before, after);

        delete(dir.dir(), &id).unwrap();
        assert!(!path.exists());
        assert!(load(dir.dir(), &id).is_err());
        assert!(list(dir.dir()).unwrap().sessions.is_empty());
        assert!(delete(dir.dir(), &id).is_err());
    }

    #[test]
    fn a_tool_payload_and_a_body_that_looks_like_a_heading_round_trip() {
        let dir = TempDir::new("conversations-tool");
        let id = new_id();
        let body = "Here is the query:\n\n## user\n\nnot a message\n\\## assistant\n\nand an escaped one";
        append(dir.dir(), &id, &message(Role::Assistant, body)).unwrap();
        let mut tool = message(Role::Tool, "register flows");
        tool.data = Some(serde_json::json!({ "rows": 3, "accounts": ["Expenses:Food"] }));
        append(dir.dir(), &id, &tool).unwrap();

        let loaded = load(dir.dir(), &id).unwrap();
        assert_eq!(loaded.messages[0].text, body);
        assert_eq!(loaded.messages[1].text, "register flows");
        assert_eq!(
            loaded.messages[1].data,
            Some(serde_json::json!({ "rows": 3, "accounts": ["Expenses:Food"] }))
        );
        assert_eq!(loaded.messages[0].data, None);

        // A tool message with nothing but a payload keeps an empty text.
        let mut payload_only = message(Role::Tool, "");
        payload_only.data = Some(serde_json::json!([1, 2]));
        append(dir.dir(), &id, &payload_only).unwrap();
        let loaded = load(dir.dir(), &id).unwrap();
        assert_eq!(loaded.messages[2].text, "");
        assert_eq!(loaded.messages[2].data, Some(serde_json::json!([1, 2])));
    }

    #[test]
    fn a_conversation_created_with_metadata_loads_it_back() {
        let dir = TempDir::new("conversations-create");
        let id = new_id();
        let tags = ["taxes".to_string(), " 2026 ".to_string()];
        let summary = create(dir.dir(), &id, " Tax   questions ", "gpt-5", &tags).unwrap();
        assert_eq!(summary.title, "Tax questions");
        assert_eq!(summary.messages, 0);

        let loaded = load(dir.dir(), &id).unwrap();
        assert_eq!(loaded.model, "gpt-5");
        assert_eq!(loaded.tags, ["taxes", "2026"]);
        assert!(loaded.messages.is_empty());
        assert!(fs::read_to_string(&loaded.path)
            .unwrap()
            .contains("tags: [taxes, 2026]\n"));
        assert!(create(dir.dir(), &id, "Again", "", &[]).is_err());

        append(dir.dir(), &id, &message(Role::User, "Hello")).unwrap();
        let listing = list(dir.dir()).unwrap();
        assert_eq!(listing.sessions[0].messages, 1);
        assert_eq!(listing.sessions[0].title, "Tax questions");
    }

    #[test]
    fn the_listing_is_newest_updated_first() {
        let dir = TempDir::new("conversations-listing");
        let sessions = sessions_dir(dir.dir());
        fs::create_dir_all(&sessions).unwrap();
        let file = |id: &str, updated: &str| {
            format!(
                "---\nid: {id}\ntitle: {id}\ncreated: 2026-01-01T00:00:00Z\nupdated: {updated}\nmodel:\n---\n\n## user\n\nhi\n"
            )
        };
        fs::write(sessions.join("a.md"), file("older", "2026-01-02T00:00:00Z")).unwrap();
        fs::write(sessions.join("b.md"), file("newer", "2026-03-02T00:00:00Z")).unwrap();
        // Not a conversation: a temp file mid-write, and a stray file.
        fs::write(sessions.join(".b.md.1.2.tmp"), "---\n").unwrap();
        fs::write(sessions.join("notes.txt"), "hello").unwrap();

        let listing = list(dir.dir()).unwrap();
        assert_eq!(
            listing
                .sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
        assert!(listing.errors.is_empty());
        assert_eq!(load(dir.dir(), "older").unwrap().messages.len(), 1);

        // A duplicate id — two copies of one conversation after a merge — is
        // listed once, and the copy that lost is reported.
        fs::write(
            sessions.join("c.md"),
            file("older", "2026-05-02T00:00:00Z"),
        )
        .unwrap();
        let listing = list(dir.dir()).unwrap();
        assert_eq!(listing.sessions.len(), 2);
        assert_eq!(listing.errors.len(), 1);
        assert!(listing.errors[0].error.contains("duplicate id `older`"));
        assert_eq!(
            load(dir.dir(), "older").unwrap().updated,
            "2026-05-02T00:00:00Z"
        );
    }

    #[test]
    fn a_ledger_nobody_talked_to_lists_nothing() {
        let dir = TempDir::new("conversations-empty");
        let listing = list(dir.dir()).unwrap();
        assert!(listing.sessions.is_empty());
        assert!(listing.errors.is_empty());
    }

    #[test]
    fn heading_escapes_round_trip_through_the_writer() {
        for line in [
            "## user",
            "## assistant",
            "## tool",
            "\\## user",
            "\\\\## assistant",
            "\\plain",
            " ## user",
            "## Notes",
            "",
        ] {
            let written = escape(line);
            if heading_role(line.trim_start_matches('\\')).is_some() {
                assert_ne!(written, line, "{line:?} should have been escaped");
            }
            assert_eq!(unescape(&written), line, "{line:?} did not survive");
        }
    }

    #[test]
    fn timestamps_are_rfc_3339_in_utc_and_fixed_width() {
        assert_eq!(timestamp_of(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp_of(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(timestamp_of(1_709_164_800), "2024-02-29T00:00:00Z");
        for seconds in [0, 1_000_000_000, 1_700_000_000, 4_102_444_800] {
            let stamp = timestamp_of(seconds);
            assert_eq!(stamp.len(), 20, "{stamp}");
            assert!(is_timestamp(&stamp), "{stamp}");
        }
        assert!(!is_timestamp("2026-10-29"));
        assert!(!is_timestamp("2026-10-29T09:12:03"));
        assert!(!is_timestamp("not a timestamp at all"));
        assert!(is_timestamp("2026-10-29T09:12:03.123+02:00"));
        assert!(!is_timestamp("2026-10-29T09:12:03.abcZ"));
    }

    #[test]
    fn the_file_name_is_ascii_and_within_the_slug_limit() {
        assert_eq!(slug_of("Budget — March review!"), "budget-march-review");
        assert_eq!(slug_of("   "), "conversation");
        assert_eq!(slug_of("Überweisung"), "berweisung");
        let long = "word ".repeat(40);
        assert!(slug_of(&long).len() <= SLUG_LIMIT);
        assert!(slug_of(&long).is_ascii());
        assert!(slug_of(&"a".repeat(80)).len() == SLUG_LIMIT);

        let dir = TempDir::new("conversations-slug");
        let id = new_id();
        let summary = append(dir.dir(), &id, &message(Role::User, &long)).unwrap();
        let name = Path::new(&summary.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_string();
        let stem = name.strip_suffix(".md").unwrap();
        let (head, tail) = stem.rsplit_once('-').unwrap();
        assert_eq!(tail, id);
        let slug = head.strip_prefix(&format!("{}-", &summary.created[..10])).unwrap();
        assert!(slug.len() <= SLUG_LIMIT, "{slug}");
        assert!(slug.is_ascii(), "{slug}");
    }

    #[test]
    fn a_title_becomes_one_line_and_a_long_one_is_cut() {
        assert_eq!(flatten_title("a\n b\tc  d "), "a b c d");
        let long = "x".repeat(TITLE_LIMIT + 10);
        let title = title_from(&message(Role::User, &long));
        assert_eq!(title.chars().count(), TITLE_LIMIT + 1);
        assert!(title.ends_with('…'));
        assert_eq!(
            title_from(&message(Role::Assistant, "## Notes\n\nsecond line")),
            "Notes"
        );
        assert_eq!(title_from(&message(Role::User, "   ")), "");
        assert_eq!(title_from(&message(Role::User, "One line\nsecond")), "One line");
    }
}
