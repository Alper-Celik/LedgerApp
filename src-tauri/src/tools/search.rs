// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finding things in the ledger directory: `grep` for a regular expression in
//! file contents, `glob` for paths.
//!
//! Both walk through the same [`ignore`] walker, so `.gitignore`, `.ignore`
//! and `.git/info/exclude` hide what they hide in every other tool of the
//! project, hidden entries stay hidden, and `.git` itself is never walked.
//! Both stop at a cap and say so, and both report paths relative to the root
//! the caller passed.

use std::fs;
use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::Serialize;

use crate::tools::paths::display_relative;

/// Files larger than this are not read: a ledger directory can hold an
/// attachment that is not text at all.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// How much of a matched line is echoed back.
pub const MAX_LINE_CHARS: usize = 400;
/// How many matches `grep` returns when it is not told otherwise.
pub const DEFAULT_GREP_RESULTS: usize = 100;
/// The most matches `grep` returns.
pub const MAX_GREP_RESULTS: usize = 1_000;
/// How many paths `glob` returns when it is not told otherwise.
pub const DEFAULT_GLOB_RESULTS: usize = 200;
/// The most paths `glob` returns.
pub const MAX_GLOB_RESULTS: usize = 2_000;

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrepHit {
    /// Relative to the walk's root, `/`-separated.
    pub path: String,
    /// 1-based.
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrepOutcome {
    pub matches: Vec<GrepHit>,
    /// The cap was reached; more matches exist.
    pub truncated: bool,
    /// How many files were read before the walk stopped.
    pub files_searched: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GlobOutcome {
    pub paths: Vec<String>,
    /// The cap was reached; more paths match.
    pub truncated: bool,
}

/// The lines of `target` (a file or a directory under `root`) matching
/// `pattern`.
///
/// `filter` is a glob on the relative path (`*.bean`, `**/*.bean`); when it
/// has no `/` it is also matched against the bare file name, so `*.bean` finds
/// the ledger files at any depth.
pub fn grep(
    root: &Path,
    target: &Path,
    pattern: &str,
    filter: Option<&str>,
    case_sensitive: bool,
    max_results: usize,
) -> Result<GrepOutcome, String> {
    if pattern.is_empty() {
        return Err("the pattern is empty".to_string());
    }
    if max_results == 0 {
        return Err("max_results must be at least 1".to_string());
    }
    let max_results = max_results.min(MAX_GREP_RESULTS);
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| format!("invalid regular expression `{pattern}`: {error}"))?;
    let filter = filter.map(PathFilter::new).transpose()?;

    let files = walk_files(target)?;
    let mut matches = Vec::new();
    let mut files_searched = 0;
    let mut truncated = false;

    'files: for file in files {
        let relative = display_relative(root, &file);
        let name = file_name(&file);
        if !filter.as_ref().is_none_or(|filter| filter.accepts(&relative, &name)) {
            continue;
        }
        // Held line by line: a ledger is text, anything else is skipped.
        let Some(text) = read_text(&file) else {
            continue;
        };
        files_searched += 1;
        for (index, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            if matches.len() == max_results {
                truncated = true;
                break 'files;
            }
            matches.push(GrepHit {
                path: relative.clone(),
                line: index + 1,
                text: clip(line),
            });
        }
    }

    matches.sort_by(|left, right| {
        (&left.path, left.line).cmp(&(&right.path, right.line))
    });
    Ok(GrepOutcome {
        matches,
        truncated,
        files_searched,
    })
}

/// The paths under `target` whose relative path matches the glob `pattern`,
/// sorted.
///
/// A pattern without a `/` also matches the bare file name, so `*.bean` means
/// "every `.bean` at any depth" while `ledgers/*.bean` means exactly one
/// directory down.
pub fn glob_paths(
    root: &Path,
    target: &Path,
    pattern: &str,
    max_results: usize,
) -> Result<GlobOutcome, String> {
    if max_results == 0 {
        return Err("max_results must be at least 1".to_string());
    }
    let max_results = max_results.min(MAX_GLOB_RESULTS);
    let matcher = compile(pattern)?;
    let by_name = (!pattern.contains('/')).then(|| compile(pattern)).transpose()?;

    let mut paths = Vec::new();
    let mut truncated = false;
    for file in walk_files(target)? {
        let relative = display_relative(root, &file);
        let name = file_name(&file);
        let matched = matcher.is_match(Path::new(&relative))
            || by_name
                .as_ref()
                .is_some_and(|matcher| matcher.is_match(&name));
        if !matched {
            continue;
        }
        if paths.len() == max_results {
            truncated = true;
            break;
        }
        paths.push(relative);
    }
    paths.sort();
    Ok(GlobOutcome { paths, truncated })
}

/// A compiled glob on relative paths.
struct PathFilter {
    matcher: GlobMatcher,
    by_name: Option<GlobMatcher>,
}

impl PathFilter {
    fn new(pattern: &str) -> Result<Self, String> {
        Ok(Self {
            matcher: compile(pattern)?,
            by_name: (!pattern.contains('/')).then(|| compile(pattern)).transpose()?,
        })
    }

    fn accepts(&self, relative: &str, name: &str) -> bool {
        self.matcher.is_match(Path::new(relative))
            || self
                .by_name
                .as_ref()
                .is_some_and(|matcher| matcher.is_match(name))
    }
}

fn compile(pattern: &str) -> Result<GlobMatcher, String> {
    GlobBuilder::new(pattern)
        // As in a shell: `*` stays inside one path component, `**` crosses
        // directories.
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| format!("invalid glob `{pattern}`: {error}"))
}

/// The text of `path`, unless it is not text at all.
fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() {
        return Some(String::new());
    }
    // A NUL early in the file is the usual signature of something that is not
    // text; the UTF-8 check below then rules out the rest.
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Every file under `target`, in the order the walker found them, with the
/// ignore files honoured.
fn walk_files(target: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut builder = WalkBuilder::new(target);
    builder
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        // A ledger directory is often a repository, but it need not be: the
        // ignore files apply either way.
        .require_git(false)
        .follow_links(false)
        .max_filesize(Some(MAX_FILE_BYTES));

    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = entry.map_err(|error| format!("cannot walk {}: {error}", target.display()))?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A matched line, clipped to something a model can read in a result.
fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let mut clipped: String = line.chars().take(MAX_LINE_CHARS).collect();
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{DEFAULT_GLOB_RESULTS, DEFAULT_GREP_RESULTS, glob_paths, grep};

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let write = |name: &str, text: &str| {
                let path = dir.path().join(name);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(path, text).unwrap();
            };
            write("main.bean", "2026-01-01 open Assets:Bank\n2026-01-02 open Assets:Cash\n");
            write("notes.md", "# Assets\n");
            write("ignored.bean", "2026-01-03 open Assets:Ignored\n");
            write(".gitignore", "ignored.bean\n");
            write(".hidden.bean", "2026-01-04 open Assets:Hidden\n");
            write("sub/also.bean", "2026-01-05 open Assets:Deeper\n");
            write("sub/ignored-dir/x.bean", "2026-01-06 open Assets:Dir\n");
            write("sub/ignored-dir/.gitignore", "*.bean\n");
            write("binary.bean", "");
            fs::write(dir.path().join("binary.bean"), [0u8, 1, 2, 3]).unwrap();
            Self { dir }
        }

        fn root(&self) -> &std::path::Path {
            self.dir.path()
        }
    }

    #[test]
    fn grep_finds_lines_and_respects_ignore_files() {
        let fixture = Fixture::new();
        let found = grep(fixture.root(), fixture.root(), "Assets", None, false, 100).unwrap();
        let paths: Vec<&str> = found.matches.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(paths, ["main.bean", "main.bean", "notes.md", "sub/also.bean"]);
        assert_eq!(found.matches[0].line, 1);
        assert!(found.matches[0].text.contains("open Assets:Bank"));
        assert!(!found.truncated);
        assert!(!paths.contains(&"ignored.bean"));
        assert!(!paths.contains(&".hidden.bean"));
        assert!(!paths.contains(&"sub/ignored-dir/x.bean"));
        assert!(!paths.contains(&"binary.bean"));
    }

    #[test]
    fn grep_honours_the_case_toggle() {
        let fixture = Fixture::new();
        let insensitive = grep(fixture.root(), fixture.root(), "assets", None, false, 100).unwrap();
        assert!(!insensitive.matches.is_empty());
        let sensitive = grep(fixture.root(), fixture.root(), "assets", None, true, 100).unwrap();
        assert!(sensitive.matches.is_empty());
    }

    #[test]
    fn grep_filters_by_glob_and_path() {
        let fixture = Fixture::new();
        let beans = grep(fixture.root(), fixture.root(), "Assets", Some("*.bean"), false, 100).unwrap();
        let paths: Vec<&str> = beans.matches.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(paths, ["main.bean", "main.bean", "sub/also.bean"]);

        let sub = fixture.root().join("sub");
        let only_sub = grep(fixture.root(), &sub, "Assets", None, false, 100).unwrap();
        assert!(only_sub.matches.iter().all(|hit| hit.path.starts_with("sub/")));
        assert_eq!(only_sub.matches.len(), 1);
    }

    #[test]
    fn grep_caps_and_says_so() {
        let fixture = Fixture::new();
        let capped = grep(fixture.root(), fixture.root(), "Assets", None, false, 2).unwrap();
        assert_eq!(capped.matches.len(), 2);
        assert!(capped.truncated);

        let all = grep(fixture.root(), fixture.root(), "Assets", None, false, DEFAULT_GREP_RESULTS).unwrap();
        assert!(!all.truncated);
        assert_eq!(all.matches.len(), 4);
    }

    #[test]
    fn grep_clips_long_lines_and_rejects_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(super::MAX_LINE_CHARS + 50);
        fs::write(dir.path().join("long.bean"), format!("{long}\n")).unwrap();
        let found = grep(dir.path(), dir.path(), "x", None, false, 10).unwrap();
        assert_eq!(found.matches[0].text.chars().count(), super::MAX_LINE_CHARS + 1);
        assert!(found.matches[0].text.ends_with('…'));

        assert!(grep(dir.path(), dir.path(), "(", None, false, 10).unwrap_err().contains("invalid regular expression"));
        assert!(grep(dir.path(), dir.path(), "", None, false, 10).unwrap_err().contains("empty"));
        assert!(grep(dir.path(), dir.path(), "x", None, false, 0).unwrap_err().contains("at least 1"));
        assert!(grep(dir.path(), dir.path(), "x", Some("["), false, 10).unwrap_err().contains("invalid glob"));
    }

    #[test]
    fn glob_is_sorted_and_honours_ignore_files() {
        let fixture = Fixture::new();
        let found = glob_paths(fixture.root(), fixture.root(), "**/*.bean", 100).unwrap();
        assert_eq!(
            found.paths,
            ["binary.bean", "main.bean", "sub/also.bean"],
            "hidden and ignored files stay hidden"
        );
        assert!(!found.truncated);

        // A pattern with no `/` is matched against the file name too.
        let flat = glob_paths(fixture.root(), fixture.root(), "*.bean", 100).unwrap();
        assert_eq!(flat.paths, found.paths);

        // A path pattern stays where it points.
        let sub = glob_paths(fixture.root(), fixture.root(), "sub/*.bean", 100).unwrap();
        assert_eq!(sub.paths, ["sub/also.bean"]);

        let none = glob_paths(fixture.root(), fixture.root(), "*.csv", 100).unwrap();
        assert!(none.paths.is_empty());
    }

    #[test]
    fn glob_caps_and_says_so() {
        let fixture = Fixture::new();
        let capped = glob_paths(fixture.root(), fixture.root(), "**/*.bean", 1).unwrap();
        assert_eq!(capped.paths.len(), 1);
        assert!(capped.truncated);
        let wide = glob_paths(fixture.root(), fixture.root(), "**/*.bean", DEFAULT_GLOB_RESULTS).unwrap();
        assert!(!wide.truncated);
        assert!(glob_paths(fixture.root(), fixture.root(), "[", 10).unwrap_err().contains("invalid glob"));
    }
}
