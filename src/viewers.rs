//! Builds in-game preview content for every supported file type: text,
//! hex dumps, archive listings, CSV tables, pretty JSON, PDF text
//! extraction, gzip'd text and font sanity checks. Pure CPU work; the
//! inspector UI renders whatever these return.

use std::io::Read;
use std::path::Path;

use crate::scan::human_size;

const BODY_CAP: usize = 60 * 1024;

pub fn read_text_preview(path: &Path, max_bytes: usize) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return "<could not read file>".into();
    };
    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut handle = file.take(max_bytes as u64);
    if handle.read_to_end(&mut buf).is_err() {
        return "<could not read file>".into();
    }
    let mut text = String::from_utf8_lossy(&buf)
        .replace('\r', "")
        .replace('\t', "    ");
    if buf.len() >= max_bytes {
        text.push_str("\n\n… (truncated)");
    }
    text
}

// ---------------------------------------------------------------------------
// Hex
// ---------------------------------------------------------------------------

/// Classic hex+ASCII dump of a byte slice.
pub fn hex_dump_bytes(buf: &[u8], total_size: u64) -> String {
    let mut out = String::with_capacity(buf.len() * 4);
    for (i, chunk) in buf.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for b in chunk {
            out.push(if (0x20..0x7f).contains(b) { *b as char } else { '.' });
        }
        out.push_str("|\n");
    }
    if total_size > buf.len() as u64 {
        out.push_str(&format!(
            "\n… {} of {} shown",
            human_size(buf.len() as u64),
            human_size(total_size)
        ));
    }
    out
}

pub fn hex_dump(path: &Path, max_bytes: usize, total_size: u64) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return "<could not read file>".into();
    };
    let mut buf = Vec::with_capacity(max_bytes);
    if file.take(max_bytes as u64).read_to_end(&mut buf).is_err() {
        return "<could not read file>".into();
    }
    hex_dump_bytes(&buf, total_size)
}

// ---------------------------------------------------------------------------
// Archives
// ---------------------------------------------------------------------------

/// Lists the entries inside zip- and tar-family archives, entirely in-game.
pub fn archive_listing(path: &Path) -> String {
    const MAX_ENTRIES: usize = 400;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut entries: Vec<(String, u64)> = Vec::new();
    let mut more = 0usize;

    let result: Result<(), String> = (|| {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        match ext.as_str() {
            "zip" | "jar" | "whl" => {
                let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
                for i in 0..zip.len() {
                    let entry = zip.by_index_raw(i).map_err(|e| e.to_string())?;
                    if entries.len() < MAX_ENTRIES {
                        entries.push((entry.name().to_string(), entry.size()));
                    } else {
                        more += 1;
                    }
                }
                Ok(())
            }
            "tar" => list_tar(file, &mut entries, &mut more, MAX_ENTRIES),
            "tgz" | "crate" => list_tar(
                flate2::read::GzDecoder::new(file),
                &mut entries,
                &mut more,
                MAX_ENTRIES,
            ),
            "gz" if name.ends_with(".tar.gz") => list_tar(
                flate2::read::GzDecoder::new(file),
                &mut entries,
                &mut more,
                MAX_ENTRIES,
            ),
            _ => Err(format!("listing .{ext} archives is not supported")),
        }
    })();

    match result {
        Ok(()) => {
            let mut out = format!("{} entries\n\n", entries.len() + more);
            for (name, size) in &entries {
                out.push_str(&format!("{:>9}  {}\n", human_size(*size), name));
            }
            if more > 0 {
                out.push_str(&format!("\n… and {more} more"));
            }
            out
        }
        Err(err) => format!(
            "Could not list contents: {err}\n\nFalling back to hex view:\n\n{}",
            hex_dump(path, 1024, std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
        ),
    }
}

fn list_tar<R: Read>(
    reader: R,
    entries: &mut Vec<(String, u64)>,
    more: &mut usize,
    max: usize,
) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entries.len() < max {
            entries.push((
                entry.path().map_err(|e| e.to_string())?.display().to_string(),
                entry.size(),
            ));
        } else {
            *more += 1;
        }
    }
    Ok(())
}

/// Plain `.gz` (not a tarball): decompress and show the payload as text if
/// it is text, otherwise as a hex dump.
pub fn gz_preview(path: &Path) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return "<could not read file>".into();
    };
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut buf = vec![0u8; BODY_CAP];
    let mut read = 0;
    while read < buf.len() {
        match decoder.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) => return format!("Could not decompress: {e}"),
        }
    }
    buf.truncate(read);
    let text = String::from_utf8_lossy(&buf);
    let junk = text.chars().filter(|&c| c == '\u{FFFD}').count();
    if junk * 30 < text.chars().count().max(1) {
        let mut out = format!("(decompressed preview)\n\n{}", text.replace('\r', ""));
        if read == BODY_CAP {
            out.push_str("\n\n… (truncated)");
        }
        out
    } else {
        format!(
            "(decompressed payload is binary)\n\n{}",
            hex_dump_bytes(&buf[..buf.len().min(2048)], read as u64)
        )
    }
}

// ---------------------------------------------------------------------------
// Structured text: CSV/TSV tables, pretty JSON
// ---------------------------------------------------------------------------

/// Aligns delimited rows into padded columns for readable previews.
/// Naive splitting (quoted delimiters not handled), good enough on approach.
pub fn csv_preview(path: &Path, tab: bool) -> String {
    const MAX_ROWS: usize = 150;
    const MAX_COLS: usize = 12;
    const MAX_CELL: usize = 26;

    let raw = read_text_preview(path, 128 * 1024);
    let delim = if tab { '\t' } else { ',' };
    let rows: Vec<Vec<String>> = raw
        .lines()
        .take(MAX_ROWS)
        .map(|line| {
            line.split(delim)
                .take(MAX_COLS)
                .map(|c| {
                    let c = c.trim().trim_matches('"');
                    let mut s: String = c.chars().take(MAX_CELL).collect();
                    if c.chars().count() > MAX_CELL {
                        s.push('…');
                    }
                    s
                })
                .collect()
        })
        .collect();
    if rows.len() < 2 {
        return raw;
    }

    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for (ri, row) in rows.iter().enumerate() {
        for (i, cell) in row.iter().enumerate() {
            out.push_str(cell);
            if i + 1 < row.len() {
                out.extend(std::iter::repeat_n(' ', widths[i] - cell.chars().count() + 2));
            }
        }
        out.push('\n');
        // Rule under the header row.
        if ri == 0 {
            for (i, w) in widths.iter().enumerate() {
                out.extend(std::iter::repeat_n('─', *w));
                if i + 1 < cols {
                    out.push_str("  ");
                }
            }
            out.push('\n');
        }
    }
    out.push_str(&format!("\n(first {} rows, columns padded)", rows.len()));
    out
}

/// Pretty-prints JSON files (great for minified payloads). Falls back to the
/// raw text when the file is huge or malformed.
pub fn json_preview(path: &Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > 1024 * 1024 {
        return read_text_preview(path, BODY_CAP);
    }
    let Ok(raw) = std::fs::read_to_string(path) else {
        return read_text_preview(path, BODY_CAP);
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => {
            let mut pretty = serde_json::to_string_pretty(&value).unwrap_or(raw);
            if pretty.len() > BODY_CAP {
                pretty.truncate(BODY_CAP);
                pretty.push_str("\n\n… (truncated)");
            }
            pretty
        }
        Err(_) => read_text_preview(path, BODY_CAP),
    }
}

// ---------------------------------------------------------------------------
// PDF text extraction
// ---------------------------------------------------------------------------

pub fn pdf_preview(path: &Path, size: u64) -> String {
    const MAX_PDF_BYTES: u64 = 15 * 1024 * 1024;
    const MAX_PAGES: usize = 8;
    if size > MAX_PDF_BYTES {
        return format!(
            "PDF is {} — too large to parse in-game.\nPress R to reveal it in Finder.",
            human_size(size)
        );
    }
    let doc = match lopdf::Document::load(path) {
        Ok(doc) => doc,
        Err(e) => return format!("Could not parse PDF: {e}"),
    };
    if doc.is_encrypted() {
        return "PDF is encrypted.".into();
    }
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let total = pages.len();
    let sample: Vec<u32> = pages.into_iter().take(MAX_PAGES).collect();
    let text = doc.extract_text(&sample).unwrap_or_default();
    let cleaned = text.trim();
    if cleaned.is_empty() {
        return format!(
            "{total} page{}\n\n(no extractable text — likely scanned images or vector-only)",
            if total == 1 { "" } else { "s" }
        );
    }
    let mut body = format!(
        "{total} page{} · text of the first {}\n\n",
        if total == 1 { "" } else { "s" },
        sample.len().min(total)
    );
    body.push_str(cleaned);
    if body.len() > BODY_CAP {
        body.truncate(BODY_CAP);
        body.push_str("\n\n… (truncated)");
    }
    body
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// Reads a ttf/otf file and sanity-checks the magic bytes; returns the raw
/// data ready to become a live Bevy `Font` asset.
pub fn read_font_bytes(path: &Path) -> Result<Vec<u8>, String> {
    const MAX_FONT_BYTES: u64 = 12 * 1024 * 1024;
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_FONT_BYTES {
        return Err(format!("font file is {} — too large", human_size(size)));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let ok = bytes.len() > 4
        && matches!(
            &bytes[..4],
            [0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true" | b"ttcf" | b"typ1"
        );
    if !ok {
        return Err("not a recognized ttf/otf font".into());
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// XML / RTF
// ---------------------------------------------------------------------------

/// Pretty-prints XML-ish files (plist, svg source, generic xml). Binary
/// plists fall back to a hex dump.
pub fn xml_preview(path: &Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > 2 * 1024 * 1024 {
        return read_text_preview(path, BODY_CAP);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return "<could not read file>".into();
    };
    if bytes.starts_with(b"bplist") {
        return format!(
            "Binary property list.\n\n{}",
            hex_dump_bytes(&bytes[..bytes.len().min(2048)], size)
        );
    }
    let raw = String::from_utf8_lossy(&bytes);
    pretty_xml(&raw)
}

fn pretty_xml(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 256);
    let mut indent = 0usize;
    let mut i = 0;
    let chars: Vec<char> = raw.chars().collect();
    while i < chars.len() {
        if chars[i] == '<' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '>' {
                j += 1;
            }
            if j >= chars.len() {
                out.extend(chars[i..].iter());
                break;
            }
            let tag: String = chars[i..=j].iter().collect();
            let closing = tag.starts_with("</");
            let self_closing = tag.ends_with("/>") || tag.starts_with("<?") || tag.starts_with("<!");
            if closing && indent > 0 {
                indent -= 1;
            }
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            for _ in 0..indent {
                out.push_str("  ");
            }
            out.push_str(&tag);
            if !closing && !self_closing {
                indent += 1;
            }
            i = j + 1;
        } else {
            let mut j = i;
            while j < chars.len() && chars[j] != '<' {
                j += 1;
            }
            let text: String = chars[i..j].iter().collect();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                out.push_str(trimmed);
            }
            i = j;
        }
        if out.len() > BODY_CAP {
            out.push_str("\n\n… (truncated)");
            break;
        }
    }
    out
}

/// Strips RTF control words so a `.rtf` file is readable as prose.
pub fn rtf_preview(path: &Path) -> String {
    let raw = read_text_preview(path, BODY_CAP);
    let mut out = String::with_capacity(raw.len() / 2);
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if matches!(chars.peek(), Some('\\' | '{' | '}')) {
                    out.push(chars.next().unwrap());
                    continue;
                }
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    let h1 = chars.next().unwrap_or('0');
                    let h2 = chars.next().unwrap_or('0');
                    if let Ok(b) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                        out.push(b as char);
                    }
                    continue;
                }
                while matches!(chars.peek(), Some(ch) if ch.is_ascii_alphabetic()) {
                    chars.next();
                }
                if chars.peek() == Some(&'-') {
                    chars.next();
                }
                while matches!(chars.peek(), Some(ch) if ch.is_ascii_digit()) {
                    chars.next();
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
            }
            '{' | '}' => {}
            '\n' | '\r' => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            other => out.push(other),
        }
        if out.len() > BODY_CAP {
            out.push_str("\n\n… (truncated)");
            break;
        }
    }
    let cleaned = out.trim().to_string();
    if cleaned.is_empty() {
        raw
    } else {
        cleaned
    }
}

// ---------------------------------------------------------------------------
// Office / EPUB (zip + XML)
// ---------------------------------------------------------------------------

const OFFICE_CAP: u64 = 20 * 1024 * 1024;

fn read_zip_entry(path: &Path, name: &str) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut entry = zip.by_name(name).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn zip_names(path: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut names = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        if let Ok(entry) = zip.by_index_raw(i) {
            names.push(entry.name().to_string());
        }
    }
    Ok(names)
}

fn xml_tag_texts(xml: &str, local_name: &str) -> String {
    let open = format!("<{local_name}");
    let close = format!("</{local_name}>");
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(gt) = after.find('>') else { break };
        let inner = &after[gt + 1..];
        if let Some(end) = inner.find(&close) {
            let text = inner[..end]
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#39;", "'");
            let text = text.trim();
            if !text.is_empty() {
                if !out.is_empty() && !out.ends_with([' ', '\n']) {
                    out.push(' ');
                }
                out.push_str(text);
            }
            rest = &inner[end + close.len()..];
        } else {
            break;
        }
        if out.len() > BODY_CAP {
            out.push_str("\n\n… (truncated)");
            break;
        }
    }
    out
}

pub fn docx_preview(path: &Path, size: u64) -> String {
    if size > OFFICE_CAP {
        return format!("Document is {} — too large to extract in-game.", human_size(size));
    }
    match read_zip_entry(path, "word/document.xml") {
        Ok(bytes) => {
            let xml = String::from_utf8_lossy(&bytes);
            let text = xml_tag_texts(&xml, "w:t");
            if text.trim().is_empty() {
                "Word document with no extractable text.".into()
            } else {
                text
            }
        }
        Err(err) => format!("Not a readable .docx ({err}).\n\n{}", archive_listing(path)),
    }
}

pub fn xlsx_preview(path: &Path, size: u64) -> String {
    if size > OFFICE_CAP {
        return format!("Workbook is {} — too large to extract in-game.", human_size(size));
    }
    let names = match zip_names(path) {
        Ok(n) => n,
        Err(err) => return format!("Not a readable .xlsx ({err}).\n\n{}", archive_listing(path)),
    };
    let sheets: Vec<_> = names
        .iter()
        .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
        .cloned()
        .collect();
    let mut out = format!("{} sheet{}\n\n", sheets.len(), if sheets.len() == 1 { "" } else { "s" });
    if let Ok(bytes) = read_zip_entry(path, "xl/sharedStrings.xml") {
        let xml = String::from_utf8_lossy(&bytes);
        let strings = xml_tag_texts(&xml, "t");
        if !strings.trim().is_empty() {
            out.push_str("Shared strings (sample):\n");
            out.push_str(strings.trim());
            out.push_str("\n\n");
        }
    }
    for name in sheets.iter().take(3) {
        if let Ok(bytes) = read_zip_entry(path, name) {
            let xml = String::from_utf8_lossy(&bytes);
            let cells = xml_tag_texts(&xml, "v");
            out.push_str(&format!("[{name}]\n"));
            if cells.trim().is_empty() {
                out.push_str("(no inline cell values — see shared strings above)\n\n");
            } else {
                out.push_str(cells.trim());
                out.push_str("\n\n");
            }
        }
        if out.len() > BODY_CAP {
            out.truncate(BODY_CAP);
            out.push_str("\n\n… (truncated)");
            break;
        }
    }
    out
}

pub fn pptx_preview(path: &Path, size: u64) -> String {
    if size > OFFICE_CAP {
        return format!("Presentation is {} — too large to extract in-game.", human_size(size));
    }
    let names = match zip_names(path) {
        Ok(n) => n,
        Err(err) => return format!("Not a readable .pptx ({err}).\n\n{}", archive_listing(path)),
    };
    let mut slides: Vec<_> = names
        .iter()
        .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
        .cloned()
        .collect();
    slides.sort();
    let mut out = format!("{} slide{}\n\n", slides.len(), if slides.len() == 1 { "" } else { "s" });
    for (i, name) in slides.iter().take(12).enumerate() {
        if let Ok(bytes) = read_zip_entry(path, name) {
            let xml = String::from_utf8_lossy(&bytes);
            let text = xml_tag_texts(&xml, "a:t");
            out.push_str(&format!("— slide {} —\n", i + 1));
            if text.trim().is_empty() {
                out.push_str("(no extractable text)\n\n");
            } else {
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
        }
        if out.len() > BODY_CAP {
            out.truncate(BODY_CAP);
            out.push_str("\n\n… (truncated)");
            break;
        }
    }
    out
}

pub fn epub_preview(path: &Path, size: u64) -> String {
    if size > OFFICE_CAP {
        return format!("EPUB is {} — too large to extract in-game.", human_size(size));
    }
    let names = match zip_names(path) {
        Ok(n) => n,
        Err(err) => return format!("Not a readable EPUB ({err}).\n\n{}", archive_listing(path)),
    };
    let htmls: Vec<_> = names
        .iter()
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".xhtml") || l.ends_with(".html") || l.ends_with(".htm")
        })
        .cloned()
        .collect();
    if htmls.is_empty() {
        return archive_listing(path);
    }
    let mut out = String::new();
    for name in htmls.iter().take(8) {
        if let Ok(bytes) = read_zip_entry(path, name) {
            let xml = String::from_utf8_lossy(&bytes);
            let text = strip_markup(&xml);
            if !text.trim().is_empty() {
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
        }
        if out.len() > BODY_CAP {
            out.truncate(BODY_CAP);
            out.push_str("\n\n… (truncated)");
            break;
        }
    }
    if out.trim().is_empty() {
        archive_listing(path)
    } else {
        out
    }
}

fn strip_markup(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut skip = false;
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            let rest: String = chars[i..].iter().take(10).collect::<String>().to_ascii_lowercase();
            if rest.starts_with("<script") || rest.starts_with("<style") {
                skip = true;
            }
            if rest.starts_with("</script") || rest.starts_with("</style") {
                skip = false;
            }
            in_tag = true;
            i += 1;
            continue;
        }
        if chars[i] == '>' {
            in_tag = false;
            i += 1;
            continue;
        }
        if !in_tag && !skip {
            out.push(chars[i]);
        }
        i += 1;
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

pub fn sqlite_preview(path: &Path, size: u64) -> String {
    const MAX_DB: u64 = 80 * 1024 * 1024;
    if size > MAX_DB {
        return format!("Database is {} — too large to open in-game.", human_size(size));
    }
    let Ok(head) = std::fs::File::open(path).and_then(|mut f| {
        let mut b = [0u8; 16];
        f.read_exact(&mut b).map(|_| b)
    }) else {
        return "<could not read file>".into();
    };
    if &head[..15] != b"SQLite format 3" {
        return format!(
            "Not a SQLite database.\n\n{}",
            hex_dump(path, 1024, size)
        );
    }
    let conn = match rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => return format!("Could not open SQLite file: {e}"),
    };
    let mut stmt = match conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    ) {
        Ok(s) => s,
        Err(e) => return format!("Could not list tables: {e}"),
    };
    let tables: Vec<String> = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(e) => return format!("Could not list tables: {e}"),
    };
    if tables.is_empty() {
        return "SQLite database with no user tables.".into();
    }
    let mut out = format!("{} table{}\n\n", tables.len(), if tables.len() == 1 { "" } else { "s" });
    for table in tables.iter().take(12) {
        let ident = table.replace('"', "\"\"");
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{ident}\""), [], |row| row.get(0))
            .unwrap_or(0);
        out.push_str(&format!("[{table}]  {count} rows\n"));
        if let Ok(mut q) = conn.prepare(&format!("SELECT * FROM \"{ident}\" LIMIT 6")) {
            let n = q.column_count().min(8);
            let headers: Vec<String> = (0..n).map(|i| q.column_name(i).unwrap_or("?").to_string()).collect();
            out.push_str(&headers.join(" | "));
            out.push('\n');
            if let Ok(rows) = q.query_map([], |row| {
                let mut cells = Vec::new();
                for i in 0..n {
                    let cell = row
                        .get_ref(i)
                        .map(|v| match v.data_type() {
                            rusqlite::types::Type::Null => "NULL".into(),
                            rusqlite::types::Type::Integer => row
                                .get::<_, i64>(i)
                                .map(|n| n.to_string())
                                .unwrap_or("?".into()),
                            rusqlite::types::Type::Real => row
                                .get::<_, f64>(i)
                                .map(|n| format!("{n:.4}"))
                                .unwrap_or("?".into()),
                            rusqlite::types::Type::Text => {
                                let s: String = row.get(i).unwrap_or_default();
                                s.chars().take(24).collect()
                            }
                            rusqlite::types::Type::Blob => "<blob>".into(),
                        })
                        .unwrap_or_else(|_| "?".into());
                    cells.push(cell);
                }
                Ok(cells.join(" | "))
            }) {
                for row in rows.flatten() {
                    out.push_str(&row);
                    out.push('\n');
                }
            }
        }
        out.push('\n');
        if out.len() > BODY_CAP {
            out.truncate(BODY_CAP);
            out.push_str("\n\n… (truncated)");
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Audio / video container metadata (no decode of the media stream)
// ---------------------------------------------------------------------------

pub fn audio_preview(path: &Path, size: u64) -> Vec<String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut lines = vec![
        format!("Kind:  Audio ({ext})"),
        format!("Size:  {}", human_size(size)),
        format!("Path:  {}", path.display()),
        String::new(),
    ];
    match ext.as_str() {
        "wav" => lines.extend(wav_info(path)),
        "flac" => lines.extend(flac_info(path)),
        "mp3" => lines.extend(id3_info(path)),
        "ogg" | "oga" => lines.push("Playback: supported (Vorbis).".into()),
        "m4a" | "aac" => {
            lines.push("Playback: not decoded in-game (AAC).".into());
            lines.push("Press R to reveal it in Finder.".into());
        }
        _ => lines.push("Press E again to stop playback.".into()),
    }
    if matches!(ext.as_str(), "wav" | "flac" | "mp3" | "ogg" | "oga") {
        lines.push("Playing now (toggle with E on the building). Esc closes this overlay.".into());
    }
    lines
}

fn wav_info(path: &Path) -> Vec<String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return vec!["Could not read WAV header.".into()];
    };
    let mut bytes = [0u8; 44];
    if file.read_exact(&mut bytes).is_err() || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return vec!["Not a standard WAV file.".into()];
    }
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
    let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let bits = u16::from_le_bytes([bytes[34], bytes[35]]);
    vec![
        format!("Format:  {channels} ch · {rate} Hz · {bits}-bit"),
        "Playback: supported.".into(),
    ]
}

fn flac_info(path: &Path) -> Vec<String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return vec!["Could not read FLAC header.".into()];
    };
    let mut bytes = [0u8; 42];
    if file.read_exact(&mut bytes).is_err() || &bytes[..4] != b"fLaC" {
        return vec!["Not a standard FLAC file.".into()];
    }
    // STREAMINFO starts at offset 8 after the 4-byte last-metadata+length header.
    let sr = ((bytes[18] as u32) << 12) | ((bytes[19] as u32) << 4) | ((bytes[20] as u32) >> 4);
    let channels = ((bytes[20] >> 1) & 0x07) + 1;
    vec![
        format!("Format:  {channels} ch · {sr} Hz"),
        "Playback: supported.".into(),
    ]
}

fn id3_info(path: &Path) -> Vec<String> {
    let Ok(file) = std::fs::File::open(path) else {
        return vec!["Playback: supported (MP3).".into()];
    };
    let mut buf = [0u8; 10];
    if file.take(10).read_exact(&mut buf).is_err() || &buf[..3] != b"ID3" {
        return vec!["Playback: supported (MP3).".into()];
    }
    let size = ((buf[6] as usize) << 21)
        | ((buf[7] as usize) << 14)
        | ((buf[8] as usize) << 7)
        | (buf[9] as usize);
    let Ok(file) = std::fs::File::open(path) else {
        return vec!["Playback: supported (MP3).".into()];
    };
    let mut tag = vec![0u8; (10 + size).min(64 * 1024)];
    let n = file.take(tag.len() as u64).read(&mut tag).unwrap_or(0);
    tag.truncate(n);
    let title = id3_frame(&tag, b"TIT2");
    let artist = id3_frame(&tag, b"TPE1");
    let mut out = Vec::new();
    if let Some(t) = title {
        out.push(format!("Title:   {t}"));
    }
    if let Some(a) = artist {
        out.push(format!("Artist:  {a}"));
    }
    out.push("Playback: supported (MP3).".into());
    out
}

fn id3_frame(tag: &[u8], id: &[u8; 4]) -> Option<String> {
    let mut i = 10;
    while i + 10 < tag.len() {
        let frame_id = &tag[i..i + 4];
        if frame_id.iter().all(|&b| b == 0) {
            break;
        }
        let len = u32::from_be_bytes(tag[i + 4..i + 8].try_into().ok()?) as usize;
        if i + 10 + len > tag.len() {
            break;
        }
        if frame_id == id {
            let body = &tag[i + 10..i + 10 + len];
            if body.is_empty() {
                return None;
            }
            // encoding byte 0 = ISO-8859-1, 3 = UTF-8. Treat both as utf8-lossy.
            let s = String::from_utf8_lossy(&body[1..]).trim_matches('\0').trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        i += 10 + len;
    }
    None
}

pub fn video_preview(path: &Path, size: u64) -> Vec<String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut lines = vec![
        format!("Kind:  Video ({ext})"),
        format!("Size:  {}", human_size(size)),
        format!("Path:  {}", path.display()),
        String::new(),
    ];
    if matches!(ext.as_str(), "mp4" | "m4v" | "mov") {
        lines.extend(mp4_info(path, size));
    }
    lines.push("Video frames are not decoded in-game.".into());
    lines.push("Press R to reveal it in Finder.".into());
    lines
}

fn mp4_info(path: &Path, size: u64) -> Vec<String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut head = vec![0u8; 4 * 1024 * 1024];
    let n = file.read(&mut head).unwrap_or(0);
    head.truncate(n);
    let mut out = Vec::new();
    if let Some(brand) = ftyp_brand(&head) {
        out.push(format!("Brand:  {brand}"));
    }
    if let Some(info) = find_mvhd(&head) {
        out.push(info);
        return out;
    }
    // moov often lives at the end; probe the tail.
    if size > head.len() as u64 {
        use std::io::{Seek, SeekFrom};
        let tail_len = 512 * 1024u64;
        let start = size.saturating_sub(tail_len);
        if file.seek(SeekFrom::Start(start)).is_ok() {
            let mut tail = vec![0u8; tail_len as usize];
            let n = file.read(&mut tail).unwrap_or(0);
            tail.truncate(n);
            if let Some(info) = find_mvhd(&tail) {
                out.push(info);
            }
        }
    }
    out
}

fn ftyp_brand(data: &[u8]) -> Option<String> {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return None;
    }
    Some(String::from_utf8_lossy(&data[8..12]).trim().to_string())
}

fn find_mvhd(data: &[u8]) -> Option<String> {
    let mut i = 0;
    while i + 8 <= data.len() {
        let mut size = u32::from_be_bytes(data[i..i + 4].try_into().ok()?) as usize;
        let kind = &data[i + 4..i + 8];
        if size == 1 && i + 16 <= data.len() {
            size = u64::from_be_bytes(data[i + 8..i + 16].try_into().ok()?) as usize;
        }
        if size < 8 {
            break;
        }
        if kind == b"mvhd" && i + 24 < data.len() {
            let version = data[i + 8];
            let (timescale, duration) = if version == 1 && i + 36 <= data.len() {
                let ts = u32::from_be_bytes(data[i + 28..i + 32].try_into().ok()?);
                let dur = u64::from_be_bytes(data[i + 32..i + 40].try_into().ok()?);
                (ts, dur)
            } else if i + 24 <= data.len() {
                let ts = u32::from_be_bytes(data[i + 20..i + 24].try_into().ok()?);
                let dur = u32::from_be_bytes(data[i + 24..i + 28].try_into().ok()?) as u64;
                (ts, dur)
            } else {
                return None;
            };
            if timescale > 0 {
                let secs = duration as f64 / timescale as f64;
                return Some(format!("Duration:  {secs:.1}s (from container)"));
            }
            return None;
        }
        // Recurse into containers that wrap mvhd.
        if matches!(kind, b"moov" | b"trak" | b"mdia") {
            let header = if data[i..].len() >= 16 && size > 16 && u32::from_be_bytes(data[i..i+4].try_into().ok()?) == 1 {
                16
            } else {
                8
            };
            if let Some(found) = find_mvhd(&data[i + header..(i + size).min(data.len())]) {
                return Some(found);
            }
        }
        i = i.saturating_add(size);
        if i <= 8 {
            break;
        }
    }
    None
}

pub const FONT_PANGRAMS: [&str; 3] = [
    "The quick brown fox jumps over the lazy dog.",
    "Sphinx of black quartz, judge my vow! 0123456789",
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz {}[]()<>#@&%",
];

// ---------------------------------------------------------------------------
// SVG rasterization (used by building screens and the image inspector)
// ---------------------------------------------------------------------------

pub fn rasterize_svg(path: &Path, max_dim: u32) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&bytes, &opt).ok()?;
    let size = tree.size();
    let (src_w, src_h) = (size.width().max(1.0), size.height().max(1.0));
    let scale = (max_dim as f32 / src_w).min(max_dim as f32 / src_h).min(1.0);
    let w = (src_w * scale).round().max(1.0) as u32;
    let h = (src_h * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some((w, h, pixmap.take()))
}
