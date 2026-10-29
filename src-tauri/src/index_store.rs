// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The index series a ledger has installed, and the CSV import that puts one
//! there.
//!
//! The store is one plain JSON file (`indexes.json`) inside the directory the
//! caller hands in — in the app that is the app's config directory, so every
//! ledger the app opens shares one set of series (they are measurements of a
//! currency, not of a journal). The file is readable and fixable by hand, and
//! [`IndexSeries::normalised`] is what makes a hand-edited file safe.
//!
//! Everything that enters the file goes through [`IndexSeries::normalised`], so
//! a hand-edited duplicate month, an unsorted export and a decimal written with
//! the wrong scale all come back in the one canonical shape the reports assume.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::index::{IndexPoint, IndexSeries, is_iso_date, is_year_month};

/// The file, inside the caller's directory, that holds every series.
const INDEX_FILE: &str = "indexes.json";

/// The publisher names that mark a series as unofficial. ENAG is an independent
/// research group rather than a statistical agency, and its series must be
/// labelled as such wherever it is shown.
fn source_is_unofficial(source: &str) -> bool {
    source
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case("enag"))
}

/// Whether `text` opens with a date, i.e. is a data row rather than the header
/// line a publisher's export usually starts with.
fn starts_with_date(text: &str) -> bool {
    is_iso_date(text) || is_year_month(text)
}

/// Parse the `date,value` rows the CSV import accepts.
///
/// The dialect is deliberately small: comma-separated, one row per month, the
/// date first and the index value second. Either `YYYY-MM` or `YYYY-MM-DD` is
/// accepted and both normalise to the first of the month; a leading header row,
/// blank lines and `#` comments are skipped. Values are decimal strings with a
/// dot — the CSV comma is the field separator, so a decimal comma cannot be
/// expressed here.
///
/// This is also the parser the CSV-export providers use, so a payload pasted
/// into the import and a payload fetched by a provider are read the same way.
pub(crate) fn points_from_csv(payload: &str) -> Result<Vec<IndexPoint>, String> {
    let payload = payload.strip_prefix('\u{feff}').unwrap_or(payload);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .comment(Some(b'#'))
        .from_reader(payload.as_bytes());

    let mut points = Vec::new();
    for (index, record) in reader.records().enumerate() {
        let row = index + 1;
        let record = record.map_err(|error| format!("CSV row {row}: {error}"))?;
        let date = record.get(0).unwrap_or("").trim();
        let value = record.get(1).unwrap_or("").trim();
        if date.is_empty() && value.is_empty() {
            continue;
        }
        // The first row may name the columns (`date,value`, `Tarih,Değer`);
        // every later non-date row is a real error.
        if points.is_empty() && !starts_with_date(date) {
            continue;
        }
        let point =
            IndexPoint::new(date, value).map_err(|error| format!("CSV row {row}: {error}"))?;
        points.push(point);
    }
    if points.is_empty() {
        return Err("the CSV holds no `date,value` rows".to_owned());
    }
    Ok(points)
}

/// The series installed for the ledger under `dir`, oldest-first and
/// id-sorted. A directory with no file is a ledger with no installed indexes,
/// not an error.
pub(crate) fn load_indexes(dir: &Path) -> Result<Vec<IndexSeries>, String> {
    let path = index_path(dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!("could not read {}: {error}", path.display()));
        }
    };
    let stored: Vec<IndexSeries> = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a list of index series: {error}", path.display()))?;
    let mut series: Vec<IndexSeries> = Vec::with_capacity(stored.len());
    for entry in stored {
        let entry = entry
            .normalised()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if series.iter().any(|known| known.id == entry.id) {
            return Err(format!(
                "{} holds two series with the id `{}`",
                path.display(),
                entry.id
            ));
        }
        series.push(entry);
    }
    series.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(series)
}

/// Install `series`, replacing any series with the same id.
///
/// The write is a rename over the existing file, so a crash mid-write leaves
/// the previous file intact rather than a truncated one.
/// The store's writers serialise on this: a save is read-modify-write of the
/// whole file, and two overlapping saves (a refresh on a worker thread, an import
/// from the UI) would otherwise each publish the state it read and drop the
/// other's series. Only this process writes the file, so a process-wide lock is
/// the whole contract.
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn save_series(dir: &Path, series: &IndexSeries) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let series = series.clone().normalised()?;
    let mut all = load_indexes(dir)?;
    match all.iter().position(|stored| stored.id == series.id) {
        Some(index) => all[index] = series,
        None => all.push(series),
    }
    all.sort_by(|a, b| a.id.cmp(&b.id));
    write_indexes(dir, &all)
}

/// Import a publisher's `date,value` CSV as the series `id`, replacing the
/// series already installed under that id, and return the stored series.
///
/// `source` is the publisher's name; a name that mentions ENAG marks the series
/// unofficial, because ENAG is an independent research group rather than a
/// statistical agency.
pub(crate) fn import_csv(
    dir: &Path,
    id: &str,
    label: &str,
    source: &str,
    currency: &str,
    csv_text: &str,
) -> Result<IndexSeries, String> {
    let points = points_from_csv(csv_text)?;
    let series = IndexSeries::new(
        id,
        label,
        source,
        source_is_unofficial(source),
        currency,
        points,
    )?;
    save_series(dir, &series)?;
    Ok(series)
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join(INDEX_FILE)
}

fn write_indexes(dir: &Path, series: &[IndexSeries]) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let path = index_path(dir);
    let text = serde_json::to_string_pretty(series)
        .map_err(|error| format!("could not encode the index series: {error}"))?;
    // A unique temp name: two writers (a refresh on a worker thread, a CSV
    // import from the UI) must not share one scratch file, or the rename can
    // publish a half-written store.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = dir.join(format!("{INDEX_FILE}.{}.{scratch}.tmp", std::process::id()));
    fs::write(&temporary, text)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("could not replace {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    const CSV: &str = "\u{feff}date,value\n2026-02,125.40\n2026-01-15,100.00\n";

    /// The import normalises months and stores a series that reloads exactly as
    /// it was returned — the store round-trip the frontend relies on.
    #[test]
    fn import_normalises_and_round_trips() {
        let dir = TempDir::new("index-store");
        let series = import_csv(dir.dir(), "tr-cpi", "Türkiye CPI", "TÜİK", "TRY", CSV).unwrap();

        assert_eq!(series.frequency, "monthly");
        assert!(!series.unofficial);
        assert_eq!(
            series
                .points
                .iter()
                .map(|point| (point.date.as_str(), point.value.as_str()))
                .collect::<Vec<_>>(),
            [("2026-01-01", "100.00"), ("2026-02-01", "125.40")]
        );

        let loaded = load_indexes(dir.dir()).unwrap();
        assert_eq!(loaded, vec![series]);
        assert!(dir.file(INDEX_FILE).is_file());
    }

    /// Importing the same id again replaces it; a second id is added beside it
    /// and the file stays id-sorted.
    #[test]
    fn saving_replaces_by_id_and_keeps_the_file_sorted() {
        let dir = TempDir::new("index-replace");
        import_csv(dir.dir(), "z-cpi", "Z CPI", "Somewhere", "USD", "2026-01,1").unwrap();
        import_csv(dir.dir(), "a-cpi", "A CPI", "Elsewhere", "TRY", "2026-01,2").unwrap();
        import_csv(dir.dir(), "z-cpi", "Z CPI", "Somewhere", "USD", "2026-01,3").unwrap();

        let loaded = load_indexes(dir.dir()).unwrap();
        assert_eq!(
            loaded
                .iter()
                .map(|series| (series.id.as_str(), series.points[0].value.as_str()))
                .collect::<Vec<_>>(),
            [("a-cpi", "2"), ("z-cpi", "3")]
        );
    }

    /// A source naming ENAG is marked unofficial; the official publishers are
    /// not.
    #[test]
    fn import_flags_enag_unofficial() {
        let dir = TempDir::new("index-unofficial");
        let series = import_csv(
            dir.dir(),
            "tr-enag",
            "ENAG inflation",
            "ENAG",
            "TRY",
            "2026-01,100",
        )
        .unwrap();
        assert!(series.unofficial);

        let official = import_csv(
            dir.dir(),
            "tr-cpi",
            "Türkiye CPI",
            "TÜİK",
            "TRY",
            "2026-01,100",
        )
        .unwrap();
        assert!(!official.unofficial);
    }

    /// A directory with no store is an empty list, and a malformed row names
    /// the CSV row it was on.
    #[test]
    fn missing_store_is_empty_and_bad_rows_are_named() {
        let dir = TempDir::new("index-empty");
        assert!(load_indexes(dir.dir()).unwrap().is_empty());

        let error = points_from_csv("date,value\n2026-01,100\n2026-02,not-a-number\n").unwrap_err();
        assert!(error.contains("CSV row 3"), "{error}");

        let error = points_from_csv("date,value\n").unwrap_err();
        assert!(error.contains("no `date,value` rows"), "{error}");
    }

    /// A stored series with a bad shape is refused on load rather than handed
    /// to the reports.
    #[test]
    fn a_broken_store_is_rejected_on_load() {
        let dir = TempDir::new("index-broken");
        fs::write(
            dir.file(INDEX_FILE),
            r#"[{"id":"x","label":"X","source":"S","unofficial":false,"frequency":"daily","currency":"USD","points":[{"date":"2026-01-01","value":"100"}]}]"#,
        )
        .unwrap();
        let error = load_indexes(dir.dir()).unwrap_err();
        assert!(error.contains("monthly"), "{error}");
    }
}
