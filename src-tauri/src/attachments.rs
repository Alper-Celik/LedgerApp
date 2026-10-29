// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The user's own files as conversational material: what a file is, the pages
//! a PDF rasterises to, and the rows a spreadsheet or CSV holds.
//!
//! Nothing here trusts a file's size. An invoice can be a 4 GiB scan and a
//! "small" xlsx can be a million rows, so every entry point has a ceiling and
//! says so in the error: a byte ceiling for [`describe`], [`read_table`] and
//! [`read_text`], plus pages and a pixel budget for [`render_pdf`] and rows and
//! columns for [`read_table`]. The PDF budget is spent by downscaling, not
//! refusing — a tall page comes back smaller rather than absent — and only
//! drops the pages past its end.
//!
//! PDFs are rasterised, never text-extracted: the reader is a model with eyes,
//! and MuPDF reorders a scanned page's glyphs no better than the model reads
//! the picture. Spreadsheet cells come back as strings, never as JSON numbers,
//! so nothing is re-rounded on the way to a model — the digits are calamine's,
//! which carries no number format to reproduce a `12.50` as the sheet showed it.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use calamine::{Data, DataType, Reader};

/// What a file is, as far as the panel cares. Decided by extension, and by the
/// first bytes when the extension says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Pdf,
    Image,
    Spreadsheet,
    Csv,
    Text,
    Other,
}

/// A file as the panel lists it before deciding how to use it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct AttachmentInfo {
    pub path: String,
    pub kind: AttachmentKind,
    /// Size on disk at the moment of describing, in bytes. `u32` because the
    /// attachment ceiling is 64 MiB and specta refuses to export 64-bit integers
    /// (a JS number cannot carry one).
    pub bytes: u32,
    /// Pages of a PDF; `None` for everything else — and for a PDF on a build
    /// without MuPDF (Android), where the count would need the renderer.
    pub pages: Option<u32>,
    /// Sheet names of a spreadsheet, in workbook order; empty otherwise.
    pub sheets: Vec<String>,
}

/// One rasterised PDF page. The PNG stays in memory: at 150 dpi an A4 page is
/// 1240 × 1754 and a handful of megabytes at worst, so writing it to disk first
/// would only add temporary files to clean up.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PdfPage {
    /// Zero-based page number in the document.
    pub index: u32,
    pub width: u32,
    pub height: u32,
    /// The page as PNG bytes, 8-bit RGB.
    pub png: Vec<u8>,
}

/// How finely and how much of a PDF to rasterise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, specta::Type)]
#[serde(default)]
pub struct RenderOptions {
    /// Resolution in dots per inch; text is legible from 120 up.
    pub dpi: u32,
    /// Pages to render at most, from the front of the document.
    pub max_pages: u32,
    /// Total pixels across every returned page. A page that would cross the
    /// line is rendered at a smaller dpi instead; pages once it is spent are
    /// dropped.
    pub max_pixels: u64,
}

impl Default for RenderOptions {
    fn default() -> Self {
        // 150 dpi is what a scan reads at; 20 pages keeps a long report within
        // one request. 64 megapixels leaves twenty A4 pages at full size and
        // still caps a wall of posters.
        Self {
            dpi: 150,
            max_pages: 20,
            max_pixels: 64_000_000,
        }
    }
}

/// How much of a sheet or CSV to hand over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, specta::Type)]
#[serde(default)]
pub struct TableOptions {
    pub max_rows: u32,
    pub max_columns: u32,
}

impl Default for TableOptions {
    fn default() -> Self {
        // Wider than the widest invoice anyone reads, narrower than a sheet
        // that would drown the prompt.
        Self {
            max_rows: 5_000,
            max_columns: 256,
        }
    }
}

/// A sheet or CSV as rows of strings. Rows may be shorter than `headers` when
/// the file's own rows are ragged; that raggedness is the file's, and is kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// True when rows or columns were dropped to fit `TableOptions`.
    pub truncated: bool,
}

/// The largest file [`describe`] and [`read_table`] will look at. Bigger files
/// are refused by name and number rather than read into memory: a ledger's
/// attachments are documents, not data dumps.
const MAX_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;

/// The highest dpi [`render_pdf`] honours. Past this, a page is upscaled
/// without gaining detail and the pixel budget is spent for nothing.
///
/// Unused on Android, where the rasteriser is absent; its geometry helper and
/// its test stay compiled so the arithmetic is covered on every target.
#[cfg_attr(target_os = "android", allow(dead_code))]
const MAX_DPI: u32 = 600;

/// The lowest dpi a downscaled page is worth returning at: below this the page
/// is a thumbnail, and the budget that would buy it ends the render instead.
const MIN_DPI: u32 = 24;

/// What a file is, and how big: the panel's first look, before it decides
/// which reader to use.
///
/// Refuses a file it cannot stat, one that is not a regular file, and one over
/// the byte ceiling above — naming the limit, so the panel can say why.
pub fn describe(path: &Path) -> Result<AttachmentInfo, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let bytes = metadata.len();
    if bytes > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "{} is {bytes} bytes, over the {MAX_ATTACHMENT_BYTES}-byte attachment limit",
            path.display()
        ));
    }
    let kind = kind_of(path);
    let (pages, sheets) = match kind {
        AttachmentKind::Pdf => (pdf_page_count(path)?, Vec::new()),
        AttachmentKind::Spreadsheet => (None, sheet_names(path)?),
        _ => (None, Vec::new()),
    };
    Ok(AttachmentInfo {
        path: path.display().to_string(),
        kind,
        bytes: bytes.min(u64::from(u32::MAX)) as u32,
        pages,
        sheets,
    })
}

/// A PDF's pages as PNGs, in order, up to the page and pixel budgets.
///
/// A page that would cross the pixel budget is rendered at the dpi that fits
/// it, down to 24 dpi; past that — and once the budget is spent — the remaining
/// pages are left out rather than returned as thumbnails nobody can read. A
/// file that is not a PDF is an error, not an empty result.
#[cfg(not(target_os = "android"))]
pub fn render_pdf(path: &Path, options: RenderOptions) -> Result<Vec<PdfPage>, String> {
    if options.dpi == 0 {
        return Err("render dpi must be at least 1".to_string());
    }
    if options.max_pages == 0 {
        return Ok(Vec::new());
    }
    let document = mupdf::Document::open(path)
        .map_err(|error| format!("cannot open {} as a PDF: {error}", path.display()))?;
    if !document.is_pdf() {
        return Err(format!("{} is not a PDF", path.display()));
    }
    // A locked document would fail page by page with MuPDF's own message; say
    // the real reason once instead.
    if document
        .needs_password()
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
    {
        return Err(format!("{} is password-protected", path.display()));
    }
    let count = document
        .page_count()
        .map_err(|error| format!("cannot count the pages of {}: {error}", path.display()))?;
    let wanted = u32::try_from(count.max(0))
        .unwrap_or(u32::MAX)
        .min(options.max_pages);
    let dpi = options.dpi.min(MAX_DPI);
    let mut pages = Vec::new();
    let mut spent = 0u64;
    for index in 0..wanted {
        let allowance = options.max_pixels.saturating_sub(spent);
        if allowance == 0 {
            break;
        }
        let page = document.load_page(index as i32).map_err(|error| {
            format!(
                "cannot load page {} of {}: {error}",
                index + 1,
                path.display()
            )
        })?;
        let bounds = page.bounds().map_err(|error| {
            format!(
                "cannot measure page {} of {}: {error}",
                index + 1,
                path.display()
            )
        })?;
        // Page geometry is in points (1/72 inch), the unit dpi is relative to.
        let width_points = f64::from(bounds.x1 - bounds.x0).max(0.0);
        let height_points = f64::from(bounds.y1 - bounds.y0).max(0.0);
        // A page that no longer fits at a readable size ends the render: the
        // pages after it would be no better off.
        let Some(page_dpi) = fit_dpi(dpi, width_points, height_points, allowance) else {
            break;
        };
        let scale = page_dpi as f32 / 72.0;
        let pixmap = page
            .to_pixmap(
                &mupdf::Matrix::new_scale(scale, scale),
                &mupdf::Colorspace::device_rgb(),
                false,
                false,
            )
            .map_err(|error| {
                format!(
                    "cannot render page {} of {}: {error}",
                    index + 1,
                    path.display()
                )
            })?;
        let (width, height) = (pixmap.width(), pixmap.height());
        let mut png = Vec::new();
        pixmap
            .write_to(&mut png, mupdf::ImageFormat::PNG)
            .map_err(|error| {
                format!(
                    "cannot encode page {} of {}: {error}",
                    index + 1,
                    path.display()
                )
            })?;
        spent = spent.saturating_add(u64::from(width) * u64::from(height));
        pages.push(PdfPage {
            index,
            width,
            height,
            png,
        });
    }
    Ok(pages)
}

/// PDF rendering needs MuPDF, whose build this app has no verified Android
/// toolchain for; the rest of the attachment layer still works there, so the
/// answer is a clear refusal rather than a build that does not exist.
#[cfg(target_os = "android")]
pub fn render_pdf(_path: &Path, _options: RenderOptions) -> Result<Vec<PdfPage>, String> {
    Err(
        "PDF pages cannot be rasterised on Android: this build has no MuPDF; \
         attach the pages as images instead"
            .to_string(),
    )
}

/// A sheet or CSV as a table. `sheet` names the sheet to read; `None` takes
/// the first sheet that has any value.
///
/// CSV delimiters are detected (`,` `;` tab), and Excel cells come back as the
/// strings they hold — integers, decimals, booleans and error codes included.
pub fn read_table(
    path: &Path,
    sheet: Option<&str>,
    options: TableOptions,
) -> Result<Table, String> {
    check_size(path)?;
    match kind_of(path) {
        AttachmentKind::Csv => read_csv(path, options),
        AttachmentKind::Spreadsheet => read_workbook(path, sheet, options),
        other => Err(format!(
            "{} is a {other:?} attachment, not a table: expected csv, tsv, xlsx, xls or ods",
            path.display()
        )),
    }
}

/// A text attachment (CSV, TSV, Markdown, JSON) as a string.
///
/// Refuses anything over `max_bytes` before reading it, and anything that is
/// not UTF-8 after — naming the byte where the encoding breaks, because a
/// Latin-1 export is a common way in.
pub fn read_text(path: &Path, max_bytes: u64) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "{} is {} bytes, over the {max_bytes}-byte text limit",
            path.display(),
            metadata.len()
        ));
    }
    // The file may have grown since the metadata check, so the read itself is
    // bounded too — one byte past the limit is enough to know.
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{} is over the {max_bytes}-byte text limit",
            path.display()
        ));
    }
    let text = String::from_utf8(bytes).map_err(|error| {
        format!(
            "{} is not UTF-8 text: invalid byte at offset {}",
            path.display(),
            error.utf8_error().valid_up_to()
        )
    })?;
    // A byte-order mark is an editor's artifact, not content.
    Ok(text
        .strip_prefix('\u{feff}')
        .map(str::to_string)
        .unwrap_or(text))
}

/// The MIME type a provider payload should carry for each kind. PDFs never
/// leave as `application/pdf`: [`render_pdf`] hands over PNG pages, one image
/// per page.
pub fn mime_of(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Pdf => "application/pdf",
        AttachmentKind::Image => "image/png",
        AttachmentKind::Spreadsheet => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        AttachmentKind::Csv => "text/csv",
        AttachmentKind::Text => "text/plain",
        AttachmentKind::Other => "application/octet-stream",
    }
}

/// Refuse `path` before a reader touches it when it is over the attachment
/// ceiling: [`read_table`] bounds rows and columns, but the file itself would
/// still be read whole, and a million-row xlsx is not a document.
fn check_size(path: &Path) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if metadata.is_file() && metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "{} is {} bytes, over the {MAX_ATTACHMENT_BYTES}-byte attachment limit",
            path.display(),
            metadata.len()
        ));
    }
    Ok(())
}

/// The kind `name` and `bytes` together settle, for callers holding the bytes
/// (an upload, a drag-and-drop payload) rather than a readable path — and for
/// the file whose extension lies about what it is.
///
/// Content decides first: a `%PDF-` header, an image signature, or a zip
/// container. A zip says only "office document", so the name picks the
/// workbook out of it — without a workbook name, a zip stays `None` rather
/// than being guessed at. `None` likewise for bytes that settle nothing.
pub fn describe_bytes(name: &str, bytes: &[u8]) -> Option<AttachmentKind> {
    if bytes.starts_with(b"%PDF-") {
        return Some(AttachmentKind::Pdf);
    }
    if is_image_magic(bytes) {
        return Some(AttachmentKind::Image);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        let extension = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())?;
        return kind_of_extension(&extension.to_ascii_lowercase())
            .filter(|kind| *kind == AttachmentKind::Spreadsheet);
    }
    None
}

/// What the file claims to be, by extension first and by content after.
fn kind_of(path: &Path) -> AttachmentKind {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .and_then(|extension| kind_of_extension(&extension))
        // A missing extension, or one nobody recognises, is decided by the
        // leading bytes instead — a scan dropped in as `invoice` is still a PDF.
        .unwrap_or_else(|| kind_from_content(path))
}

/// The kind an extension names, if it names one.
fn kind_of_extension(extension: &str) -> Option<AttachmentKind> {
    match extension {
        "pdf" => Some(AttachmentKind::Pdf),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif" => {
            Some(AttachmentKind::Image)
        }
        "xlsx" | "xls" | "xlsm" | "xlsb" | "ods" => Some(AttachmentKind::Spreadsheet),
        "csv" | "tsv" => Some(AttachmentKind::Csv),
        "txt" | "md" | "markdown" | "json" | "jsonl" | "ndjson" | "yaml" | "yml" | "toml"
        | "xml" | "html" | "htm" | "log" | "rst" => Some(AttachmentKind::Text),
        _ => None,
    }
}

/// The kind a file's first bytes give away, for files whose name does not.
fn kind_from_content(path: &Path) -> AttachmentKind {
    let mut magic = [0u8; 8];
    let read = File::open(path)
        .and_then(|mut file| file.read(&mut magic))
        .unwrap_or(0);
    let magic = &magic[..read];
    if magic.starts_with(b"%PDF-") {
        AttachmentKind::Pdf
    } else if is_image_magic(magic) {
        AttachmentKind::Image
    } else {
        AttachmentKind::Other
    }
}

/// The signatures the image formats this app accepts share. A PDF's pages are
/// what a model reads, so an image is recognised by content and not converted:
/// a PNG is already what [`render_pdf`] produces.
fn is_image_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.starts_with(b"RIFF") && bytes.len() >= 8 && &bytes[4..8] == b"WEBP")
        || bytes.starts_with(b"BM")
}

/// A PDF's page count, or `None` on a build without a rasteriser.
#[cfg(not(target_os = "android"))]
fn pdf_page_count(path: &Path) -> Result<Option<u32>, String> {
    let document = mupdf::Document::open(path)
        .map_err(|error| format!("cannot open {} as a PDF: {error}", path.display()))?;
    if !document.is_pdf() {
        return Err(format!("{} is not a PDF", path.display()));
    }
    let count = document
        .page_count()
        .map_err(|error| format!("cannot count the pages of {}: {error}", path.display()))?;
    Ok(Some(u32::try_from(count.max(0)).unwrap_or(u32::MAX)))
}

/// Without MuPDF there is nothing to ask: a PDF still describes, without a
/// page count.
#[cfg(target_os = "android")]
fn pdf_page_count(_path: &Path) -> Result<Option<u32>, String> {
    Ok(None)
}

/// The sheet names of a workbook, in the order the file lists them.
fn sheet_names(path: &Path) -> Result<Vec<String>, String> {
    let workbook = calamine::open_workbook_auto(path)
        .map_err(|error| format!("cannot open {} as a spreadsheet: {error}", path.display()))?;
    Ok(workbook.sheet_names())
}

/// The dpi at which a page of `width_points` × `height_points` fits `allowance`
/// pixels, never above `dpi`; `None` when it no longer fits legibly.
///
/// Downscaling is the whole point — a page too large for the budget comes back
/// smaller rather than absent — but only down to [`MIN_DPI`], below which the
/// page is a thumbnail a model cannot read and the budget is better spent on
/// nothing at all.
#[cfg_attr(target_os = "android", allow(dead_code))]
fn fit_dpi(dpi: u32, width_points: f64, height_points: f64, allowance: u64) -> Option<u32> {
    if width_points <= 0.0 || height_points <= 0.0 {
        return Some(dpi);
    }
    // Round up the way a rasteriser does, so the estimate never undershoots.
    let pixels = |dpi: u32| -> u64 {
        let scale = f64::from(dpi) / 72.0;
        let width = (width_points * scale).ceil().max(1.0) as u64;
        let height = (height_points * scale).ceil().max(1.0) as u64;
        width.saturating_mul(height)
    };
    if pixels(dpi) <= allowance {
        return Some(dpi);
    }
    let floor = dpi.min(MIN_DPI);
    if pixels(floor) > allowance {
        return None;
    }
    let scale = (allowance as f64 / (width_points * height_points)).sqrt();
    let mut candidate = ((72.0 * scale).floor() as u32).clamp(floor, dpi);
    while candidate > floor && pixels(candidate) > allowance {
        candidate -= 1;
    }
    Some(candidate)
}

/// The delimiter a CSV actually uses: the one that appears most often outside
/// quotes in the first few kilobytes. A comma is the guess when nothing shows.
fn sniff_delimiter(path: &Path) -> Result<u8, String> {
    const SAMPLE_BYTES: u64 = 8 * 1024;
    let mut sample = Vec::new();
    File::open(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .take(SAMPLE_BYTES)
        .read_to_end(&mut sample)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut commas = 0usize;
    let mut semicolons = 0usize;
    let mut tabs = 0usize;
    let mut quoted = false;
    let mut index = 0;
    while index < sample.len() {
        match sample[index] {
            // A doubled quote inside a quoted field is an escaped quote, not
            // the end of the field.
            b'"' if quoted && sample.get(index + 1) == Some(&b'"') => index += 1,
            b'"' => quoted = !quoted,
            b',' if !quoted => commas += 1,
            b';' if !quoted => semicolons += 1,
            b'\t' if !quoted => tabs += 1,
            _ => {}
        }
        index += 1;
    }
    // The last maximum wins a tie in `max_by_key`, so the comma — the safe
    // guess, and what a file with no delimiter at all gets — has to come last.
    let counts = [(b';', semicolons), (b'\t', tabs), (b',', commas)];
    Ok(counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(delimiter, _)| delimiter)
        .unwrap_or(b','))
}

/// A CSV or TSV as rows. The first record is the header row, as it is in the
/// exports people attach.
fn read_csv(path: &Path, options: TableOptions) -> Result<Table, String> {
    let delimiter = sniff_delimiter(path)?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut records = reader.records();
    let headers = match records.next() {
        Some(record) => {
            record.map_err(|error| format!("cannot read {}: {error}", path.display()))?
        }
        None => return Ok(Table::default()),
    };
    let columns = options.max_columns as usize;
    let mut table = Table {
        headers: headers.iter().take(columns).map(str::to_string).collect(),
        rows: Vec::new(),
        truncated: headers.len() > columns,
    };
    for record in records {
        let record = record.map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if table.rows.len() >= options.max_rows as usize {
            table.truncated = true;
            break;
        }
        table.truncated |= record.len() > columns;
        table
            .rows
            .push(record.iter().take(columns).map(str::to_string).collect());
    }
    Ok(table)
}

/// A workbook as rows, from the named sheet or the first one that holds a
/// value.
fn read_workbook(path: &Path, sheet: Option<&str>, options: TableOptions) -> Result<Table, String> {
    let mut workbook = calamine::open_workbook_auto(path)
        .map_err(|error| format!("cannot open {} as a spreadsheet: {error}", path.display()))?;
    let names = workbook.sheet_names();
    let name = match sheet {
        Some(wanted) => names
            .iter()
            .find(|name| name.as_str() == wanted)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{} has no sheet named {wanted:?}; it has: {}",
                    path.display(),
                    names.join(", ")
                )
            })?,
        None => first_filled_sheet(&mut workbook, &names)?
            .ok_or_else(|| format!("{} has no sheets to read", path.display()))?,
    };
    let range = workbook
        .worksheet_range(&name)
        .map_err(|error| format!("cannot read sheet {name:?} of {}: {error}", path.display()))?;
    table_from_range(&range, options)
}

/// The first sheet holding any value, else the first sheet, so an empty
/// workbook still answers with an (empty) table.
fn first_filled_sheet<RS: Read + std::io::Seek>(
    workbook: &mut impl Reader<RS>,
    names: &[String],
) -> Result<Option<String>, String> {
    for name in names {
        let range = workbook
            .worksheet_range(name)
            // The reader's own error type is only Debug, not Display.
            .map_err(|error| format!("cannot read sheet {name:?}: {error:?}"))?;
        if !range.is_empty() {
            return Ok(Some(name.clone()));
        }
    }
    Ok(names.first().cloned())
}

/// A worksheet's used rectangle as a table: the first row is the header, and
/// trailing blank rows are dropped because they are formatting, not content.
fn table_from_range(range: &calamine::Range<Data>, options: TableOptions) -> Result<Table, String> {
    let mut rows = range.rows();
    let header = match rows.next() {
        Some(header) => header,
        None => return Ok(Table::default()),
    };
    // The header's last filled cell sets the width; a blank header row is
    // nothing to tabulate.
    let width = header
        .iter()
        .rposition(|cell| !cell.is_empty())
        .map_or(0, |at| at + 1);
    if width == 0 {
        return Ok(Table::default());
    }
    let columns = options.max_columns as usize;
    let keep = width.min(columns);
    let mut table = Table {
        headers: header.iter().take(keep).map(cell_text).collect(),
        rows: Vec::new(),
        truncated: width > columns,
    };
    for row in rows {
        let cells: Vec<String> = row.iter().take(keep).map(cell_text).collect();
        table.truncated |= row.iter().skip(keep).any(|cell| !cell.is_empty());
        if table.rows.len() >= options.max_rows as usize {
            // Only a row with something in it proves the sheet was cut short;
            // the blank tail of a used rectangle proves nothing.
            if cells.iter().any(|cell| !cell.is_empty()) {
                table.truncated = true;
                break;
            }
            continue;
        }
        table.rows.push(cells);
    }
    while table
        .rows
        .last()
        .is_some_and(|row| row.iter().all(String::is_empty))
    {
        table.rows.pop();
    }
    Ok(table)
}

/// One cell as text. Floats come out in their shortest round-trip form, never
/// in exponent notation, and a date is the serial number the file stores —
/// calamine has no number format to apply.
fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(text) => text.clone(),
        Data::Int(value) => value.to_string(),
        Data::Float(value) => value.to_string(),
        Data::Bool(value) => (if *value { "TRUE" } else { "FALSE" }).to_string(),
        Data::DateTime(value) => value.to_string(),
        Data::DateTimeIso(text) | Data::DurationIso(text) => text.clone(),
        Data::Error(error) => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::testing::TempDir;

    /// A PDF of `count` pages, each 200 × 100 points and blank but for a filled
    /// rectangle: the smallest document MuPDF opens, and the geometry every
    /// render assertion is measured against. Written out with a real cross
    /// reference table so the file is a PDF, not a MuPDF repair.
    fn tiny_pdf(count: usize) -> Vec<u8> {
        // Object 1 is the catalog, 2 the page tree, then a page and its
        // content stream per page.
        let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
        let kids: Vec<String> = (0..count)
            .map(|page| format!("{} 0 R", 3 + page * 2))
            .collect();
        objects.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {count} >>",
            kids.join(" ")
        ));
        for page in 0..count {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents {} 0 R >>",
                4 + page * 2
            ));
            let content = "0 0 200 100 re f";
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
        }
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
        }
        let xref_at = pdf.len();
        pdf.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        pdf.into_bytes()
    }

    /// A workbook's opening: two sheets, the empty one first so the "first
    /// non-empty sheet" rule has a sheet to skip.
    fn xlsx_fixture() -> Vec<u8> {
        zip_file(&[
            (
                "[Content_Types].xml",
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Empty" sheetId="1" r:id="rId1"/><sheet name="Invoices" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1"/><sheetData/></worksheet>"#,
            ),
            (
                "xl/worksheets/sheet2.xml",
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Date</t></is></c><c r="B1" t="inlineStr"><is><t>Description</t></is></c><c r="C1" t="inlineStr"><is><t>Amount</t></is></c><c r="D1" t="inlineStr"><is><t>Paid</t></is></c></row><row r="2"><c r="A2"><v>45292</v></c><c r="B2" t="inlineStr"><is><t>Coffee, ground</t></is></c><c r="C2"><v>1234.5</v></c><c r="D2" t="b"><v>1</v></c></row><row r="3"><c r="A3"><v>45293</v></c><c r="B3" t="inlineStr"><is><t>Filter papers</t></is></c><c r="C3"><v>12.75</v></c><c r="D3" t="b"><v>0</v></c></row></sheetData></worksheet>"#,
            ),
        ])
    }

    /// An ODS of one sheet, its number stored as `office:value` and shown as
    /// `12.50` — the case where the cell's text and the file's value differ.
    fn ods_fixture() -> Vec<u8> {
        zip_file(&[
            (
                "mimetype",
                b"application/vnd.oasis.opendocument.spreadsheet",
            ),
            (
                "META-INF/manifest.xml",
                br#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#,
            ),
            (
                "content.xml",
                br#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2"><office:body><office:spreadsheet><table:table table:name="Rent"><table:table-row><table:table-cell office:value-type="string"><text:p>Month</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>Total</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell office:value-type="string"><text:p>January</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="12.5"><text:p>12.50</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#,
            ),
        ])
    }

    /// A stored (uncompressed) ZIP of `entries` — the container xlsx and ods
    /// both are. Writing it here keeps the fixtures bytes the test owns instead
    /// of checked-in blobs, and avoids pulling a zip crate in just for tests.
    fn zip_file(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = Vec::new();
        let mut directory = Vec::new();
        for (name, data) in entries {
            let offset = zip.len() as u32;
            let checksum = crc32(data);
            zip.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
            zip.extend_from_slice(&0u16.to_le_bytes()); // flags
            zip.extend_from_slice(&0u16.to_le_bytes()); // stored
            zip.extend_from_slice(&0u16.to_le_bytes()); // time
            zip.extend_from_slice(&0u16.to_le_bytes()); // date
            zip.extend_from_slice(&checksum.to_le_bytes());
            zip.extend_from_slice(&(data.len() as u32).to_le_bytes());
            zip.extend_from_slice(&(data.len() as u32).to_le_bytes());
            zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes()); // extra length
            zip.extend_from_slice(name.as_bytes());
            zip.extend_from_slice(data);

            directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            directory.extend_from_slice(&20u16.to_le_bytes()); // version made by
            directory.extend_from_slice(&20u16.to_le_bytes()); // version needed
            directory.extend_from_slice(&0u16.to_le_bytes()); // flags
            directory.extend_from_slice(&0u16.to_le_bytes()); // stored
            directory.extend_from_slice(&0u16.to_le_bytes()); // time
            directory.extend_from_slice(&0u16.to_le_bytes()); // date
            directory.extend_from_slice(&checksum.to_le_bytes());
            directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes()); // extra length
            directory.extend_from_slice(&0u16.to_le_bytes()); // comment length
            directory.extend_from_slice(&0u16.to_le_bytes()); // disk
            directory.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            directory.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }
        let directory_at = zip.len() as u32;
        let directory_size = directory.len() as u32;
        zip.extend_from_slice(&directory);
        zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes()); // end of directory
        zip.extend_from_slice(&0u16.to_le_bytes()); // this disk
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk with the directory
        zip.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        zip.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        zip.extend_from_slice(&directory_size.to_le_bytes());
        zip.extend_from_slice(&directory_at.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment length
        zip
    }

    /// The CRC-32 a ZIP entry carries, in the reflected polynomial every
    /// implementation uses.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let odd = crc & 1;
                crc >>= 1;
                if odd == 1 {
                    crc ^= 0xedb8_8320;
                }
            }
        }
        !crc
    }

    /// Put `bytes` in the fixture directory under `name`.
    fn fixture(dir: &TempDir, name: &str, bytes: impl AsRef<[u8]>) -> std::path::PathBuf {
        let path = dir.file(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn describe_reports_a_pdf_with_its_page_count() {
        let dir = TempDir::new("attachments-describe-pdf");
        let path = fixture(&dir, "scan.pdf", tiny_pdf(3));

        let info = describe(&path).unwrap();

        assert_eq!(info.path, path.display().to_string());
        assert_eq!(info.kind, AttachmentKind::Pdf);
        assert_eq!(info.pages, Some(3));
        assert_eq!(u64::from(info.bytes), fs::metadata(&path).unwrap().len());
        assert!(info.sheets.is_empty());
    }

    #[test]
    fn describe_lists_a_workbook_s_sheets() {
        let dir = TempDir::new("attachments-describe-sheets");
        let path = fixture(&dir, "invoices.xlsx", xlsx_fixture());

        let info = describe(&path).unwrap();

        assert_eq!(info.kind, AttachmentKind::Spreadsheet);
        assert_eq!(info.sheets, ["Empty", "Invoices"]);
        assert_eq!(info.pages, None);
    }

    #[test]
    fn describe_classifies_by_extension_and_by_content() {
        let dir = TempDir::new("attachments-kinds");
        // The magic-byte fixtures never reach a page count or a sheet scan, so
        // they are only names to classify.
        let pdf = fixture(&dir, "no-extension", b"%PDF-1.7\n");
        let png = fixture(&dir, "scan", b"\x89PNG\r\n\x1a\n");
        let blob = fixture(&dir, "blob.bin", b"\x00\x01\x02\x03");
        let png_named = fixture(&dir, "receipt.png", b"\x89PNG\r\n\x1a\n");

        assert_eq!(kind_of(&pdf), AttachmentKind::Pdf);
        assert_eq!(kind_of(&png), AttachmentKind::Image);
        assert_eq!(kind_of(&blob), AttachmentKind::Other);
        assert_eq!(kind_of(&png_named), AttachmentKind::Image);
        // Extensions win over content: a `.md` file is text, whatever it holds.
        assert_eq!(kind_of(Path::new("notes.md")), AttachmentKind::Text);
        assert_eq!(kind_of(Path::new("prices.csv")), AttachmentKind::Csv);
        assert_eq!(kind_of(Path::new("journal.tsv")), AttachmentKind::Csv);
        assert_eq!(kind_of(Path::new("book.xlsx")), AttachmentKind::Spreadsheet);
        assert_eq!(kind_of(Path::new("book.ODS")), AttachmentKind::Spreadsheet);
        assert_eq!(kind_of(Path::new("photo.JPEG")), AttachmentKind::Image);

        // Describing each kind: only a PDF or a workbook pays for a parse.
        let csv = fixture(&dir, "prices.csv", "Name,Amount\nRent,1000\n");
        let text = fixture(&dir, "notes.md", "# Notes\n");
        assert_eq!(describe(&csv).unwrap().kind, AttachmentKind::Csv);
        assert_eq!(describe(&text).unwrap().kind, AttachmentKind::Text);
        assert_eq!(describe(&blob).unwrap().kind, AttachmentKind::Other);

        let image = describe(&png_named).unwrap();
        assert_eq!(image.kind, AttachmentKind::Image);
        assert_eq!(image.pages, None);
        assert!(image.sheets.is_empty());
        assert_eq!(u64::from(image.bytes), fs::metadata(&png_named).unwrap().len());
    }

    #[test]
    fn describe_bytes_reads_the_magic_not_the_name() {
        // The four containers the panel meets: a PDF, a PNG, a JPEG and a
        // workbook zip.
        assert_eq!(
            describe_bytes("scan", b"%PDF-1.7\n"),
            Some(AttachmentKind::Pdf)
        );
        assert_eq!(
            describe_bytes("scan", b"\x89PNG\r\n\x1a\n"),
            Some(AttachmentKind::Image)
        );
        assert_eq!(
            describe_bytes("receipt", b"\xff\xd8\xff\xe0"),
            Some(AttachmentKind::Image)
        );
        assert_eq!(
            describe_bytes("invoices.xlsx", b"PK\x03\x04"),
            Some(AttachmentKind::Spreadsheet)
        );
        assert_eq!(
            describe_bytes("rent.ods", b"PK\x03\x04"),
            Some(AttachmentKind::Spreadsheet)
        );

        // Content wins over a name that lies: a JPEG called `statement.pdf` is
        // an image.
        assert_eq!(
            describe_bytes("statement.pdf", b"\xff\xd8\xff\xe0"),
            Some(AttachmentKind::Image)
        );
        // A zip that names no workbook, and bytes that settle nothing, are not
        // guessed at.
        assert_eq!(describe_bytes("archive.zip", b"PK\x03\x04"), None);
        assert_eq!(describe_bytes("notes.md", b"# Notes\n"), None);
    }

    #[test]
    fn describe_refuses_a_missing_file() {
        let dir = TempDir::new("attachments-missing");
        let error = describe(&dir.file("nothing.pdf")).unwrap_err();

        assert!(error.contains("cannot read"), "{error}");
        assert!(error.contains("nothing.pdf"), "{error}");
    }

    #[test]
    fn describe_refuses_a_file_over_the_byte_limit() {
        let dir = TempDir::new("attachments-huge");
        let path = dir.file("huge.csv");
        let file = fs::File::create(&path).unwrap();
        // Sparse: the length counts without writing the bytes.
        file.set_len(MAX_ATTACHMENT_BYTES + 1).unwrap();

        let error = describe(&path).unwrap_err();

        assert!(
            error.contains(&MAX_ATTACHMENT_BYTES.to_string()),
            "the limit is not named: {error}"
        );
        assert!(
            error.contains(&(MAX_ATTACHMENT_BYTES + 1).to_string()),
            "the size is not named: {error}"
        );
    }

    #[test]
    fn render_pdf_returns_a_png_per_page_at_the_requested_dpi() {
        let dir = TempDir::new("attachments-render");
        let path = fixture(&dir, "scan.pdf", tiny_pdf(2));

        let pages = render_pdf(&path, RenderOptions::default()).unwrap();

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].index, 0);
        assert_eq!(pages[1].index, 1);
        // 200 × 100 points at 150 dpi: 200 / 72 × 150 ≈ 417 wide, 100 / 72 × 150
        // ≈ 209 high — MuPDF rounds the transformed bounds up, and `fit_dpi`
        // estimates the same way.
        assert_eq!((pages[0].width, pages[0].height), (417, 209));
        assert_eq!((pages[1].width, pages[1].height), (417, 209));
        // PNG magic, so the bytes are an image and not a raw pixmap.
        assert!(pages[0].png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(pages[1].png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // The encoded page is what stays in memory, and it is not the raw
        // pixmap: 417 × 209 RGB would be 261 KiB before compression.
        assert!(
            pages[0].png.len() < 32 * 1024,
            "page PNG is {} bytes",
            pages[0].png.len()
        );
    }

    #[test]
    fn render_pdf_stops_at_the_page_limit() {
        let dir = TempDir::new("attachments-render-pages");
        let path = fixture(&dir, "three.pdf", tiny_pdf(3));
        let options = RenderOptions {
            max_pages: 1,
            ..RenderOptions::default()
        };

        let pages = render_pdf(&path, options).unwrap();

        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].index, 0);
    }

    #[test]
    fn render_pdf_downscales_a_page_over_the_pixel_budget() {
        let dir = TempDir::new("attachments-render-pixels");
        let path = fixture(&dir, "scan.pdf", tiny_pdf(1));
        // A whole page at 150 dpi is 417 × 208 = 86 736 pixels.
        let options = RenderOptions {
            dpi: 150,
            max_pages: 1,
            max_pixels: 10_000,
        };

        let pages = render_pdf(&path, options).unwrap();

        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert!(
            u64::from(page.width) * u64::from(page.height) <= options.max_pixels,
            "page is {} × {}, over the budget",
            page.width,
            page.height
        );
        assert!(
            page.width < 417 && page.height < 208,
            "page was not downscaled: {} × {}",
            page.width,
            page.height
        );
        assert!(page.png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn fit_dpi_keeps_the_asked_dpi_when_the_page_fits() {
        // 200 × 100 points at 150 dpi is 87 153 pixels, under 100 000.
        assert_eq!(fit_dpi(150, 200.0, 100.0, 100_000), Some(150));
        // The same page under 10 000 pixels: 50 dpi is 139 × 70 = 9 730.
        assert_eq!(fit_dpi(150, 200.0, 100.0, 10_000), Some(50));
        // A page with no geometry cannot be scaled down to fit anything.
        assert_eq!(fit_dpi(150, 0.0, 0.0, 1), Some(150));
        // Below the readable floor the page is dropped rather than shrunk to a
        // thumbnail: 24 dpi of this page is 2 278 pixels, more than 1 000.
        assert_eq!(fit_dpi(150, 200.0, 100.0, 1_000), None);
    }

    #[test]
    fn render_pdf_drops_the_pages_past_the_pixel_budget() {
        let dir = TempDir::new("attachments-render-budget");
        let path = fixture(&dir, "three.pdf", tiny_pdf(3));
        // The first page takes 87 153 pixels of the 100 000; the second fits
        // the rest downscaled; nothing readable is left for the third.
        let options = RenderOptions {
            dpi: 150,
            max_pages: 3,
            max_pixels: 100_000,
        };

        let pages = render_pdf(&path, options).unwrap();

        assert_eq!(pages.len(), 2);
        assert_eq!((pages[0].width, pages[0].height), (417, 209));
        assert!(pages[1].width < 417 && pages[1].height < 209);
        let spent: u64 = pages
            .iter()
            .map(|page| u64::from(page.width) * u64::from(page.height))
            .sum();
        assert!(spent <= options.max_pixels, "spent {spent} pixels");
    }

    #[test]
    fn render_pdf_refuses_a_file_that_is_not_a_pdf() {
        let dir = TempDir::new("attachments-render-refuse");
        let path = fixture(&dir, "notes.pdf", "just some text\n");

        let error = render_pdf(&path, RenderOptions::default()).unwrap_err();

        assert!(error.contains("PDF"), "{error}");
    }

    #[test]
    fn read_table_reads_the_first_non_empty_sheet() {
        let dir = TempDir::new("attachments-xlsx");
        let path = fixture(&dir, "invoices.xlsx", xlsx_fixture());

        let table = read_table(&path, None, TableOptions::default()).unwrap();

        assert_eq!(table.headers, ["Date", "Description", "Amount", "Paid"]);
        assert_eq!(table.rows.len(), 2);
        // Numbers as the strings a sheet shows: 45292 is Excel's serial for a
        // date, 1234.5 a decimal, 1 a TRUE flag.
        assert_eq!(table.rows[0], ["45292", "Coffee, ground", "1234.5", "TRUE"]);
        assert_eq!(table.rows[1], ["45293", "Filter papers", "12.75", "FALSE"]);
        assert!(!table.truncated);
    }

    #[test]
    fn read_table_reads_the_named_sheet() {
        let dir = TempDir::new("attachments-xlsx-sheet");
        let path = fixture(&dir, "invoices.xlsx", xlsx_fixture());

        let empty = read_table(&path, Some("Empty"), TableOptions::default()).unwrap();
        assert_eq!(empty, Table::default());

        let named = read_table(&path, Some("Invoices"), TableOptions::default()).unwrap();
        assert_eq!(named.headers.len(), 4);
    }

    #[test]
    fn read_table_refuses_a_sheet_that_is_not_there() {
        let dir = TempDir::new("attachments-xlsx-unknown");
        let path = fixture(&dir, "invoices.xlsx", xlsx_fixture());

        let error = read_table(&path, Some("Payroll"), TableOptions::default()).unwrap_err();

        assert!(error.contains("Payroll"), "{error}");
        // The message names what the workbook does have.
        assert!(error.contains("Invoices"), "{error}");
    }

    #[test]
    fn read_table_truncates_rows_and_columns() {
        let dir = TempDir::new("attachments-xlsx-caps");
        let path = fixture(&dir, "invoices.xlsx", xlsx_fixture());
        let options = TableOptions {
            max_rows: 1,
            max_columns: 2,
        };

        let table = read_table(&path, Some("Invoices"), options).unwrap();

        assert_eq!(table.headers, ["Date", "Description"]);
        assert_eq!(table.rows, [["45292", "Coffee, ground"]]);
        assert!(table.truncated);
    }

    #[test]
    fn read_table_reads_an_ods() {
        let dir = TempDir::new("attachments-ods");
        let path = fixture(&dir, "rent.ods", ods_fixture());

        let table = read_table(&path, None, TableOptions::default()).unwrap();

        assert_eq!(table.headers, ["Month", "Total"]);
        assert_eq!(table.rows, [["January", "12.5"]]);
    }

    #[test]
    fn read_csv_detects_semicolons_and_tabs() {
        let dir = TempDir::new("attachments-csv-delimiters");
        let semicolons = fixture(
            &dir,
            "bank.csv",
            "Date;Description;Amount\n2026-01-02;Bakery;12,50\n",
        );
        let tabs = fixture(&dir, "prices.tsv", "Name\tAmount\nRent\t1000\n");

        let semicolon_table = read_table(&semicolons, None, TableOptions::default()).unwrap();
        assert_eq!(semicolon_table.headers, ["Date", "Description", "Amount"]);
        // A decimal comma inside a semicolon CSV stays one field.
        assert_eq!(semicolon_table.rows, [["2026-01-02", "Bakery", "12,50"]]);

        let tab_table = read_table(&tabs, None, TableOptions::default()).unwrap();
        assert_eq!(tab_table.headers, ["Name", "Amount"]);
        assert_eq!(tab_table.rows, [["Rent", "1000"]]);
    }

    #[test]
    fn read_csv_keeps_quoted_delimiters_and_newlines() {
        let dir = TempDir::new("attachments-csv-quotes");
        let path = fixture(
            &dir,
            "export.csv",
            "Date,Description,Amount\n\
             2026-01-02,\"Coffee, ground\",1234.50\n\
             2026-01-03,\"Two\nlines\",9.99\n",
        );

        let table = read_table(&path, None, TableOptions::default()).unwrap();

        assert_eq!(table.headers, ["Date", "Description", "Amount"]);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0], ["2026-01-02", "Coffee, ground", "1234.50"]);
        // A newline inside quotes does not end the record.
        assert_eq!(table.rows[1], ["2026-01-03", "Two\nlines", "9.99"]);
    }

    #[test]
    fn read_csv_truncates_at_the_caps() {
        let dir = TempDir::new("attachments-csv-caps");
        let path = fixture(
            &dir,
            "wide.csv",
            "Date,Description,Amount,Note\na,b,c,d\ne,f,g,h\n",
        );
        let options = TableOptions {
            max_rows: 1,
            max_columns: 2,
        };

        let table = read_table(&path, None, options).unwrap();

        assert_eq!(table.headers, ["Date", "Description"]);
        assert_eq!(table.rows, [["a", "b"]]);
        assert!(table.truncated);
    }

    #[test]
    fn read_table_refuses_a_file_that_is_not_a_table() {
        let dir = TempDir::new("attachments-table-refuse");
        let text = fixture(&dir, "notes.md", "# Notes\n");
        let pdf = fixture(&dir, "scan.pdf", tiny_pdf(1));

        for path in [&text, &pdf] {
            let error = read_table(path, None, TableOptions::default()).unwrap_err();
            assert!(error.contains("not a table"), "{error}");
        }
        // A missing file is named, not guessed at.
        let error = read_table(&dir.file("gone.csv"), None, TableOptions::default()).unwrap_err();
        assert!(error.contains("cannot read"), "{error}");
    }

    #[test]
    fn read_table_refuses_a_file_over_the_byte_limit() {
        let dir = TempDir::new("attachments-table-huge");
        let path = dir.file("huge.csv");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_ATTACHMENT_BYTES + 1)
            .unwrap();

        let error = read_table(&path, None, TableOptions::default()).unwrap_err();

        assert!(
            error.contains(&MAX_ATTACHMENT_BYTES.to_string()),
            "the limit is not named: {error}"
        );
    }

    #[test]
    fn read_text_reads_utf8_and_strips_the_byte_order_mark() {
        let dir = TempDir::new("attachments-text");
        let path = fixture(&dir, "notes.md", "\u{feff}# Notes\n");

        assert_eq!(read_text(&path, 1024).unwrap(), "# Notes\n");
    }

    #[test]
    fn read_text_refuses_a_file_over_the_limit() {
        let dir = TempDir::new("attachments-text-limit");
        let path = fixture(&dir, "long.json", "{\"a\": 1}");

        let error = read_text(&path, 4).unwrap_err();

        assert!(error.contains("over the 4-byte text limit"), "{error}");
    }

    #[test]
    fn read_text_refuses_a_file_that_is_not_utf8() {
        let dir = TempDir::new("attachments-text-encoding");
        let path = fixture(&dir, "latin1.csv", [b'D', 0xe9, b'c']);

        let error = read_text(&path, 1024).unwrap_err();

        assert!(error.contains("not UTF-8"), "{error}");
        assert!(error.contains("offset 1"), "{error}");
    }

    #[test]
    fn mime_of_maps_every_kind() {
        assert_eq!(mime_of(AttachmentKind::Pdf), "application/pdf");
        assert_eq!(mime_of(AttachmentKind::Image), "image/png");
        assert_eq!(mime_of(AttachmentKind::Csv), "text/csv");
        assert_eq!(mime_of(AttachmentKind::Text), "text/plain");
        assert_eq!(mime_of(AttachmentKind::Other), "application/octet-stream");
        assert_eq!(
            mime_of(AttachmentKind::Spreadsheet),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
    }
}
