use crate::{Result, invalid};
use std::{
    collections::BTreeSet,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 25 * 1024 * 1024;
const MAX_CHARS: usize = 50_000;
const MAX_PDF_CACHE_CHARS: usize = 500_000;
const MAX_PDF_OBJECTS: usize = 50_000;
const MAX_DIR_DEPTH: usize = 16;

/// Returns the root directory for course study materials (`<root>/resources`).
pub fn sources_dir(root: &Path) -> PathBuf {
    root.join("resources")
}

/// Returns the root directory for extracted text cache (`<root>/data/text`).
pub fn cache_dir(root: &Path) -> PathBuf {
    root.join("data").join("text")
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn push_bounded(out: &mut String, count: &mut usize, text: &str) {
    let remaining = MAX_CHARS.saturating_sub(*count);
    for ch in text.chars().take(remaining) {
        out.push(ch);
        *count += 1;
    }
}

fn extract(name: &str, bytes: &[u8]) -> Result<Option<String>> {
    if bytes.len() > MAX_BYTES {
        return Err(invalid(
            "Dosya 25 MB sınırını aşıyor / File exceeds 25 MB limit.",
        ));
    }
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "txt" | "md" => Ok(Some(truncate(&String::from_utf8_lossy(bytes), MAX_CHARS))),
        "pdf" => {
            let document = lopdf::Document::load_mem(bytes)?;
            if document.objects.len() > MAX_PDF_OBJECTS {
                return Err(invalid(
                    "PDF çok karmaşık (50.000 nesne sınırını aşıyor) / PDF is too complex (exceeds 50,000 object limit).",
                ));
            }
            let pages = document.get_pages();
            let mut output = String::new();
            let mut output_chars = 0usize;
            let mut readable = 0;
            for page in pages.keys().take(30) {
                // A broken content stream on one page must not hide the readable pages around it.
                let Ok(page_text) = document.extract_text(&[*page]) else {
                    continue;
                };
                readable += 1;
                if !output.is_empty() {
                    output.push('\n');
                    output_chars += 1;
                }
                push_bounded(&mut output, &mut output_chars, &page_text);
                if output_chars >= MAX_CHARS {
                    break;
                }
            }
            if readable == 0 && !pages.is_empty() {
                return Err(invalid(
                    "PDF sayfaları okunamadı / Failed to read PDF pages.",
                ));
            }
            Ok(Some(output))
        }
        "docx" => {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
            let entry = archive.by_name("word/document.xml")?;
            if entry.size() > MAX_BYTES as u64 {
                return Err(invalid(
                    "Belge içeriği 25 MB sınırını aşıyor / Document content exceeds 25 MB limit.",
                ));
            }
            let mut xml = Vec::new();
            entry.take(MAX_BYTES as u64 + 1).read_to_end(&mut xml)?;
            if xml.len() > MAX_BYTES {
                return Err(invalid(
                    "Belge içeriği 25 MB sınırını aşıyor / Document content exceeds 25 MB limit.",
                ));
            }
            let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
            let mut output = String::new();
            let mut output_chars = 0usize;
            let mut inside = false;
            loop {
                use quick_xml::events::Event;
                match reader.read_event()? {
                    Event::Start(e) if e.local_name().as_ref() == b"t" => {
                        if !output.is_empty() {
                            output.push('\n');
                            output_chars += 1;
                        }
                        inside = true;
                    }
                    Event::End(e) if e.local_name().as_ref() == b"t" => inside = false,
                    Event::Text(e) if inside => {
                        push_bounded(&mut output, &mut output_chars, &e.decode()?);
                    }
                    Event::CData(e) if inside => {
                        push_bounded(&mut output, &mut output_chars, &e.decode()?);
                    }
                    Event::GeneralRef(e) if inside => {
                        let raw_ref = format!("&{};", e.decode()?);
                        let decoded = quick_xml::escape::unescape(&raw_ref)?;
                        push_bounded(&mut output, &mut output_chars, &decoded);
                    }
                    Event::Eof => break,
                    _ => (),
                }
                if output_chars >= MAX_CHARS {
                    break;
                }
            }
            Ok(Some(output))
        }
        _ => Ok(None),
    }
}

pub fn subjects(root: &Path) -> Result<Vec<String>> {
    let directory = sources_dir(root);
    let Ok(canon_root) = fs::canonicalize(&directory) else {
        return Ok(vec![]);
    };
    let mut subjects = vec![];
    for entry in fs::read_dir(&directory)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        // metadata follows junctions/symlinks inside resources while broken links skip safely;
        // canonical containment ensures targets cannot escape the resources root.
        if !meta.is_dir() {
            continue;
        }
        let Ok(canon_entry) = fs::canonicalize(&path) else {
            continue;
        };
        if canon_entry == canon_root || !canon_entry.starts_with(&canon_root) {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || crate::course::validate_identifier(&name).is_err() {
            continue;
        }
        subjects.push(name);
    }
    subjects.sort();
    Ok(subjects)
}

/// Kopya, arşiv, sınav ve gizli klasörler ders materyali olarak indekslenmez.
const SKIPPED_DIRECTORIES: [&str; 3] = ["_duplicates", "99_ARCHIVE", "exams"];

fn files(
    directory: &Path,
    canon_root: &Path,
    depth: usize,
    visited: &mut BTreeSet<PathBuf>,
    output: &mut Vec<PathBuf>,
) -> Result<()> {
    let Ok(canon) = fs::canonicalize(directory) else {
        return Ok(());
    };
    if !canon.starts_with(canon_root) || !visited.insert(canon) {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(kind) = fs::metadata(&path) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if kind.is_dir() {
            if depth < MAX_DIR_DEPTH && !SKIPPED_DIRECTORIES.contains(&name.as_str()) {
                files(&path, canon_root, depth + 1, visited, output)?;
            }
        } else if kind.is_file()
            && !name.eq_ignore_ascii_case("course.json")
            && path.extension().is_some_and(|extension| {
                ["pdf", "docx", "txt", "md"]
                    .contains(&extension.to_string_lossy().to_lowercase().as_str())
            })
        {
            let Ok(canon_file) = fs::canonicalize(&path) else {
                continue;
            };
            if canon_file.starts_with(canon_root) {
                output.push(path);
            }
        }
    }
    if depth == 0 {
        output.sort();
    }
    Ok(())
}

fn cache_name(relative: &Path) -> String {
    format!(
        "{}.txt",
        relative
            .to_string_lossy()
            .replace('%', "%25")
            .replace(['\\', '/'], "%2F")
    )
}

pub fn index(root: &Path, force: bool) -> Result<usize> {
    let sources_root = sources_dir(root);
    fs::create_dir_all(&sources_root)?;
    let canon_sources = fs::canonicalize(&sources_root)?;
    let cache_root = cache_dir(root);
    crate::config::ensure_private_dir(&cache_root)?;
    let mut updated = 0;
    let subjects = subjects(root)?;
    for subject in &subjects {
        let source = sources_root.join(subject);
        let cache = cache_root.join(subject);
        crate::config::ensure_private_dir(&cache)?;
        let mut paths = vec![];
        let mut visited = BTreeSet::new();
        files(&source, &canon_sources, 0, &mut visited, &mut paths)?;
        let mut expected = BTreeSet::new();
        for path in paths {
            let name = cache_name(path.strip_prefix(&source)?);
            let target = cache.join(&name);
            expected.insert(name);
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            let newer = match fs::metadata(&target) {
                Ok(cached) => metadata.modified()? > cached.modified()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => return Err(error.into()),
            };
            if force || newer {
                let name = path
                    .file_name()
                    .ok_or_else(|| invalid("Dosya adı eksik / Missing file name."))?
                    .to_string_lossy();
                if metadata.len() > MAX_BYTES as u64 {
                    crate::telegram::log_file(
                        root,
                        &format!("Atlandı (25 MB üstü): {subject}/{name}"),
                    );
                    continue;
                }
                let text = name
                    .to_lowercase()
                    .ends_with(".pdf")
                    .then(|| pdftotext(&path))
                    .flatten();
                let extracted = match text {
                    Some(text) => Ok(Some(text)),
                    None => fs::read(&path)
                        .map_err(Into::into)
                        .and_then(|bytes| extract(&name, &bytes)),
                };
                match extracted {
                    Ok(Some(text)) => {
                        crate::config::atomic_write_private(&target, text.as_bytes())?;
                        updated += 1;
                    }
                    Ok(None) => {}
                    // One unreadable file must not hide the rest of the subject.
                    Err(_) => crate::telegram::log_file(
                        root,
                        &format!("Okunamadı, atlandı: {subject}/{name}"),
                    ),
                }
            }
        }
        for entry in fs::read_dir(&cache)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|ext| ext == "txt")
                && !expected.contains(&entry.file_name().to_string_lossy().into_owned())
            {
                fs::remove_file(entry.path())?;
            }
        }
    }
    // A removed subject must not leave sources available to a later prompt.
    for entry in fs::read_dir(&cache_root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && !subjects.contains(&entry.file_name().to_string_lossy().into_owned())
        {
            for file in fs::read_dir(entry.path())? {
                let file = file?;
                if file.file_type()?.is_file()
                    && file.path().extension().is_some_and(|ext| ext == "txt")
                {
                    fs::remove_file(file.path())?;
                }
            }
        }
    }
    Ok(updated)
}

fn clean_extracted(raw: &str) -> String {
    let mut output = String::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !output.ends_with("\n\n") && !output.is_empty() {
                output.push('\n');
            }
            continue;
        }
        // Skip empty dotted worksheet lines.
        let non_punct = trimmed.chars().filter(|c| c.is_alphanumeric()).count();
        if (trimmed.contains("……") || trimmed.contains("......")) && non_punct < 5 {
            continue;
        }
        output.push_str(trimmed);
        output.push('\n');
    }
    output
}

/// Extracts PDF text via poppler's pdftotext from PATH (or Git for Windows on Windows).
fn pdftotext(path: &Path) -> Option<String> {
    if let Some(text) = run_pdftotext(path) {
        return Some(text);
    }
    // Observed: the Windows pdftotext build fails on a name with decomposed Turkish letters ("İ" as
    // I + U+0307), while the precomposed name of the same file works. Retry on an ASCII-named copy.
    if path.file_name()?.to_string_lossy().is_ascii() {
        return None;
    }
    let tmp = tempfile::Builder::new()
        .prefix("studia-")
        .suffix(".pdf")
        .tempfile()
        .ok()?
        .into_temp_path();
    fs::copy(path, &tmp).ok()?;
    run_pdftotext(&tmp)
}

fn run_pdftotext(path: &Path) -> Option<String> {
    let candidates: &[&str] = &[
        "pdftotext",
        #[cfg(windows)]
        r"C:\Program Files\Git\mingw64\bin\pdftotext.exe",
    ];
    for program in candidates {
        let mut command = std::process::Command::new(program);
        command.args(["-enc", "UTF-8", "-q"]).arg(path).arg("-");
        if let Ok(stdout) = crate::telegram::command_output_with_timeout(
            &mut command,
            "",
            Some(std::time::Duration::from_secs(30)),
            MAX_BYTES,
            &[],
        ) {
            let text = truncate(&clean_extracted(&stdout), MAX_PDF_CACHE_CHARS);
            if !text.trim().is_empty() {
                // Cached up to MAX_PDF_CACHE_CHARS so chunk selection covers large books safely.
                return Some(text);
            }
        }
    }
    None
}

const CHUNK_CHARS: usize = 3_000;
const PROMPT_CHARS: usize = 28_000;

pub fn fold_tr(text: &str) -> String {
    text.chars()
        .flat_map(|ch| match ch {
            'ı' | 'I' | 'İ' => 'i'.to_lowercase(),
            'ğ' | 'Ğ' => 'g'.to_lowercase(),
            'ü' | 'Ü' => 'u'.to_lowercase(),
            'ş' | 'Ş' => 's'.to_lowercase(),
            'ö' | 'Ö' => 'o'.to_lowercase(),
            'ç' | 'Ç' => 'c'.to_lowercase(),
            _ => ch.to_lowercase(),
        })
        .collect()
}

fn words(text: &str) -> BTreeSet<String> {
    fold_tr(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_owned)
        .collect()
}

fn for_each_char_chunk(text: &str, chunk_chars: usize, mut f: impl FnMut(&str)) {
    if text.is_empty() || chunk_chars == 0 {
        return;
    }
    let mut start = 0usize;
    let mut count = 0usize;
    for (idx, _) in text.char_indices() {
        if count == chunk_chars {
            f(&text[start..idx]);
            start = idx;
            count = 0;
        }
        count += 1;
    }
    if start < text.len() {
        f(&text[start..]);
    }
}

/// Source text for one prompt: the chunks sharing the most words with `query`, capped at
/// `PROMPT_CHARS`, so a large subject still fits in the AI context window.
pub fn text(root: &Path, subject: &str, query: &str) -> Result<String> {
    crate::course::validate_identifier(subject)?;
    // Only names obtained from the first-level material folders may be selected.
    if !subjects(root)?.iter().any(|name| name == subject) {
        return Err(invalid("Ders bulunamadı / Course not found."));
    }
    let cache_root = cache_dir(root);
    let directory = cache_root.join(subject);
    let mut paths = vec![];
    if directory.exists() {
        let canon_cache_root = fs::canonicalize(&cache_root)?;
        let canon_dir = fs::canonicalize(&directory)?;
        if !canon_dir.starts_with(&canon_cache_root) {
            return Err(invalid(
                "Önbellek dizini geçersiz / Invalid cache directory.",
            ));
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.is_file()
                && path.extension().is_some_and(|ext| ext == "txt")
                && let Ok(canon_file) = fs::canonicalize(&path)
                && canon_file.starts_with(&canon_cache_root)
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let wanted = words(query);
    let mut chunks = vec![];
    let mut seen = BTreeSet::new();
    for path in paths {
        let Some(raw_name) = path.file_name() else {
            continue;
        };
        let name = raw_name.to_string_lossy();
        let name = name
            .strip_suffix(".txt")
            .unwrap_or(&name)
            .replace("%2F", "/")
            .replace("%25", "%");
        if name.contains(" (2).") {
            continue;
        }
        let name_folded = fold_tr(&name);
        let name_bonus = wanted
            .iter()
            .filter(|w| w.len() >= 4 && name_folded.contains(w.as_str()))
            .count()
            * 4;
        let content = fs::read_to_string(&path)?;
        for_each_char_chunk(&content, CHUNK_CHARS, |piece| {
            if piece.trim().is_empty() || !seen.insert(piece.to_string()) {
                return;
            }
            let raw_hits = words(&format!("{name} {piece}"))
                .intersection(&wanted)
                .count();
            let hits = raw_hits + name_bonus;
            chunks.push((hits, chunks.len(), name.clone(), piece.to_string()));
        });
    }
    if chunks.is_empty() {
        return Err(invalid(
            "Bu derste metin kaynağı yok. resources/ klasörüne belge ekleyin / No text sources in this course. Add documents to resources/.",
        ));
    }
    chunks.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut picked = vec![];
    let mut size = 0;
    for chunk in chunks {
        if size + chunk.3.len() > PROMPT_CHARS && !picked.is_empty() {
            break;
        }
        size += chunk.3.len();
        picked.push(chunk);
    }
    picked.sort_by_key(|chunk| chunk.1);
    let mut output = String::new();
    for (_, _, name, piece) in picked {
        output.push_str(&format!("\n### {name}\n{piece}\n"));
    }
    Ok(output)
}

/// Lists up to `max_items` relative study file paths inside `resources/<subject>/`
/// so AI plan generation can align days with actual source materials.
pub fn catalog(root: &Path, subject: &str, max_items: usize) -> Result<Vec<String>> {
    crate::course::validate_identifier(subject)?;
    if !subjects(root)?.iter().any(|name| name == subject) {
        return Err(invalid("Ders bulunamadı / Course not found."));
    }
    let sources_root = sources_dir(root);
    let canon_root = fs::canonicalize(&sources_root)?;
    let subject_dir = sources_root.join(subject);
    let mut paths = Vec::new();
    let mut visited = BTreeSet::new();
    files(&subject_dir, &canon_root, 0, &mut visited, &mut paths)?;
    let mut items = Vec::new();
    for path in paths {
        if let Ok(rel) = path.strip_prefix(&subject_dir) {
            let clean = rel.to_string_lossy().replace('\\', "/");
            if !clean.contains(" (2).") {
                items.push(clean);
                if items.len() >= max_items {
                    break;
                }
            }
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn duplicate_and_archive_folders_are_not_indexed_and_big_files_do_not_stop_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let subject = root.path().join("resources/Ders");
        for dir in ["_duplicates", "99_ARCHIVE", "Kitaplar"] {
            fs::create_dir_all(subject.join(dir)).unwrap();
        }
        fs::write(subject.join("_duplicates/kopya.txt"), "kopya").unwrap();
        fs::write(subject.join("99_ARCHIVE/eski.txt"), "eski").unwrap();
        fs::write(subject.join("Kitaplar/yeni.txt"), "yeni").unwrap();
        // A 26 MB text file is over the reader limit and must be skipped, not abort the index.
        let big = fs::File::create(subject.join("Kitaplar/dev.txt")).unwrap();
        big.set_len(26 * 1024 * 1024).unwrap();
        drop(big);
        assert_eq!(index(root.path(), false).unwrap(), 1);
        let context = text(root.path(), "Ders", "").unwrap();
        assert!(context.contains("yeni"));
        assert!(!context.contains("kopya") && !context.contains("eski"));
    }

    #[test]
    fn cache_rebuild_refresh_prune_and_nested_filename_collisions() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("resources/Ders/nested")).unwrap();
        for (name, text) in [
            ("a.md", "ilk"),
            ("nested/a.md", "alt"),
            ("nested%2Fa.md", "yüzde"),
        ] {
            fs::write(root.path().join("resources/Ders").join(name), text).unwrap();
        }
        assert_eq!(index(root.path(), false).unwrap(), 3);
        assert_eq!(index(root.path(), false).unwrap(), 0);
        let context = text(root.path(), "Ders", "").unwrap();
        assert!(context.contains("### nested/a.md\nalt"));
        assert!(context.contains("### nested%2Fa.md\nyüzde"));
        let path = root.path().join("resources/Ders/a.md");
        fs::write(&path, "yeni").unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        drop(file);
        assert_eq!(index(root.path(), false).unwrap(), 1);
        assert!(text(root.path(), "Ders", "").unwrap().contains("yeni"));
        fs::remove_file(path).unwrap();
        index(root.path(), false).unwrap();
        assert!(!text(root.path(), "Ders", "").unwrap().contains("yeni"));
        assert!(text(root.path(), "../Ders", "").is_err());
    }

    #[test]
    fn extraction_limits_docx_entities_and_supported_types() {
        assert_eq!(
            extract("a.TXT", "Türkçe".as_bytes()).unwrap(),
            Some("Türkçe".into())
        );
        assert_eq!(extract("a.csv", b"ignored").unwrap(), None);
        assert!(extract("a.pdf", b"bad PDF").is_err());
        assert!(extract("a.md", &vec![b'x'; MAX_BYTES + 1]).is_err());
        assert_eq!(
            extract("a.md", "ş".repeat(MAX_CHARS + 1).as_bytes())
                .unwrap()
                .unwrap()
                .chars()
                .count(),
            MAX_CHARS
        );
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(br#"<w:document xmlns:w="test"><w:t>A &amp; B</w:t><w:t><![CDATA[C]]></w:t></w:document>"#).unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        assert_eq!(extract("a.docx", &bytes).unwrap(), Some("A & B\nC".into()));
    }

    /// Minimal PDF with one page per content stream (Helvetica only).
    fn pdf_with_pages(contents: &[&str]) -> Vec<u8> {
        let kids: String = (0..contents.len())
            .map(|index| format!("{} 0 R ", 4 + 2 * index))
            .collect();
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!(
                "<< /Type /Pages /Kids [{kids}] /Count {} >>",
                contents.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        for (index, content) in contents.iter().enumerate() {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
                5 + 2 * index
            ));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
        }
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            pdf.extend(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    #[test]
    fn pdf_page_with_broken_content_does_not_hide_the_other_pages() {
        // "1.2.3.4.5 Tf" is rejected by lopdf's content parser; a real file hit the same error.
        let broken = "1.2.3.4.5 Tf";
        let good = "BT /F1 12 Tf 72 720 Td (birinci) Tj ET";
        let text = extract("a.pdf", &pdf_with_pages(&[good, broken]))
            .unwrap()
            .unwrap();
        assert_eq!(text.trim(), "birinci");
        assert!(extract("a.pdf", &pdf_with_pages(&[broken])).is_err());
    }

    #[test]
    fn large_subject_keeps_only_matching_chunks() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("data/text/Ders");
        fs::create_dir_all(root.path().join("resources/Ders")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        for index in 0..30 {
            fs::write(
                cache.join(format!("dolgu{index:02}.txt")),
                "lorem ".repeat(500),
            )
            .unwrap();
        }
        fs::write(cache.join("zz.txt"), "passive voice konusu").unwrap();
        let context = text(root.path(), "Ders", "Passive voice nedir?").unwrap();
        assert!(context.contains("passive voice konusu"));
        assert!(context.len() <= PROMPT_CHARS + 2_000);
    }

    #[test]
    fn spec_src_1_2_3_limits_skipped_dirs_and_no_legacy_heuristics() {
        let root = tempfile::tempdir().unwrap();
        let resources = root.path().join("resources");
        fs::create_dir_all(resources.join(".hidden")).unwrap();
        fs::write(resources.join(".hidden/secret.txt"), "hidden subject").unwrap();

        let subject = resources.join("Ders");
        for dir in [".git", "exams", "notes"] {
            fs::create_dir_all(subject.join(dir)).unwrap();
        }
        fs::write(subject.join(".git/config.txt"), "git hidden").unwrap();
        fs::write(subject.join("exams/past.txt"), "exam question").unwrap();
        fs::write(subject.join("course.json"), "{}").unwrap();
        fs::write(subject.join("notes/real.txt"), "actual material").unwrap();

        // Depth limit (16): nested subdir past depth 16 must not be indexed.
        let mut deep_ok = subject.join("notes");
        for i in 0..14 {
            deep_ok = deep_ok.join(format!("d{i}"));
        }
        fs::create_dir_all(&deep_ok).unwrap();
        fs::write(deep_ok.join("ok.txt"), "within depth").unwrap();
        let mut too_deep = deep_ok.clone();
        for i in 14..18 {
            too_deep = too_deep.join(format!("d{i}"));
        }
        fs::create_dir_all(&too_deep).unwrap();
        fs::write(too_deep.join("ignored.txt"), "beyond depth").unwrap();

        // >25 MiB non-ASCII PDF must skip before pdftotext / tempfile copy and log to data/bot.log.
        let big_pdf = fs::File::create(subject.join("notes/türkçe_dev.pdf")).unwrap();
        big_pdf.set_len(MAX_BYTES as u64 + 1).unwrap();
        drop(big_pdf);

        let subs = subjects(root.path()).unwrap();
        assert_eq!(subs, vec!["Ders".to_string()]);
        assert_eq!(index(root.path(), false).unwrap(), 2);

        let context = text(root.path(), "Ders", "").unwrap();
        assert!(context.contains("actual material"));
        assert!(context.contains("within depth"));
        assert!(!context.contains("git hidden"));
        assert!(!context.contains("exam question"));
        assert!(!context.contains("beyond depth"));

        let log = fs::read_to_string(root.path().join("data/bot.log")).unwrap_or_default();
        assert!(log.contains("türkçe_dev.pdf"));

        // SPEC-02 COURSE-1: words() must not inject hardcoded trigger expansions.
        let tokens = words("edat");
        assert!(tokens.contains("edat"));
        assert!(!tokens.contains("zamiredat"));
        assert!(!tokens.contains("edatlarlistesi"));

        // Symlink loop & broken symlink check when OS permits symlink creation.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(
                resources.join("missing_target"),
                resources.join("broken_link"),
            )
            .unwrap();
            symlink(&subject, subject.join("notes/loop_back")).unwrap();
            assert_eq!(subjects(root.path()).unwrap(), vec!["Ders".to_string()]);
            index(root.path(), false).unwrap();
        }
    }

    #[test]
    fn spec_sec_canonical_containment_pdf_caps_and_streaming_chunks() {
        // Streaming char_indices chunking preserves multibyte Turkish text across boundaries.
        let sample = "şğüıöçİabc";
        let mut pieces = Vec::new();
        for_each_char_chunk(sample, 4, |chunk| pieces.push(chunk.to_string()));
        assert_eq!(pieces, vec!["şğüı", "öçİa", "bc"]);
        assert_eq!(pieces.concat(), sample);

        // PDF with > MAX_PDF_OBJECTS is rejected cleanly.
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(MAX_PDF_OBJECTS + 1);
        for i in 1..=(MAX_PDF_OBJECTS + 1) {
            offsets.push(pdf.len());
            pdf.extend(format!("{i} 0 obj\nnull\nendobj\n").as_bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
        for off in offsets {
            pdf.extend(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend(
            format!(
                "trailer\n<< /Size {} >>\nstartxref\n{xref}\n%%EOF\n",
                MAX_PDF_OBJECTS + 2
            )
            .as_bytes(),
        );
        let err = extract("bomb.pdf", &pdf).unwrap_err().to_string();
        assert!(err.contains("50.000"), "unexpected error: {err}");

        // Symlinks pointing outside resources / data/text must be skipped/rejected.
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let root = tempfile::tempdir().unwrap();
            let resources = root.path().join("resources");
            let outside = root.path().join("outside");
            fs::create_dir_all(resources.join("Safe")).unwrap();
            fs::create_dir_all(&outside).unwrap();
            fs::write(resources.join("Safe/ok.txt"), "safe content").unwrap();
            fs::write(outside.join("secret.txt"), "leaked secret").unwrap();

            symlink(&outside, resources.join("EscapedSubject")).unwrap();
            symlink(
                outside.join("secret.txt"),
                resources.join("Safe/escaped.txt"),
            )
            .unwrap();

            assert_eq!(subjects(root.path()).unwrap(), vec!["Safe".to_string()]);
            assert_eq!(index(root.path(), false).unwrap(), 1);
            let ctx = text(root.path(), "Safe", "").unwrap();
            assert!(ctx.contains("safe content"));
            assert!(!ctx.contains("leaked secret"));

            let cache_dir = root.path().join("data/text/Safe");
            assert_eq!(
                fs::metadata(root.path().join("data/text"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&cache_dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            let cached_file = cache_dir.join("ok.txt.txt");
            assert_eq!(
                fs::metadata(&cached_file).unwrap().permissions().mode() & 0o777,
                0o600
            );

            symlink(outside.join("secret.txt"), cache_dir.join("evil_cache.txt")).unwrap();
            let ctx2 = text(root.path(), "Safe", "").unwrap();
            assert!(!ctx2.contains("leaked secret"));

            fs::remove_dir_all(&cache_dir).unwrap();
            symlink(&outside, &cache_dir).unwrap();
            assert!(text(root.path(), "Safe", "").is_err());
        }
    }

    #[test]
    fn catalog_lists_subject_files_respecting_skips_and_bounds() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("resources/Math");
        fs::create_dir_all(sub.join("Algebra")).unwrap();
        fs::create_dir_all(sub.join("exams")).unwrap();
        fs::write(sub.join("Algebra/01_linear.md"), "x").unwrap();
        fs::write(sub.join("Algebra/02_quad.txt"), "y").unwrap();
        fs::write(sub.join("exams/mock.json"), "{}").unwrap();
        fs::write(sub.join("course.json"), "{}").unwrap();
        let list = catalog(root.path(), "Math", 10).unwrap();
        assert_eq!(
            list,
            vec![
                "Algebra/01_linear.md".to_string(),
                "Algebra/02_quad.txt".to_string()
            ]
        );
        assert_eq!(catalog(root.path(), "Math", 1).unwrap().len(), 1);
    }
}
