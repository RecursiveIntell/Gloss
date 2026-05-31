use crate::db::notebook_db::Source;
use crate::error::GlossError;
use crate::redaction::{redact_path, redact_text_paths};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;
use zip::read::ZipArchive;

const MAX_DOCUMENT_ARCHIVE_ENTRIES: usize = 2_000;
const MAX_DOCUMENT_XML_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LEGACY_OFFICE_BYTES: u64 = 32 * 1024 * 1024;
const LEGACY_OFFICE_TIMEOUT_MS: u64 = 20_000;
const MAX_DOCUMENT_TEXT_CHARS: usize = 1_000_000;

pub struct ExtractedText {
    pub text: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
struct LegacyOfficeExtractorReceipt {
    schema: &'static str,
    receipt_id: String,
    source_id: String,
    source_path_redacted: String,
    format: String,
    extractor: &'static str,
    argv_redacted: Vec<String>,
    timeout_ms: u64,
    elapsed_ms: u128,
    exit_code: Option<i32>,
    success: bool,
    timed_out: bool,
    stdout_sha256: String,
    stdout_bytes: usize,
    stderr_sha256: String,
    stderr_bytes: usize,
    stderr_preview_redacted: Option<String>,
    output_truncated: bool,
}

/// Extract text content from a source based on its type.
/// Phase 1 supports: text, markdown, paste.
#[allow(dead_code)]
pub fn extract_text(source: &Source, notebook_dir: &Path) -> Result<String, GlossError> {
    extract_text_with_metadata(source, notebook_dir).map(|extracted| extracted.text)
}

pub fn extract_text_with_metadata(
    source: &Source,
    notebook_dir: &Path,
) -> Result<ExtractedText, GlossError> {
    match source.source_type.as_str() {
        "text" | "markdown" | "code" => {
            // Read from file (code files are treated as UTF-8 text)
            let text = if let Some(ref file_path) = source.file_path {
                let full_path = notebook_dir.join("sources").join(file_path);
                // Prevent path traversal: canonicalize and verify the path stays
                // within the notebook sources directory.
                let sources_dir = notebook_dir.join("sources");
                match full_path.canonicalize() {
                    Ok(canonical) => {
                        let canonical_root = match sources_dir.canonicalize() {
                            Ok(root) => root,
                            Err(e) => {
                                tracing::warn!(
                                    "Cannot canonicalize sources dir {:?}: {} — rejecting path as unsafe",
                                    sources_dir, e
                                );
                                return Err(GlossError::Ingestion {
                                    source_id: source.id.clone(),
                                    message: "Cannot verify source path safety (sources dir not canonicalizable)".into(),
                                });
                            }
                        };
                        if !canonical.starts_with(&canonical_root) {
                            return Err(GlossError::Ingestion {
                                source_id: source.id.clone(),
                                message: "Path traversal detected in source file_path".into(),
                            });
                        }
                        std::fs::read_to_string(&canonical).map_err(|e| GlossError::Ingestion {
                            source_id: source.id.clone(),
                            message: format!("Failed to read file: {}", e),
                        })
                    }
                    Err(e) => {
                        std::fs::read_to_string(&full_path).map_err(|e2| GlossError::Ingestion {
                            source_id: source.id.clone(),
                            message: format!(
                                "Failed to read file (canonicalize: {}, read: {})",
                                e, e2
                            ),
                        })
                    }
                }
            } else if let Some(ref content) = source.content_text {
                Ok(content.clone())
            } else {
                Err(GlossError::Ingestion {
                    source_id: source.id.clone(),
                    message: "No file_path or content_text for text source".into(),
                })
            }?;
            Ok(ExtractedText {
                text,
                metadata: None,
            })
        }
        "paste" | "url" | "youtube" => {
            // Paste, URL, and YouTube transcript sources have content_text set directly.
            let text = source
                .content_text
                .clone()
                .ok_or_else(|| GlossError::Ingestion {
                    source_id: source.id.clone(),
                    message: format!("No content_text for {} source", source.source_type),
                })?;
            Ok(ExtractedText {
                text,
                metadata: None,
            })
        }
        "document" => extract_document_text(source, notebook_dir),
        "image" => {
            // Images cannot be extracted as text yet — requires vision model
            Ok(ExtractedText {
                text: format!("[Image file: {}]", source.title),
                metadata: None,
            })
        }
        "video" => {
            // Videos cannot be extracted as text yet — requires processing pipeline
            Ok(ExtractedText {
                text: format!("[Video file: {}]", source.title),
                metadata: None,
            })
        }
        _ => Err(GlossError::Ingestion {
            source_id: source.id.clone(),
            message: format!("Unsupported source type: {}", source.source_type),
        }),
    }
}

fn extract_document_text(
    source: &Source,
    notebook_dir: &Path,
) -> Result<ExtractedText, GlossError> {
    let file_path = source
        .file_path
        .as_ref()
        .ok_or_else(|| extraction_error(source, "document source has no file_path"))?;
    let untrusted_full_path = notebook_dir.join("sources").join(file_path);
    // Prevent path traversal: canonicalize and verify the path stays
    // within the notebook sources directory.
    let sources_dir = notebook_dir.join("sources");
    let full_path = match untrusted_full_path.canonicalize() {
        Ok(canonical) => {
            let canonical_root = match sources_dir.canonicalize() {
                Ok(root) => root,
                Err(e) => {
                    tracing::warn!(
                        "Cannot canonicalize sources dir {:?}: {} — rejecting path as unsafe",
                        sources_dir,
                        e
                    );
                    return Err(extraction_error(
                        source,
                        "Cannot verify source path safety (sources dir not canonicalizable)",
                    ));
                }
            };
            if !canonical.starts_with(&canonical_root) {
                return Err(extraction_error(
                    source,
                    "Path traversal detected in source file_path",
                ));
            }
            canonical
        }
        Err(e) => {
            tracing::debug!("Cannot canonicalize source path: {e}; attempting direct read");
            untrusted_full_path
        }
    };
    let format = source_document_format(source).ok_or_else(|| {
        extraction_error(
            source,
            "document source has no supported format metadata or extension",
        )
    })?;
    let (text, metadata) = match format.as_str() {
        "pdf" => (extract_pdf(source, &full_path)?, None),
        "docx" => (extract_docx(source, &full_path)?, None),
        "xlsx" => (extract_xlsx(source, &full_path)?, None),
        "pptx" => (extract_pptx(source, &full_path)?, None),
        "epub" => (extract_epub(source, &full_path)?, None),
        "doc" | "xls" | "ppt" => {
            let (text, receipt) = extract_legacy_office(source, &full_path, &format)?;
            let metadata = serde_json::json!({
                "schema": "DocumentExtractionMetadataV1",
                "legacy_office_extractor": receipt,
            });
            (text, Some(metadata))
        }
        other => Err(extraction_error(
            source,
            &format!("unsupported document extraction format: {other}"),
        ))?,
    };
    Ok(ExtractedText { text, metadata })
}

fn source_document_format(source: &Source) -> Option<String> {
    if let Some(metadata) = &source.metadata {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(metadata) {
            if let Some(language) = value.get("language").and_then(|v| v.as_str()) {
                if matches!(
                    language,
                    "pdf" | "docx" | "xlsx" | "pptx" | "epub" | "doc" | "xls" | "ppt"
                ) {
                    return Some(language.to_string());
                }
            }
        }
    }
    source
        .original_filename
        .as_ref()
        .or(source.file_path.as_ref())
        .and_then(|name| Path::new(name).extension())
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .filter(|ext| {
            matches!(
                ext.as_str(),
                "pdf" | "docx" | "xlsx" | "pptx" | "epub" | "doc" | "xls" | "ppt"
            )
        })
}

fn extract_pdf(source: &Source, path: &Path) -> Result<String, GlossError> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| extraction_error(source, &format!("failed to stat PDF: {e}")))?;
    if metadata.len() > MAX_PDF_BYTES {
        return Err(extraction_error(
            source,
            &format!(
                "PDF too large for bounded extraction: {} bytes, limit {}",
                metadata.len(),
                MAX_PDF_BYTES
            ),
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| extraction_error(source, &format!("failed to read PDF: {e}")))?;
    if bytes.len() as u64 > MAX_PDF_BYTES {
        return Err(extraction_error(source, "PDF exceeded bounded read limit"));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(extraction_error(source, "PDF header is missing or invalid"));
    }

    let extracted = catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(&bytes)
    }))
    .map_err(|_| extraction_error(source, "PDF extractor panicked on malformed input"))?
    .map_err(|e| extraction_error(source, &format!("PDF text extraction failed: {e}")))?;
    if extracted.len() > MAX_DOCUMENT_TEXT_CHARS {
        return Err(extraction_error(
            source,
            "PDF extracted text exceeds bounded output limit",
        ));
    }
    non_empty_document_text(source, "pdf", extracted)
}

fn extract_docx(source: &Source, path: &Path) -> Result<String, GlossError> {
    let mut archive = open_document_archive(source, path)?;
    require_entry(&mut archive, source, "[Content_Types].xml")?;
    let xml = read_zip_text_entry(&mut archive, source, "word/document.xml")?;
    let text = xml_text_nodes(source, &xml, &["t"], &[])?;
    non_empty_document_text(source, "docx", text)
}

fn extract_pptx(source: &Source, path: &Path) -> Result<String, GlossError> {
    let mut archive = open_document_archive(source, path)?;
    require_entry(&mut archive, source, "[Content_Types].xml")?;
    let mut slide_names = archive
        .file_names()
        .filter(|name| {
            name.starts_with("ppt/slides/slide")
                && name.ends_with(".xml")
                && is_safe_zip_entry_name(name)
        })
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    slide_names.sort();
    if slide_names.is_empty() {
        return Err(extraction_error(source, "pptx has no slide XML entries"));
    }

    let mut out = String::new();
    for (index, name) in slide_names.iter().enumerate() {
        let xml = read_zip_text_entry(&mut archive, source, name)?;
        append_document_line(source, &mut out, &format!("Slide {}", index + 1))?;
        let slide_text = xml_text_nodes(source, &xml, &["t"], &[])?;
        append_document_line(source, &mut out, &slide_text)?;
    }
    non_empty_document_text(source, "pptx", out)
}

fn extract_xlsx(source: &Source, path: &Path) -> Result<String, GlossError> {
    let mut archive = open_document_archive(source, path)?;
    require_entry(&mut archive, source, "[Content_Types].xml")?;
    let shared_strings = match read_zip_text_entry(&mut archive, source, "xl/sharedStrings.xml") {
        Ok(xml) => xml_text_list(source, &xml, &["t"], &[])?,
        Err(_) => Vec::new(),
    };

    let mut sheet_names = archive
        .file_names()
        .filter(|name| {
            name.starts_with("xl/worksheets/sheet")
                && name.ends_with(".xml")
                && is_safe_zip_entry_name(name)
        })
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    sheet_names.sort();
    if sheet_names.is_empty() {
        return Err(extraction_error(
            source,
            "xlsx has no worksheet XML entries",
        ));
    }

    let mut out = String::new();
    for (index, name) in sheet_names.iter().enumerate() {
        let xml = read_zip_text_entry(&mut archive, source, name)?;
        append_document_line(source, &mut out, &format!("Worksheet {}", index + 1))?;
        let values = xlsx_sheet_values(source, &xml, &shared_strings)?;
        append_document_line(source, &mut out, &values.join("\t"))?;
    }
    non_empty_document_text(source, "xlsx", out)
}

fn extract_epub(source: &Source, path: &Path) -> Result<String, GlossError> {
    let mut archive = open_document_archive(source, path)?;
    let mimetype = read_zip_text_entry(&mut archive, source, "mimetype")?;
    if mimetype.trim() != "application/epub+zip" {
        return Err(extraction_error(
            source,
            "epub mimetype is missing or invalid",
        ));
    }
    let container_xml = read_zip_text_entry(&mut archive, source, "META-INF/container.xml")?;
    let rootfile = first_attr_for_element(source, &container_xml, "rootfile", "full-path")?
        .ok_or_else(|| extraction_error(source, "epub container has no rootfile full-path"))?;
    if !is_safe_zip_entry_name(&rootfile) {
        return Err(extraction_error(source, "epub rootfile path is unsafe"));
    }
    let opf_xml = read_zip_text_entry(&mut archive, source, &rootfile)?;
    let item_paths = epub_spine_item_paths(source, &rootfile, &opf_xml)?;
    if item_paths.is_empty() {
        return Err(extraction_error(
            source,
            "epub spine has no readable XHTML items",
        ));
    }

    let mut out = String::new();
    for path in item_paths {
        let xml = read_zip_text_entry(&mut archive, source, &path)?;
        let text = xml_text_nodes(source, &xml, &[], &["script", "style"])?;
        append_document_line(source, &mut out, &text)?;
    }
    non_empty_document_text(source, "epub", out)
}

fn extract_legacy_office(
    source: &Source,
    path: &Path,
    format: &str,
) -> Result<(String, LegacyOfficeExtractorReceipt), GlossError> {
    let metadata = std::fs::metadata(path).map_err(|e| {
        extraction_error(
            source,
            &format!("failed to stat legacy Office document: {e}"),
        )
    })?;
    if metadata.len() > MAX_LEGACY_OFFICE_BYTES {
        return Err(extraction_error(
            source,
            &format!(
                "legacy Office document too large for bounded extraction: {} bytes, limit {}",
                metadata.len(),
                MAX_LEGACY_OFFICE_BYTES
            ),
        ));
    }

    let extractor = legacy_office_extractor_for_format(format).ok_or_else(|| {
        extraction_error(
            source,
            &format!("unsupported legacy Office extraction format: {format}"),
        )
    })?;
    let receipt_id = format!(
        "legacy-office-extract-{}-{}",
        source.id,
        uuid::Uuid::new_v4()
    );
    let stdout_path = std::env::temp_dir().join(format!("{receipt_id}.stdout"));
    let stderr_path = std::env::temp_dir().join(format!("{receipt_id}.stderr"));
    let stdout_file = File::create(&stdout_path).map_err(|e| {
        extraction_error(source, &format!("failed to create extractor stdout: {e}"))
    })?;
    let stderr_file = File::create(&stderr_path).map_err(|e| {
        extraction_error(source, &format!("failed to create extractor stderr: {e}"))
    })?;

    let start = Instant::now();
    let mut child = Command::new(extractor)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| {
            cleanup_temp_file(&stdout_path);
            cleanup_temp_file(&stderr_path);
            extraction_error(
                source,
                &format!("legacy Office extractor '{extractor}' is unavailable: {e}"),
            )
        })?;

    let wait_result = child
        .wait_timeout(Duration::from_millis(LEGACY_OFFICE_TIMEOUT_MS))
        .map_err(|e| {
            extraction_error(source, &format!("legacy Office extractor wait failed: {e}"))
        })?;
    let timed_out = wait_result.is_none();
    let exit_status = if let Some(status) = wait_result {
        status
    } else {
        let _ = child.kill();
        child.wait().map_err(|e| {
            extraction_error(
                source,
                &format!("legacy Office extractor kill wait failed: {e}"),
            )
        })?
    };
    let elapsed_ms = start.elapsed().as_millis();

    let stdout = read_bounded_temp_file(source, &stdout_path, MAX_DOCUMENT_TEXT_CHARS * 4)?;
    let stderr = read_bounded_temp_file(source, &stderr_path, 64 * 1024)?;
    cleanup_temp_file(&stdout_path);
    cleanup_temp_file(&stderr_path);

    let text = String::from_utf8_lossy(&stdout.bytes).into_owned();
    let stderr_text = String::from_utf8_lossy(&stderr.bytes).into_owned();
    let receipt = LegacyOfficeExtractorReceipt {
        schema: "LegacyOfficeExtractorReceiptV1",
        receipt_id,
        source_id: source.id.clone(),
        source_path_redacted: redact_path(path),
        format: format.to_string(),
        extractor,
        argv_redacted: vec![extractor.to_string(), "[source_document_path]".to_string()],
        timeout_ms: LEGACY_OFFICE_TIMEOUT_MS,
        elapsed_ms,
        exit_code: exit_status.code(),
        success: exit_status.success() && !timed_out,
        timed_out,
        stdout_sha256: sha256_hex(&stdout.bytes),
        stdout_bytes: stdout.bytes.len(),
        stderr_sha256: sha256_hex(&stderr.bytes),
        stderr_bytes: stderr.bytes.len(),
        stderr_preview_redacted: non_empty_preview(&stderr_text),
        output_truncated: stdout.truncated || stderr.truncated,
    };

    if timed_out {
        return Err(extraction_error(
            source,
            &format!("legacy Office extractor '{extractor}' timed out"),
        ));
    }
    if !exit_status.success() {
        return Err(extraction_error(
            source,
            &format!(
                "legacy Office extractor '{extractor}' failed with code {:?}: {}",
                exit_status.code(),
                non_empty_preview(&stderr_text).unwrap_or_else(|| "no stderr".to_string())
            ),
        ));
    }
    if stdout.truncated {
        return Err(extraction_error(
            source,
            "legacy Office extractor output exceeded bounded read limit",
        ));
    }

    Ok((non_empty_document_text(source, format, text)?, receipt))
}

fn legacy_office_extractor_for_format(format: &str) -> Option<&'static str> {
    match format {
        "doc" => Some("antiword"),
        "xls" => Some("xls2csv"),
        "ppt" => Some("catppt"),
        _ => None,
    }
}

struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_bounded_temp_file(
    source: &Source,
    path: &Path,
    max_bytes: usize,
) -> Result<BoundedBytes, GlossError> {
    let mut file = File::open(path)
        .map_err(|e| extraction_error(source, &format!("failed to open extractor output: {e}")))?;
    let mut bytes = Vec::new();
    let mut limited = file.by_ref().take(max_bytes as u64 + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| extraction_error(source, &format!("failed to read extractor output: {e}")))?;
    let truncated = bytes.len() > max_bytes;
    if truncated {
        bytes.truncate(max_bytes);
    }
    Ok(BoundedBytes { bytes, truncated })
}

fn cleanup_temp_file(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn non_empty_preview(text: &str) -> Option<String> {
    let preview = redact_text_paths(text)
        .chars()
        .take(500)
        .collect::<String>();
    let preview = preview.trim().to_string();
    if preview.is_empty() {
        None
    } else {
        Some(preview)
    }
}

fn open_document_archive(source: &Source, path: &Path) -> Result<ZipArchive<File>, GlossError> {
    let file = File::open(path).map_err(|e| {
        extraction_error(
            source,
            &format!("failed to open document archive for extraction: {e}"),
        )
    })?;
    let archive = ZipArchive::new(file).map_err(|e| {
        extraction_error(
            source,
            &format!("failed to read document ZIP container: {e}"),
        )
    })?;
    if archive.len() > MAX_DOCUMENT_ARCHIVE_ENTRIES {
        return Err(extraction_error(
            source,
            &format!(
                "document archive has too many entries: {} > {}",
                archive.len(),
                MAX_DOCUMENT_ARCHIVE_ENTRIES
            ),
        ));
    }
    for name in archive.file_names() {
        if !is_safe_zip_entry_name(name) {
            return Err(extraction_error(
                source,
                &format!("document archive contains unsafe entry name: {name}"),
            ));
        }
    }
    Ok(archive)
}

fn require_entry(
    archive: &mut ZipArchive<File>,
    source: &Source,
    name: &str,
) -> Result<(), GlossError> {
    archive.by_name(name).map(|_| ()).map_err(|_| {
        extraction_error(
            source,
            &format!("document archive missing required entry: {name}"),
        )
    })
}

fn read_zip_text_entry(
    archive: &mut ZipArchive<File>,
    source: &Source,
    name: &str,
) -> Result<String, GlossError> {
    if !is_safe_zip_entry_name(name) {
        return Err(extraction_error(source, "unsafe document entry requested"));
    }
    let mut entry = archive
        .by_name(name)
        .map_err(|e| extraction_error(source, &format!("document entry not found: {name}: {e}")))?;
    if entry.size() > MAX_DOCUMENT_XML_BYTES {
        return Err(extraction_error(
            source,
            &format!(
                "document entry too large: {name} has {} bytes, limit {}",
                entry.size(),
                MAX_DOCUMENT_XML_BYTES
            ),
        ));
    }
    let mut bytes = Vec::new();
    let mut limited = entry.by_ref().take(MAX_DOCUMENT_XML_BYTES + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| extraction_error(source, &format!("failed reading document entry: {e}")))?;
    if bytes.len() as u64 > MAX_DOCUMENT_XML_BYTES {
        return Err(extraction_error(
            source,
            &format!("document entry exceeded read limit: {name}"),
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        extraction_error(
            source,
            &format!("document entry is not UTF-8 XML/text: {name}"),
        )
    })
}

fn xml_text_nodes(
    source: &Source,
    xml: &str,
    target_names: &[&str],
    ignored_ancestors: &[&str],
) -> Result<String, GlossError> {
    let values = xml_text_list(source, xml, target_names, ignored_ancestors)?;
    let mut out = String::new();
    for value in values {
        append_document_text(source, &mut out, &value)?;
    }
    Ok(out)
}

fn xml_text_list(
    source: &Source,
    xml: &str,
    target_names: &[&str],
    ignored_ancestors: &[&str],
) -> Result<Vec<String>, GlossError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    let mut values = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                stack.push(local_xml_name(event.name().as_ref()));
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(event)) => {
                let current = stack.last().map(String::as_str);
                let target_matches = target_names.is_empty()
                    || current
                        .map(|name| target_names.contains(&name))
                        .unwrap_or(false);
                let ignored = stack
                    .iter()
                    .any(|name| ignored_ancestors.contains(&name.as_str()));
                if target_matches && !ignored {
                    let decoded = event
                        .decode()
                        .map_err(|e| extraction_error(source, &format!("invalid XML text: {e}")))?;
                    let normalized = collapse_inline_whitespace(decoded.as_ref());
                    if !normalized.is_empty() {
                        values.push(normalized);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(extraction_error(
                    source,
                    &format!("invalid document XML boundary: {e}"),
                ));
            }
            _ => {}
        }
    }
    Ok(values)
}

fn xlsx_sheet_values(
    source: &Source,
    xml: &str,
    shared_strings: &[String],
) -> Result<Vec<String>, GlossError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut values = Vec::new();
    let mut current_cell_type: Option<String> = None;
    let mut in_value = false;
    let mut value_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_xml_name(event.name().as_ref());
                if name == "c" {
                    current_cell_type = attr_value(&reader, &event, "t")?;
                } else if name == "v" || name == "t" {
                    in_value = true;
                    value_buf.clear();
                }
            }
            Ok(Event::End(event)) => {
                let name = local_xml_name(event.name().as_ref());
                if name == "v" || name == "t" {
                    in_value = false;
                    let raw = value_buf.trim();
                    if !raw.is_empty() {
                        if current_cell_type.as_deref() == Some("s") {
                            if let Ok(index) = raw.parse::<usize>() {
                                if let Some(value) = shared_strings.get(index) {
                                    values.push(value.clone());
                                }
                            }
                        } else {
                            values.push(collapse_inline_whitespace(raw));
                        }
                    }
                    value_buf.clear();
                } else if name == "c" {
                    current_cell_type = None;
                }
            }
            Ok(Event::Text(event)) if in_value => {
                let decoded = event.decode().map_err(|e| {
                    extraction_error(source, &format!("invalid worksheet text: {e}"))
                })?;
                value_buf.push_str(decoded.as_ref());
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(extraction_error(
                    source,
                    &format!("invalid worksheet XML boundary: {e}"),
                ));
            }
            _ => {}
        }
    }
    Ok(values)
}

fn first_attr_for_element(
    source: &Source,
    xml: &str,
    element_name: &str,
    attr_name: &str,
) -> Result<Option<String>, GlossError> {
    for attrs in attrs_for_element(source, xml, element_name)? {
        if let Some(value) = attrs.get(attr_name) {
            return Ok(Some(value.clone()));
        }
    }
    Ok(None)
}

fn epub_spine_item_paths(
    source: &Source,
    rootfile: &str,
    opf_xml: &str,
) -> Result<Vec<String>, GlossError> {
    let base_dir = rootfile.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let mut manifest = BTreeMap::new();
    for attrs in attrs_for_element(source, opf_xml, "item")? {
        let Some(id) = attrs.get("id") else { continue };
        let Some(href) = attrs.get("href") else {
            continue;
        };
        let media_type = attrs.get("media-type").map(String::as_str).unwrap_or("");
        if matches!(
            media_type,
            "application/xhtml+xml" | "text/html" | "application/xml"
        ) {
            if let Some(path) = safe_join_zip_path(base_dir, href) {
                manifest.insert(id.clone(), path);
            }
        }
    }

    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for attrs in attrs_for_element(source, opf_xml, "itemref")? {
        let Some(idref) = attrs.get("idref") else {
            continue;
        };
        if let Some(path) = manifest.get(idref) {
            if seen.insert(path.clone()) {
                paths.push(path.clone());
            }
        }
    }
    Ok(paths)
}

fn attrs_for_element(
    source: &Source,
    xml: &str,
    element_name: &str,
) -> Result<Vec<BTreeMap<String, String>>, GlossError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event))
                if local_xml_name(event.name().as_ref()) == element_name =>
            {
                let mut attrs = BTreeMap::new();
                for attr in event.attributes() {
                    let attr = attr.map_err(|e| {
                        extraction_error(source, &format!("invalid XML attribute: {e}"))
                    })?;
                    let key = local_xml_name(attr.key.as_ref());
                    let value = attr
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|e| {
                            extraction_error(source, &format!("invalid XML attribute value: {e}"))
                        })?
                        .into_owned();
                    attrs.insert(key, value);
                }
                out.push(attrs);
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(extraction_error(
                    source,
                    &format!("invalid XML attribute boundary: {e}"),
                ));
            }
            _ => {}
        }
    }
    Ok(out)
}

fn attr_value(
    reader: &Reader<&[u8]>,
    event: &quick_xml::events::BytesStart<'_>,
    attr_name: &str,
) -> Result<Option<String>, GlossError> {
    for attr in event.attributes() {
        let attr = attr.map_err(|e| GlossError::Ingestion {
            source_id: String::new(),
            message: format!("invalid worksheet attribute: {e}"),
        })?;
        if local_xml_name(attr.key.as_ref()) == attr_name {
            let value = attr
                .decode_and_unescape_value(reader.decoder())
                .map_err(|e| GlossError::Ingestion {
                    source_id: String::new(),
                    message: format!("invalid worksheet attribute value: {e}"),
                })?
                .into_owned();
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn safe_join_zip_path(base_dir: &str, href: &str) -> Option<String> {
    if href.is_empty() || href.starts_with('/') || href.contains('\\') || href.contains('\0') {
        return None;
    }
    let mut parts = base_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            other => parts.push(other.to_string()),
        }
    }
    let joined = parts.join("/");
    is_safe_zip_entry_name(&joined).then_some(joined)
}

fn is_safe_zip_entry_name(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains(':')
    {
        return false;
    }
    Path::new(name)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn local_xml_name(name: &[u8]) -> String {
    let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
    String::from_utf8_lossy(local).into_owned()
}

fn append_document_text(source: &Source, out: &mut String, text: &str) -> Result<(), GlossError> {
    let text = collapse_inline_whitespace(text);
    if text.is_empty() {
        return Ok(());
    }
    if !out.is_empty() && !out.ends_with(['\n', ' ']) {
        out.push(' ');
    }
    if out.len() + text.len() > MAX_DOCUMENT_TEXT_CHARS {
        return Err(extraction_error(
            source,
            "document extracted text exceeds bounded output limit",
        ));
    }
    out.push_str(&text);
    Ok(())
}

fn append_document_line(source: &Source, out: &mut String, text: &str) -> Result<(), GlossError> {
    let text = collapse_inline_whitespace(text);
    if text.is_empty() {
        return Ok(());
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if out.len() + text.len() + 1 > MAX_DOCUMENT_TEXT_CHARS {
        return Err(extraction_error(
            source,
            "document extracted text exceeds bounded output limit",
        ));
    }
    out.push_str(&text);
    out.push('\n');
    Ok(())
}

fn collapse_inline_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn non_empty_document_text(
    source: &Source,
    format: &str,
    text: String,
) -> Result<String, GlossError> {
    let text = text.trim().to_string();
    if text.is_empty() {
        Err(extraction_error(
            source,
            &format!("{format} extractor found no source text"),
        ))
    } else {
        Ok(text)
    }
}

fn extraction_error(source: &Source, message: &str) -> GlossError {
    GlossError::Ingestion {
        source_id: source.id.clone(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::notebook_db::Source;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn source_for(filename: &str, language: &str) -> Source {
        Source {
            id: format!("source-{language}"),
            source_type: "document".to_string(),
            title: filename.to_string(),
            original_filename: Some(filename.to_string()),
            file_hash: None,
            url: None,
            file_path: Some(filename.to_string()),
            content_text: None,
            word_count: None,
            metadata: Some(serde_json::json!({ "language": language }).to_string()),
            summary: None,
            summary_model: None,
            status: "pending".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
            processing_state: None,
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, contents) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn minimal_pdf(text: &str) -> Vec<u8> {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let content = format!("BT /F1 24 Tf 72 720 Td ({escaped}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = vec![0usize];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
        }
        let xref_offset = pdf.len();
        pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        ));
        pdf.into_bytes()
    }

    #[test]
    fn extracts_pdf_docx_pptx_xlsx_epub_from_strict_boundaries() {
        let dir = tempdir().unwrap();
        let sources = dir.path().join("sources");
        fs::create_dir(&sources).unwrap();

        fs::write(sources.join("sample.pdf"), minimal_pdf("Omega PDF text")).unwrap();
        let pdf = extract_text(&source_for("sample.pdf", "pdf"), dir.path()).unwrap();
        assert!(pdf.contains("Omega PDF text"));

        write_zip(
            &sources.join("sample.docx"),
            &[
                ("[Content_Types].xml", "<Types/>"),
                (
                    "word/document.xml",
                    r#"<w:document><w:body><w:p><w:r><w:t>Alpha docx text</w:t></w:r></w:p></w:body></w:document>"#,
                ),
            ],
        );
        let docx = extract_text(&source_for("sample.docx", "docx"), dir.path()).unwrap();
        assert!(docx.contains("Alpha docx text"));

        write_zip(
            &sources.join("sample.pptx"),
            &[
                ("[Content_Types].xml", "<Types/>"),
                (
                    "ppt/slides/slide1.xml",
                    r#"<p:sld><p:cSld><a:t>Beta slide text</a:t></p:cSld></p:sld>"#,
                ),
            ],
        );
        let pptx = extract_text(&source_for("sample.pptx", "pptx"), dir.path()).unwrap();
        assert!(pptx.contains("Slide 1"));
        assert!(pptx.contains("Beta slide text"));

        write_zip(
            &sources.join("sample.xlsx"),
            &[
                ("[Content_Types].xml", "<Types/>"),
                (
                    "xl/sharedStrings.xml",
                    r#"<sst><si><t>Gamma header</t></si><si><t>Delta cell</t></si></sst>"#,
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet><sheetData><row><c t="s"><v>0</v></c><c t="s"><v>1</v></c><c><v>42</v></c></row></sheetData></worksheet>"#,
                ),
            ],
        );
        let xlsx = extract_text(&source_for("sample.xlsx", "xlsx"), dir.path()).unwrap();
        assert!(xlsx.contains("Gamma header"));
        assert!(xlsx.contains("Delta cell"));
        assert!(xlsx.contains("42"));

        write_zip(
            &sources.join("sample.epub"),
            &[
                ("mimetype", "application/epub+zip"),
                (
                    "META-INF/container.xml",
                    r#"<container><rootfiles><rootfile full-path="OPS/package.opf"/></rootfiles></container>"#,
                ),
                (
                    "OPS/package.opf",
                    r#"<package><manifest><item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
                ),
                (
                    "OPS/chapter1.xhtml",
                    r#"<html><body><h1>Epsilon title</h1><p>Zeta epub text</p></body></html>"#,
                ),
            ],
        );
        let epub = extract_text(&source_for("sample.epub", "epub"), dir.path()).unwrap();
        assert!(epub.contains("Epsilon title"));
        assert!(epub.contains("Zeta epub text"));
    }

    #[test]
    fn malformed_docx_fails_instead_of_widening_to_plain_text() {
        let dir = tempdir().unwrap();
        let sources = dir.path().join("sources");
        fs::create_dir(&sources).unwrap();
        write_zip(
            &sources.join("bad.docx"),
            &[("[Content_Types].xml", "<Types/>")],
        );

        let err = extract_text(&source_for("bad.docx", "docx"), dir.path()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("word/document.xml") || message.contains("document entry"));
    }

    #[test]
    fn malformed_pdf_fails_instead_of_widening_to_plain_text() {
        let dir = tempdir().unwrap();
        let sources = dir.path().join("sources");
        fs::create_dir(&sources).unwrap();
        fs::write(sources.join("bad.pdf"), b"not a pdf with plain text").unwrap();

        let err = extract_text(&source_for("bad.pdf", "pdf"), dir.path()).unwrap_err();
        assert!(err.to_string().contains("PDF header"));
    }

    #[test]
    fn legacy_office_extractors_have_strict_tool_mapping() {
        assert_eq!(legacy_office_extractor_for_format("doc"), Some("antiword"));
        assert_eq!(legacy_office_extractor_for_format("xls"), Some("xls2csv"));
        assert_eq!(legacy_office_extractor_for_format("ppt"), Some("catppt"));
        assert_eq!(legacy_office_extractor_for_format("docx"), None);
        assert_eq!(legacy_office_extractor_for_format("txt"), None);
    }

    #[test]
    fn legacy_office_receipt_redacts_paths_and_records_bounds() {
        let source = source_for("sample.doc", "doc");
        let receipt = LegacyOfficeExtractorReceipt {
            schema: "LegacyOfficeExtractorReceiptV1",
            receipt_id: "receipt".to_string(),
            source_id: source.id.clone(),
            source_path_redacted: redact_path(Path::new("/home/example/private/sample.doc")),
            format: "doc".to_string(),
            extractor: "antiword",
            argv_redacted: vec!["antiword".to_string(), "[source_document_path]".to_string()],
            timeout_ms: LEGACY_OFFICE_TIMEOUT_MS,
            elapsed_ms: 10,
            exit_code: Some(0),
            success: true,
            timed_out: false,
            stdout_sha256: sha256_hex(b"legacy text"),
            stdout_bytes: "legacy text".len(),
            stderr_sha256: sha256_hex(b""),
            stderr_bytes: 0,
            stderr_preview_redacted: non_empty_preview("warning /home/example/private/sample.doc"),
            output_truncated: false,
        };
        let value = serde_json::to_value(receipt).unwrap();
        assert_eq!(value["schema"], "LegacyOfficeExtractorReceiptV1");
        assert_eq!(value["argv_redacted"][1], "[source_document_path]");
        assert_eq!(value["timeout_ms"], LEGACY_OFFICE_TIMEOUT_MS);
        assert!(!value.to_string().contains("/home/example/private"));
    }
}
