// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The dashboard definitions: which widgets a page holds, in what order and how
//! wide — the one thing about the dashboard screen that is not derived from the
//! ledger.
//!
//! The definitions live in the app's config directory as `dashboards.json`,
//! versioned so a future shape can migrate instead of guessing, and written
//! through a temporary file so an interrupted write leaves the previous file
//! whole rather than a truncated one. Nothing here reads the ledger: a dashboard
//! only *names* widget kinds and carries their settings, and the kinds are the
//! frontend registry's keys.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file the definitions live in, inside the config directory the caller
/// names (the app's own config directory, not a ledger's).
const STORE_FILE: &str = "dashboards.json";

/// The shape this module writes. A file with another version is refused rather
/// than read as if it were this one.
const STORE_VERSION: u32 = 1;

/// The stem of the temporary file a write goes through. The process id and a
/// counter keep two concurrent writes — the app and a test binary, say — off
/// each other's file.
const TEMP_STEM: &str = "dashboards.json.tmp";

/// One saved dashboard page.
///
/// The id is the page's route segment and its identity for
/// [`save`]/[`delete`]: stable once written, so a rename never leaves the old
/// page behind under a new id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Dashboard {
    pub id: String,
    /// The label the list and the page title show.
    pub name: String,
    /// The page's widgets, in the order they were laid out. A page with no
    /// widgets is valid — an empty dashboard is one the user has not filled in
    /// yet, not a corrupt one.
    pub widgets: Vec<DashboardWidget>,
}

/// One widget on a dashboard page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct DashboardWidget {
    /// Unique within its dashboard; the frontend's key for the widget and what
    /// a layout edit reorders.
    pub id: String,
    /// The widget kind, a key in the frontend's registry (e.g. `net-worth`).
    /// Deliberately a string rather than an enum: the store's job is to keep
    /// what the frontend wrote, and a new widget must not need a backend
    /// change.
    pub kind: String,
    /// The widget's own settings, a JSON value the backend never interprets —
    /// which accounts to show, comparison windows, and so on. Defaults are the
    /// widget's business.
    #[specta(type = specta_typescript::Unknown)]
    pub config: serde_json::Value,
    /// How many columns of the page's grid the widget takes. The grid is four
    /// columns wide, so only 1, 2 and 4 fill a row evenly.
    pub span: u8,
    /// Position in the page's reading order, ascending.
    pub order: i32,
}

/// The file as it lies on disk: a version beside the definitions, so a version
/// this app does not know is refused by name instead of half-read.
#[derive(Debug, Serialize, Deserialize)]
struct Store {
    version: u32,
    dashboards: Vec<Dashboard>,
}

/// Where the definitions live under `dir`.
fn store_path(dir: &Path) -> PathBuf {
    dir.join(STORE_FILE)
}

/// The dashboards the store holds, oldest first.
///
/// A missing file is the first run, and answers [`default_dashboards`] rather
/// than an empty list: the screen opens on something usable, and the first
/// [`save`] materializes the file with the rest of the defaults intact. Once
/// the file exists — including when it holds none — it is what the caller gets.
///
/// A file that cannot be parsed or names another version is an error: the
/// definitions are the user's, so a broken file is reported to be looked at,
/// never quietly replaced with defaults.
pub fn load(dir: &Path) -> Result<Vec<Dashboard>, String> {
    match read_store(dir)? {
        Some(store) => Ok(store.dashboards),
        None => Ok(default_dashboards()),
    }
}

/// The default set [`load`] hands a first run: one `overview` page holding the
/// six default widgets. Nothing is written here, so a caller that only looks
/// never creates the file; the first [`save`] is what persists it.
///
/// The widgets' ids are the kinds they name — the identity the frontend keys a
/// widget by, and the thing a saved layout can be matched to the registry by.

/// Write `dashboard` into the store, adding it or replacing the one with the
/// same id.
///
/// The store is read first, so saving one dashboard keeps the others — and a
/// file that cannot be read fails the save rather than being overwritten with
/// this one dashboard. The write itself is atomic (see [`write_store`]).
pub fn save(dir: &Path, dashboard: Dashboard) -> Result<(), String> {
    validate(&dashboard)?;
    // One read-modify-write of the whole store, serialised across the app's
    // processes the way the index store is: a second instance saving a different
    // dashboard must not publish a snapshot that never saw this one.
    let _lock = crate::history::Lock::for_named_store(dir, "dashboards")?;
    let mut dashboards = load(dir)?;
    match dashboards
        .iter_mut()
        .find(|existing| existing.id == dashboard.id)
    {
        Some(existing) => *existing = dashboard,
        None => dashboards.push(dashboard),
    }
    write_store(dir, &dashboards)
}

/// Remove the dashboard `id` from the store, answering whether there was one.
///
/// Deleting a seeded dashboard persists the rest, so the deleted one does not
/// come back on the next load. An unknown id is not an error — a caller that
/// deletes twice gets `false` the second time.
pub fn delete(dir: &Path, id: &str) -> Result<bool, String> {
    let _lock = crate::history::Lock::for_named_store(dir, "dashboards")?;
    let mut dashboards = load(dir)?;
    let before = dashboards.len();
    dashboards.retain(|dashboard| dashboard.id != id);
    if dashboards.len() == before {
        return Ok(false);
    }
    write_store(dir, &dashboards)?;
    Ok(true)
}

/// The six widgets a first run lays out, on one `overview` page.
///
/// Spans follow the grid: the two headline figures sit side by side (2 each),
/// the two time series take a row of their own (4), and the share and the trend
/// close the page (2 each).
fn default_dashboards() -> Vec<Dashboard> {
    vec![Dashboard {
        id: "overview".to_string(),
        name: "Overview".to_string(),
        widgets: vec![
            widget("net-worth", "net-worth", 2, 0),
            widget("cash-flow", "cash-flow", 2, 1),
            widget("spending-by-month", "spending-by-month", 4, 2),
            widget("budget-vs-actual", "budget-vs-actual", 4, 3),
            widget("category-share", "category-share", 2, 4),
            widget("lifestyle-inflation", "lifestyle-inflation", 2, 5),
        ],
    }]
}

/// A default widget: its kind as its id, no settings of its own (the widget's
/// defaults apply), and its place in the grid.
fn widget(id: &str, kind: &str, span: u8, order: i32) -> DashboardWidget {
    DashboardWidget {
        id: id.to_string(),
        kind: kind.to_string(),
        config: serde_json::json!({}),
        span,
        order,
    }
}

/// What a dashboard must satisfy to be stored or shown: an id to be saved by,
/// and widgets the grid can place.
fn validate(dashboard: &Dashboard) -> Result<(), String> {
    if dashboard.id.trim().is_empty() {
        return Err("a dashboard needs an id".to_string());
    }
    for widget in &dashboard.widgets {
        if widget.id.trim().is_empty() {
            return Err(format!(
                "dashboard `{}` has a widget without an id",
                dashboard.id
            ));
        }
        if !matches!(widget.span, 1 | 2 | 4) {
            return Err(format!(
                "dashboard `{}` widget `{}` has span {}, but a widget is 1, 2 or 4 columns wide",
                dashboard.id, widget.id, widget.span
            ));
        }
    }
    Ok(())
}

/// The file's contents, or `None` when there is no file yet.
///
/// Everything wrong with a present file — unreadable, unparseable, another
/// version, an invalid widget, a duplicated id — is an error, because the only
/// safe alternative would be to throw the user's definitions away.
fn read_store(dir: &Path) -> Result<Option<Store>, String> {
    let path = store_path(dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read `{}`: {error}", path.display())),
    };
    let store: Store = serde_json::from_str(&text)
        .map_err(|error| format!("`{}` is not a dashboards file: {error}", path.display()))?;
    if store.version != STORE_VERSION {
        return Err(format!(
            "`{}` is version {}, but this app reads version {STORE_VERSION}",
            path.display(),
            store.version
        ));
    }
    for (index, dashboard) in store.dashboards.iter().enumerate() {
        validate(dashboard)?;
        if store.dashboards[..index]
            .iter()
            .any(|other| other.id == dashboard.id)
        {
            return Err(format!(
                "`{}` lists dashboard `{}` twice",
                path.display(),
                dashboard.id
            ));
        }
    }
    Ok(Some(store))
}

/// Replace the store in one step: render it, write it to a temporary file
/// beside its destination, then rename that file over the destination.
///
/// A rename within a directory is atomic, so a reader (or a crash) sees either
/// the old file or the whole new one — never a half-written JSON document. The
/// temporary file is removed on any failure, so a failed write leaves no debris
/// and the previous file untouched.
fn write_store(dir: &Path, dashboards: &[Dashboard]) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create `{}`: {error}", dir.display()))?;
    let store = Store {
        version: STORE_VERSION,
        dashboards: dashboards.to_vec(),
    };
    let mut text = serde_json::to_string_pretty(&store)
        .map_err(|error| format!("cannot render the dashboards: {error}"))?;
    text.push('\n');
    let bytes = text.as_bytes();
    crate::fsutil::write_atomic(&store_path(dir), bytes, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    /// A dashboard with one widget, for the tests that only care about the
    /// store.
    fn dashboard(id: &str, name: &str) -> Dashboard {
        Dashboard {
            id: id.to_string(),
            name: name.to_string(),
            widgets: vec![widget("main", "net-worth", 4, 0)],
        }
    }

    /// What a dashboard answers, so a loaded one can be compared to what was
    /// saved without depending on field order in the file.
    fn stored(dir: &TempDir, id: &str) -> Option<Dashboard> {
        load(dir.dir())
            .unwrap()
            .into_iter()
            .find(|dashboard| dashboard.id == id)
    }

    /// The names in the config directory — where a stray temporary file would
    /// show up.
    fn names(dir: &TempDir) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir.dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A saved dashboard comes back as it went in — settings included — from a
    /// versioned file.
    #[test]
    fn a_dashboard_round_trips_through_the_file() {
        let dir = TempDir::new("dashboards-round-trip");
        let saved = Dashboard {
            id: "spending".to_string(),
            name: "Spending".to_string(),
            widgets: vec![DashboardWidget {
                id: "by-month".to_string(),
                kind: "spending-by-month".to_string(),
                config: serde_json::json!({ "currency": "TRY", "months": 12 }),
                span: 4,
                order: 0,
            }],
        };
        save(dir.dir(), saved.clone()).unwrap();
        assert_eq!(stored(&dir, "spending"), Some(saved));

        let text = fs::read_to_string(dir.file(STORE_FILE)).unwrap();
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(file["version"], 1);
        // The seeded page, then the one just saved.
        assert_eq!(file["dashboards"][0]["id"], "overview");
        assert_eq!(file["dashboards"][1]["id"], "spending");
    }

    /// Saving one dashboard keeps the others, and saving it again replaces it:
    /// the first save of a fresh install writes the seeded defaults with it,
    /// so they are not lost by editing a page.
    #[test]
    fn save_upserts_by_id_and_keeps_the_seeded_set() {
        let dir = TempDir::new("dashboards-upsert");
        save(dir.dir(), dashboard("spending", "Spending")).unwrap();
        let all = load(dir.dir()).unwrap();
        // The seeded page plus the saved one.
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|dashboard| dashboard.id == "overview"));

        save(dir.dir(), dashboard("spending", "Spending again")).unwrap();
        let all = load(dir.dir()).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(stored(&dir, "spending").unwrap().name, "Spending again");
    }

    /// Deleting is by id and lasts: a deleted seeded dashboard does not come
    /// back, and deleting what is not there is a plain "no".
    #[test]
    fn delete_removes_a_dashboard_for_good() {
        let dir = TempDir::new("dashboards-delete");
        save(dir.dir(), dashboard("spending", "Spending")).unwrap();

        assert!(delete(dir.dir(), "spending").unwrap());
        assert_eq!(stored(&dir, "spending"), None);
        assert!(!delete(dir.dir(), "spending").unwrap());

        // The seeded page deletes like any other, and staying deleted is the
        // point: the defaults are not re-seeded into a file that exists.
        assert!(delete(dir.dir(), "overview").unwrap());
        assert!(load(dir.dir()).unwrap().is_empty());

        // An unknown id leaves the file exactly as it was.
        let before = fs::read_to_string(dir.file(STORE_FILE)).unwrap();
        assert!(!delete(dir.dir(), "never-saved").unwrap());
        assert_eq!(fs::read_to_string(dir.file(STORE_FILE)).unwrap(), before);
    }

    /// A file that cannot be read is reported, and neither a load nor a save
    /// writes over it — the definitions are the user's to keep or fix.
    #[test]
    fn a_corrupt_file_is_reported_and_never_overwritten() {
        let dir = TempDir::new("dashboards-corrupt");
        let corrupt = "{ \"version\": 1, \"dashboards\": [ ";
        fs::write(dir.file(STORE_FILE), corrupt).unwrap();

        let error = load(dir.dir()).unwrap_err();
        assert!(error.contains(STORE_FILE), "{error}");
        let error = save(dir.dir(), dashboard("spending", "Spending")).unwrap_err();
        assert!(error.contains(STORE_FILE), "{error}");
        assert_eq!(fs::read_to_string(dir.file(STORE_FILE)).unwrap(), corrupt);

        // Another version is a different file, not a corrupt one — refused by
        // number so the message says what to do.
        fs::write(dir.file(STORE_FILE), "{\"version\": 7, \"dashboards\": []}").unwrap();
        let error = load(dir.dir()).unwrap_err();
        assert!(error.contains("version 7"), "{error}");

        // A definition the grid cannot place is refused too, rather than
        // silently dropped or rendered broken.
        fs::write(
            dir.file(STORE_FILE),
            "{\"version\": 1, \"dashboards\": [{\"id\": \"x\", \"name\": \"X\", \"widgets\": \
             [{\"id\": \"w\", \"kind\": \"net-worth\", \"config\": {}, \"span\": 3, \"order\": 0}]}]}",
        )
        .unwrap();
        let error = load(dir.dir()).unwrap_err();
        assert!(error.contains("span 3"), "{error}");
        assert!(save(dir.dir(), dashboard("spending", "Spending")).is_err());
    }

    /// A write goes through a temporary file and leaves only the store behind:
    /// no partial file, no debris.
    #[test]
    fn a_write_leaves_a_complete_file_and_no_temporary() {
        let dir = TempDir::new("dashboards-atomic");
        save(dir.dir(), dashboard("spending", "Spending")).unwrap();
        assert_eq!(names(&dir), [STORE_FILE]);

        // What was written is one whole JSON document, parseable as the store
        // it claims to be.
        let text = fs::read_to_string(dir.file(STORE_FILE)).unwrap();
        let file: Store = serde_json::from_str(&text).unwrap();
        assert_eq!(file.version, STORE_VERSION);
        // The seeded page and the saved one.
        assert_eq!(file.dashboards.len(), 2);

        delete(dir.dir(), "spending").unwrap();
        assert_eq!(names(&dir), [STORE_FILE]);
    }

    /// The first run's set: one page holding the six default widgets, each with
    /// a stable id, a kind the frontend knows, a span the grid can place, and
    /// no settings of its own.
    #[test]
    fn seed_is_the_six_default_widgets() {
        let dir = TempDir::new("dashboards-seed");
        let seeded = load(dir.dir()).unwrap();
        assert_eq!(seeded.len(), 1);
        let overview = &seeded[0];
        assert_eq!(overview.id, "overview");
        assert_eq!(overview.name, "Overview");

        let kinds: Vec<&str> = overview
            .widgets
            .iter()
            .map(|widget| widget.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            [
                "net-worth",
                "cash-flow",
                "spending-by-month",
                "budget-vs-actual",
                "category-share",
                "lifestyle-inflation",
            ]
        );
        for (order, widget) in overview.widgets.iter().enumerate() {
            assert_eq!(
                widget.id, widget.kind,
                "a default widget is named by its kind"
            );
            assert_eq!(widget.order, order as i32);
            assert!(
                matches!(widget.span, 1 | 2 | 4),
                "`{}` has span {}",
                widget.id,
                widget.span
            );
            assert_eq!(widget.config, serde_json::json!({}));
        }

        // Seeding writes nothing, and the seeded page is storable as it stands:
        // a save keeps the six widgets intact.
        assert!(names(&dir).is_empty());
        save(dir.dir(), overview.clone()).unwrap();
        assert_eq!(load(dir.dir()).unwrap(), seeded);

        // Once the file exists, its dashboards win over the defaults.
        save(dir.dir(), dashboard("spending", "Spending")).unwrap();
        assert_eq!(load(dir.dir()).unwrap(), load(dir.dir()).unwrap());
    }
}
