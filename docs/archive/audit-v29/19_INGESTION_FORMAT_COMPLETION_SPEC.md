# Ingestion Format Completion Spec

## RC ingestion scope

- text, markdown, code, paste, folder import only.
- fix terminal counts, source lifecycle, dense/projection backfill receipts.

## Broad ingestion after RC

| Format | Planned approach | Receipt |
|---|---|---|
| PDF | Evaluate `pdf-extract`, `lopdf`, or system Poppler wrapper; preserve page/spans where possible | `PdfExtractionReceiptV1` |
| DOCX | ZIP + XML or vetted crate; preserve paragraphs/tables | `DocxExtractionReceiptV1` |
| XLSX | `calamine`; sheet/range metadata | `SpreadsheetExtractionReceiptV1` |
| CSV | Rust `csv`; dialect and table chunks | `CsvExtractionReceiptV1` |
| PPTX | ZIP + XML; slide order, notes | `PptxExtractionReceiptV1` |
| EPUB | EPUB crate + HTML normalizer | `EpubExtractionReceiptV1` |
| HTML/URL | opt-in network fetch + readability/scraper | `WebFetchReceiptV1` |
| YouTube | opt-in transcript fetch/provider | `TranscriptFetchReceiptV1` |
| Audio | whisper-rs + ffmpeg | `TranscriptionReceiptV1` |
| Image/OCR | defer or explicit OCR provider | `OcrReceiptV1` |

Do not begin broad ingestion until RC proof is complete.
