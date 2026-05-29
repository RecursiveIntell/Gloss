use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ImportSupport {
    Supported,
    SupportedDegraded,
    Deferred,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportCapability {
    pub key: &'static str,
    pub label: &'static str,
    pub extensions: &'static [&'static str],
    pub source_type: Option<&'static str>,
    pub language: Option<&'static str>,
    pub support: ImportSupport,
    pub receipt_schema: &'static str,
    pub reason: &'static str,
}

impl ImportCapability {
    pub fn is_importable(&self) -> bool {
        matches!(
            self.support,
            ImportSupport::Supported | ImportSupport::SupportedDegraded
        )
    }
}

const TEXT_EXTENSIONS: &[&str] = &["txt"];
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "rst"];
const CSV_EXTENSIONS: &[&str] = &["csv", "tsv"];
const HTML_EXTENSIONS: &[&str] = &["html", "htm"];
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "mov", "avi", "mkv"];
const PDF_EXTENSIONS: &[&str] = &["pdf"];
const DOCX_EXTENSIONS: &[&str] = &["docx"];
const DOC_EXTENSIONS: &[&str] = &["doc"];
const XLSX_EXTENSIONS: &[&str] = &["xlsx"];
const XLS_EXTENSIONS: &[&str] = &["xls"];
const PPTX_EXTENSIONS: &[&str] = &["pptx"];
const PPT_EXTENSIONS: &[&str] = &["ppt"];
const EPUB_EXTENSIONS: &[&str] = &["epub"];
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "ogg", "flac", "m4a", "aac", "wma"];
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst"];
const BINARY_EXTENSIONS: &[&str] = &[
    "o",
    "obj",
    "so",
    "dll",
    "dylib",
    "a",
    "lib",
    "exe",
    "bin",
    "elf",
    "class",
    "pyc",
    "pyo",
    "wasm",
    "ico",
    "ttf",
    "otf",
    "woff",
    "woff2",
    "eot",
    "db",
    "sqlite",
    "sqlite3",
    "mdb",
    "swp",
    "swo",
    "onnx",
    "pt",
    "pth",
    "safetensors",
    "gguf",
    "ggml",
    "usearch",
    "lock",
];

const CODE_EXTENSIONS: &[(&str, &str)] = &[
    ("py", "python"),
    ("js", "javascript"),
    ("jsx", "jsx"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("rs", "rust"),
    ("go", "go"),
    ("java", "java"),
    ("c", "c"),
    ("cpp", "cpp"),
    ("cc", "cpp"),
    ("cxx", "cpp"),
    ("h", "c_header"),
    ("hpp", "c_header"),
    ("cs", "csharp"),
    ("rb", "ruby"),
    ("php", "php"),
    ("swift", "swift"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("scala", "scala"),
    ("lua", "lua"),
    ("r", "r"),
    ("sql", "sql"),
    ("sh", "shell"),
    ("bash", "shell"),
    ("zsh", "shell"),
    ("css", "css"),
    ("scss", "scss"),
    ("sass", "scss"),
    ("xml", "xml"),
    ("json", "json"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("toml", "toml"),
    ("ini", "config"),
    ("cfg", "config"),
    ("conf", "config"),
    ("vue", "vue"),
    ("svelte", "svelte"),
    ("dart", "dart"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("zig", "zig"),
    ("nim", "nim"),
    ("pl", "perl"),
    ("pm", "perl"),
    ("proto", "protobuf"),
    ("graphql", "graphql"),
    ("gql", "graphql"),
    ("tf", "terraform"),
    ("hcl", "terraform"),
    ("dockerfile", "dockerfile"),
    ("makefile", "makefile"),
    ("svg", "xml"),
];

pub fn import_capability_matrix() -> Vec<ImportCapability> {
    vec![
        ImportCapability {
            key: "text",
            label: "Text files",
            extensions: TEXT_EXTENSIONS,
            source_type: Some("text"),
            language: None,
            support: ImportSupport::Supported,
            receipt_schema: "ExtractionReceiptV1",
            reason: "UTF-8 text extraction is implemented.",
        },
        ImportCapability {
            key: "markdown",
            label: "Markdown/reStructuredText",
            extensions: MARKDOWN_EXTENSIONS,
            source_type: Some("markdown"),
            language: None,
            support: ImportSupport::Supported,
            receipt_schema: "ExtractionReceiptV1",
            reason: "Markdown text extraction and heading-aware chunking are implemented.",
        },
        ImportCapability {
            key: "code",
            label: "Code/config files",
            extensions: &[],
            source_type: Some("code"),
            language: None,
            support: ImportSupport::Supported,
            receipt_schema: "ExtractionReceiptV1",
            reason: "Recognized code/config extensions are imported as UTF-8 text with language metadata.",
        },
        ImportCapability {
            key: "paste",
            label: "Paste",
            extensions: &[],
            source_type: Some("paste"),
            language: None,
            support: ImportSupport::Supported,
            receipt_schema: "ExtractionReceiptV1",
            reason: "Pasted text is stored directly as source content.",
        },
        ImportCapability {
            key: "csv",
            label: "CSV/TSV",
            extensions: CSV_EXTENSIONS,
            source_type: Some("text"),
            language: Some("csv"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "ExtractionReceiptV1",
            reason: "CSV/TSV import is plain text only; table normalization is not implemented.",
        },
        ImportCapability {
            key: "html_file",
            label: "HTML file",
            extensions: HTML_EXTENSIONS,
            source_type: Some("code"),
            language: Some("html"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "ExtractionReceiptV1",
            reason: "HTML files import as source text; readability extraction is not implemented.",
        },
        ImportCapability {
            key: "image",
            label: "Image OCR/vision",
            extensions: IMAGE_EXTENSIONS,
            source_type: Some("image"),
            language: None,
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "VisionJobReceiptV1",
            reason: "Image files are routed to the vision job pipeline; OCR/vision quality depends on configured model capability.",
        },
        ImportCapability {
            key: "video",
            label: "Video metadata/frame vision",
            extensions: VIDEO_EXTENSIONS,
            source_type: Some("video"),
            language: None,
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "ToolInvocationReceiptV1",
            reason: "Video files are routed through ffmpeg/ffprobe receipts; transcription and full video understanding are not implemented.",
        },
        ImportCapability {
            key: "pdf",
            label: "PDF",
            extensions: PDF_EXTENSIONS,
            source_type: Some("document"),
            language: Some("pdf"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "DocumentExtractorReceiptV1",
            reason: "PDF files are extracted locally with a bounded pure-Rust text extractor; OCR, forms, and layout fidelity are not implemented.",
        },
        ImportCapability {
            key: "docx",
            label: "DOCX",
            extensions: DOCX_EXTENSIONS,
            source_type: Some("document"),
            language: Some("docx"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "DocumentExtractorReceiptV1",
            reason: "DOCX files are extracted locally from bounded OOXML ZIP/XML text nodes.",
        },
        ImportCapability {
            key: "xlsx",
            label: "XLSX",
            extensions: XLSX_EXTENSIONS,
            source_type: Some("document"),
            language: Some("xlsx"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "DocumentExtractorReceiptV1",
            reason: "XLSX files are extracted locally from bounded OOXML ZIP/XML shared strings and worksheet values.",
        },
        ImportCapability {
            key: "pptx",
            label: "PPTX",
            extensions: PPTX_EXTENSIONS,
            source_type: Some("document"),
            language: Some("pptx"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "DocumentExtractorReceiptV1",
            reason: "PPTX files are extracted locally from bounded OOXML ZIP/XML slide text nodes.",
        },
        ImportCapability {
            key: "epub",
            label: "EPUB",
            extensions: EPUB_EXTENSIONS,
            source_type: Some("document"),
            language: Some("epub"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "DocumentExtractorReceiptV1",
            reason: "EPUB files are extracted locally from bounded ZIP/XML spine XHTML text nodes.",
        },
        ImportCapability {
            key: "doc",
            label: "Legacy DOC",
            extensions: DOC_EXTENSIONS,
            source_type: Some("document"),
            language: Some("doc"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "LegacyOfficeExtractorReceiptV1",
            reason: "Legacy .doc files are extracted through the local antiword CLI with timeout, size, output, and redacted tool-receipt bounds; layout fidelity is not claimed.",
        },
        ImportCapability {
            key: "xls",
            label: "Legacy XLS",
            extensions: XLS_EXTENSIONS,
            source_type: Some("document"),
            language: Some("xls"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "LegacyOfficeExtractorReceiptV1",
            reason: "Legacy .xls files are extracted through the local xls2csv CLI with timeout, size, output, and redacted tool-receipt bounds; workbook fidelity is not claimed.",
        },
        ImportCapability {
            key: "ppt",
            label: "Legacy PPT",
            extensions: PPT_EXTENSIONS,
            source_type: Some("document"),
            language: Some("ppt"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "LegacyOfficeExtractorReceiptV1",
            reason: "Legacy .ppt files are extracted through the local catppt CLI with timeout, size, output, and redacted tool-receipt bounds; slide layout fidelity is not claimed.",
        },
        ImportCapability {
            key: "url",
            label: "URL import",
            extensions: &[],
            source_type: Some("url"),
            language: Some("html"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "UrlImportReceiptV1",
            reason: "URL import performs one user-consented HTTP(S) fetch with public-host, redirect, content-type, and byte limits; no crawling or authenticated fetching is implemented.",
        },
        ImportCapability {
            key: "youtube_transcript",
            label: "YouTube transcript",
            extensions: &[],
            source_type: Some("youtube"),
            language: Some("transcript"),
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "YouTubeTranscriptReceiptV1",
            reason: "YouTube transcript import fetches only YouTube caption tracks from public YouTube watch URLs with explicit per-import network consent; no video download, authenticated access, or scraping beyond caption metadata is implemented.",
        },
        ImportCapability {
            key: "audio",
            label: "Audio metadata",
            extensions: AUDIO_EXTENSIONS,
            source_type: Some("audio"),
            language: None,
            support: ImportSupport::SupportedDegraded,
            receipt_schema: "AudioMetadataReceiptV1; AudioTranscriptionReceiptV1",
            reason: "Audio files are routed through ffprobe metadata extraction and optional cached Whisper CLI transcription with ToolInvocationReceiptV1; transcription is skipped unless a local Whisper model is already cached.",
        },
    ]
}

pub fn classify_import_extension(ext: &str) -> ImportCapability {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let ext = ext.as_str();

    if ext.is_empty() {
        return ImportCapability {
            key: "extensionless_text",
            label: "Extensionless text",
            extensions: &[],
            source_type: Some("text"),
            language: None,
            support: ImportSupport::Supported,
            receipt_schema: "ExtractionReceiptV1",
            reason: "Extensionless files are accepted for local-first source trees such as LICENSE, Dockerfile, and Makefile.",
        };
    }

    if TEXT_EXTENSIONS.contains(&ext) {
        return import_capability_matrix()[0].clone();
    }
    if MARKDOWN_EXTENSIONS.contains(&ext) {
        return import_capability_matrix()[1].clone();
    }
    if CSV_EXTENSIONS.contains(&ext) {
        return import_capability_matrix()[4].clone();
    }
    if HTML_EXTENSIONS.contains(&ext) {
        return import_capability_matrix()[5].clone();
    }
    if IMAGE_EXTENSIONS.contains(&ext) {
        return import_capability_matrix()[6].clone();
    }
    if VIDEO_EXTENSIONS.contains(&ext) {
        return import_capability_matrix()[7].clone();
    }
    for (code_ext, language) in CODE_EXTENSIONS {
        if ext == *code_ext {
            return ImportCapability {
                key: "code",
                label: "Code/config files",
                extensions: &[],
                source_type: Some("code"),
                language: Some(language),
                support: ImportSupport::Supported,
                receipt_schema: "ExtractionReceiptV1",
                reason: "Recognized code/config extensions are imported as UTF-8 text with language metadata.",
            };
        }
    }

    for capability in import_capability_matrix() {
        if capability.extensions.contains(&ext) {
            return capability;
        }
    }

    let (key, label, reason) = if ARCHIVE_EXTENSIONS.contains(&ext) {
        (
            "unsupported_archive",
            "Unsupported archive",
            "Archive extraction is not implemented and is never silently widened to text import.",
        )
    } else if BINARY_EXTENSIONS.contains(&ext) {
        (
            "unsupported_binary",
            "Unsupported binary",
            "Binary formats are not importable source text and are never silently widened to text import.",
        )
    } else {
        (
            "unknown_extension",
            "Unknown extension",
            "Unknown extensions are not silently widened to text import.",
        )
    };

    ImportCapability {
        key,
        label,
        extensions: &[],
        source_type: None,
        language: None,
        support: ImportSupport::Unsupported,
        receipt_schema: "UnsupportedCapabilityReceiptV1",
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_import_extension, import_capability_matrix, ImportSupport};

    #[test]
    fn unknown_extensions_do_not_silently_import_as_text() {
        let capability = classify_import_extension("mystery");
        assert_eq!(capability.support, ImportSupport::Unsupported);
        assert_eq!(capability.source_type, None);
        assert!(capability.reason.contains("not silently widened"));
    }

    #[test]
    fn broad_spec_formats_are_explicitly_deferred_or_degraded() {
        for ext in ["doc", "xls", "ppt"] {
            let capability = classify_import_extension(ext);
            assert_eq!(
                capability.support,
                ImportSupport::SupportedDegraded,
                "{ext}"
            );
            assert_eq!(capability.source_type, Some("document"), "{ext}");
            assert_eq!(capability.receipt_schema, "LegacyOfficeExtractorReceiptV1");
        }

        for ext in ["pdf", "docx", "xlsx", "pptx", "epub"] {
            let capability = classify_import_extension(ext);
            assert_eq!(
                capability.support,
                ImportSupport::SupportedDegraded,
                "{ext}"
            );
            assert_eq!(capability.source_type, Some("document"), "{ext}");
            assert_eq!(capability.receipt_schema, "DocumentExtractorReceiptV1");
        }

        assert_eq!(
            classify_import_extension("csv").support,
            ImportSupport::SupportedDegraded
        );
        assert_eq!(
            classify_import_extension("html").support,
            ImportSupport::SupportedDegraded
        );
        assert_eq!(
            classify_import_extension("mp4").support,
            ImportSupport::SupportedDegraded
        );
        assert_eq!(
            classify_import_extension("mp3").support,
            ImportSupport::SupportedDegraded
        );
        let youtube = import_capability_matrix()
            .into_iter()
            .find(|entry| entry.key == "youtube_transcript")
            .unwrap();
        assert_eq!(youtube.support, ImportSupport::SupportedDegraded);
        assert_eq!(youtube.source_type, Some("youtube"));
        assert_eq!(youtube.receipt_schema, "YouTubeTranscriptReceiptV1");
    }

    #[test]
    fn capability_matrix_names_every_requested_broad_boundary() {
        let keys = import_capability_matrix()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>();
        for key in [
            "text",
            "markdown",
            "code",
            "paste",
            "csv",
            "html_file",
            "pdf",
            "docx",
            "doc",
            "xlsx",
            "xls",
            "pptx",
            "ppt",
            "epub",
            "url",
            "youtube_transcript",
            "image",
            "audio",
            "video",
        ] {
            assert!(keys.contains(&key), "{key}");
        }
    }
}
