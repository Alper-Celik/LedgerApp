// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Confinement: every path a tool is given is resolved against the one root
//! the caller passed, and a path that leaves that root — through `..`, an
//! absolute path, or a symbolic link — is refused before anything is opened.
//!
//! Refusal is lexical first (a `..` component is refused outright, so a
//! nonexistent `../x` cannot be created either), then physical: a path that
//! exists is canonicalized and must still sit under the canonical root, which
//! is what catches the symlink out of the ledger directory.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The directory a tool call is confined to: the ledger's own directory.
#[derive(Debug, Clone)]
pub(crate) struct Root {
    canonical: PathBuf,
}

impl Root {
    /// The canonical form of `path`, which must be an existing directory.
    ///
    /// A ledger opened as a bare file name (`main.bean`, the default) resolves to
    /// no directory at all; that means the process's working directory, which is
    /// where the loader looked for it in the first place.
    pub(crate) fn new(path: &Path) -> Result<Self, String> {
        let directory = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        let canonical = fs::canonicalize(directory).map_err(|error| {
            format!(
                "the ledger directory {} is not readable: {error}",
                directory.display()
            )
        })?;
        if !canonical.is_dir() {
            return Err(format!("{} is not a directory", canonical.display()));
        }
        Ok(Self { canonical })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.canonical
    }

    /// The path `raw` names, relative to the root — never outside it.
    ///
    /// A path that does not exist yet (a `write` to a new file) resolves
    /// against its canonical parent, so the file is still born inside the
    /// root and through no symbolic link.
    pub(crate) fn resolve(&self, raw: &str) -> Result<PathBuf, String> {
        let given = raw.trim();
        if given.is_empty() {
            return Err("path is empty".to_string());
        }
        let relative = Path::new(given);

        let joined = if relative.is_absolute() {
            if !relative.starts_with(&self.canonical) {
                return Err(format!(
                    "`{given}` is outside the ledger directory ({})",
                    self.canonical.display()
                ));
            }
            relative.to_path_buf()
        } else {
            for component in relative.components() {
                match component {
                    Component::ParentDir => {
                        return Err(format!(
                            "`{given}` is outside the ledger directory (`..` is not allowed)"
                        ));
                    }
                    Component::Normal(_) | Component::CurDir => {}
                    _ => {
                        return Err(format!(
                            "`{given}` is not a path inside the ledger directory"
                        ));
                    }
                }
            }
            self.canonical.join(relative)
        };

        match fs::symlink_metadata(&joined) {
            // Whatever is there — a file, a directory, even a dangling link —
            // is followed to its physical path and re-checked.
            Ok(_) => {
                let physical = fs::canonicalize(&joined)
                    .map_err(|error| format!("cannot resolve `{given}`: {error}"))?;
                if !physical.starts_with(&self.canonical) {
                    return Err(format!(
                        "`{given}` is outside the ledger directory (it resolves to {})",
                        physical.display()
                    ));
                }
                Ok(physical)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let parent = joined
                    .parent()
                    .ok_or_else(|| format!("`{given}` does not name a file"))?;
                let canonical_parent = fs::canonicalize(parent).map_err(|_| {
                    format!(
                        "no such directory: {}",
                        display_relative(&self.canonical, parent)
                    )
                })?;
                if !canonical_parent.starts_with(&self.canonical) {
                    return Err(format!(
                        "`{given}` is outside the ledger directory (it resolves to {})",
                        canonical_parent.display()
                    ));
                }
                let name = joined
                    .file_name()
                    .ok_or_else(|| format!("`{given}` does not name a file"))?;
                Ok(canonical_parent.join(name))
            }
            Err(error) => Err(format!("cannot resolve `{given}`: {error}")),
        }
    }
}

/// `path` as the tools report it: relative to the root, `/`-separated.
pub(crate) fn display_relative(root: &Path, path: &Path) -> String {
    let stripped = path.strip_prefix(root).unwrap_or(path);
    let text = stripped.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::testing::TempDir;

    use super::{Root, display_relative};

    fn scratch() -> TempDir {
        TempDir::new("tool-paths")
    }

    #[test]
    fn refuses_parent_components() {
        let dir = scratch();
        let root = Root::new(dir.dir()).unwrap();
        let error = root.resolve("../secret.bean").unwrap_err();
        assert!(error.contains("outside the ledger directory"), "{error}");
        let error = root.resolve("sub/../../secret.bean").unwrap_err();
        assert!(error.contains("outside the ledger directory"), "{error}");
    }

    #[test]
    fn refuses_absolute_paths_outside() {
        let dir = scratch();
        let root = Root::new(dir.dir()).unwrap();
        let outside = std::env::temp_dir().join("ledger-app-outside.bean");
        let error = root.resolve(outside.to_str().unwrap()).unwrap_err();
        assert!(error.contains("outside the ledger directory"), "{error}");
    }

    #[test]
    fn accepts_paths_inside() {
        let dir = scratch();
        dir.seed("main.bean", "");
        fs::create_dir(dir.dir().join("sub")).unwrap();
        let root = Root::new(dir.dir()).unwrap();
        let inside = root.resolve("main.bean").unwrap();
        assert!(inside.starts_with(&root.path()));
        // A path that does not exist yet resolves through its parent.
        let new = root.resolve("sub/new.bean").unwrap();
        assert_eq!(display_relative(root.path(), &new), "sub/new.bean");
        assert!(new.starts_with(root.path()));
        // An absolute path inside the root is accepted too.
        assert_eq!(root.resolve(inside.to_str().unwrap()).unwrap(), inside);
    }

    #[test]
    fn refuses_a_missing_parent() {
        let dir = scratch();
        let root = Root::new(dir.dir()).unwrap();
        let error = root.resolve("nope/new.bean").unwrap_err();
        assert!(error.contains("no such directory"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_out_of_the_root() {
        let inside = scratch();
        let outside = scratch();
        outside.seed("secret.bean", "secret\n");
        std::os::unix::fs::symlink(
            outside.dir().join("secret.bean"),
            inside.dir().join("link.bean"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            &outside.dir().join("missing.bean"),
            inside.dir().join("dangling.bean"),
        )
        .unwrap();

        let root = Root::new(inside.dir()).unwrap();
        let error = root.resolve("link.bean").unwrap_err();
        assert!(error.contains("outside the ledger directory"), "{error}");
        // A dangling link has no physical target to check, so it is refused
        // rather than followed into whatever it names.
        let error = root.resolve("dangling.bean").unwrap_err();
        assert!(error.contains("cannot resolve"), "{error}");
    }

    #[test]
    fn displays_relative_paths() {
        let dir = scratch();
        let root = Root::new(dir.dir()).unwrap();
        assert_eq!(
            display_relative(root.path(), &root.path().join("a/b.bean")),
            "a/b.bean"
        );
        assert_eq!(display_relative(root.path(), root.path()), ".");
    }
}
