#!/usr/bin/env node
import { readFileSync } from "node:fs";

const checks = [];

function check(name, condition, detail) {
  checks.push({ name, pass: Boolean(condition), detail });
}

const types = readFileSync("src/lib/types.ts", "utf8");
const chatPanel = readFileSync("src/components/chat/ChatPanel.tsx", "utf8");
const notebookSidebar = readFileSync("src/components/notebooks/NotebookSidebar.tsx", "utf8");
const sourcesPanel = readFileSync("src/components/sources/SourcesPanel.tsx", "utf8");
const settingsDialog = readFileSync("src/components/settings/SettingsDialog/index.tsx", "utf8");
const studioPanel = readFileSync("src/components/studio/StudioPanel.tsx", "utf8");
const studioStore = readFileSync("src/stores/studioStore.ts", "utf8");
const panelLayout = readFileSync("src/components/layout/PanelLayout.tsx", "utf8");
const context = readFileSync("src-tauri/src/retrieval/context.rs", "utf8");

check(
  "message citations use evidence envelope",
  /citations\?: ChatEvidencePayload;/.test(types),
  "Message.citations must be a single ChatEvidencePayload shape."
);
check(
  "note citations are citation array only",
  /citations\?: Citation\[\];/.test(types),
  "Note.citations must not accept JSON strings or evidence envelopes."
);
check(
  "no citation union types",
  !/citations\?:[^\n|]*\|/.test(types),
  "citations fields must not be polymorphic unions."
);
check(
  "unknown evidence defaults are not marked preserved",
  /source_scope_preserved: false,/.test(chatPanel),
  "Null evidence must not claim source scope was preserved."
);
check(
  "system prompt excludes quoted passage payloads",
  !/Retrieved passages|lives in the system message/.test(context),
  "Source passage text must be assembled outside the system prompt."
);
check(
  "user turn wraps source data",
  /SOURCE_DATA/.test(context) && /Treat the following blocks as quoted source data/.test(context),
  "Source passages must be wrapped as quoted data in the current user turn."
);
check(
  "notebook portability UI validates before import",
  /validateNotebookImportArchive/.test(notebookSidebar) && /importNotebookArchive/.test(notebookSidebar),
  "Notebook archive import must validate manifest/hash evidence before importing."
);
check(
  "notebook portability UI exposes export receipt path",
  /exportNotebookArchive/.test(notebookSidebar) && /file_count/.test(notebookSidebar),
  "Notebook archive export UI must call the backend receipt-bearing export command."
);
check(
  "DB doctor UI can run check and repair",
  /handleRunDatabaseDoctor\(false\)/.test(settingsDialog) &&
    /handleRunDatabaseDoctor\(true\)/.test(settingsDialog) &&
    /handleCheckDatabaseDoctor/.test(settingsDialog) &&
    /handleRepairDatabaseDoctor/.test(settingsDialog) &&
    /dbDoctorReceipt/.test(settingsDialog) &&
    /repaired_source_count_mismatches/.test(settingsDialog) &&
    /repaired_orphan_rows/.test(settingsDialog) &&
    /quarantined_failed_import_sources/.test(settingsDialog) &&
    /repaired_stale_queue_jobs/.test(settingsDialog),
  "Settings diagnostics must expose DB doctor check and repair receipt fields."
);
check(
  "failed import quarantine UI exposes review/quarantine/delete workflow",
  /Failed imports/.test(sourcesPanel) &&
    /quarantineFailedImports/.test(sourcesPanel) &&
    /deleteFailedImports/.test(sourcesPanel) &&
    /setStatusFilter\("error"\)/.test(sourcesPanel) &&
    /Delete Failed/.test(sourcesPanel),
  "Sources panel must expose a dedicated failed-import review/quarantine/delete workflow."
);
check(
  "YouTube transcript UI exposes consented transcript import",
  /Add YouTube Transcript/.test(sourcesPanel) &&
    /addSourceYouTubeTranscript/.test(sourcesPanel) &&
    /YouTube transcript fetch/.test(sourcesPanel) &&
    /video download/.test(sourcesPanel) &&
    /authenticated YouTube access/.test(sourcesPanel),
  "Sources panel must expose consented YouTube transcript import without claiming video download or authenticated access."
);
check(
  "Studio UI exposes generation and export workflow",
  /StudioPanel/.test(panelLayout) &&
    /generateOutput/.test(studioPanel) &&
    /exportOutput/.test(studioPanel) &&
    /StudioExportReceiptV1/.test(types) &&
    /exportStudioOutput/.test(studioStore),
  "Studio panel must expose artifact generation, history, proof fields, and receipt-bearing export."
);

const failed = checks.filter((item) => !item.pass);
if (failed.length) {
  console.error(JSON.stringify({ status: "fail", failed }, null, 2));
  process.exit(1);
}

console.log(JSON.stringify({ status: "pass", checks: checks.map(({ name }) => name) }, null, 2));
