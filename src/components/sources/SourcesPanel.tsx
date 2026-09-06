import { memo, useState, useMemo, useCallback } from "react";
import { useSourceStore } from "../../stores/sourceStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../../lib/tauri";
import type { Source } from "../../lib/types";
import { featureById } from "../../lib/features";
import { Virtuoso } from "react-virtuoso";
import {
  FileText,
  Upload,
  FolderOpen,
  ClipboardPaste,
  Code,
  Image,
  Link,
  Music,
  Video,
  Trash2,
  CheckSquare,
  Square,
  ChevronRight,
  AlertCircle,
  Search,
  Layers,
  RotateCcw,
} from "lucide-react";

interface SourcesPanelProps {
  notebookId: string;
}

type SourceStoreState = ReturnType<typeof useSourceStore.getState>;

const SUPPORTED_EXTENSIONS = [
  "txt", "md", "markdown", "rst",
  "csv", "tsv", "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "epub",
  "py", "js", "jsx", "ts", "tsx", "rs", "go", "java", "c", "cpp", "cc", "cxx",
  "h", "hpp", "cs", "rb", "php", "swift", "kt", "kts", "scala", "lua", "r",
  "sql", "sh", "bash", "zsh", "css", "scss", "sass", "html", "htm", "xml",
  "json", "yaml", "yml", "toml", "ini", "cfg", "conf", "vue", "svelte",
  "dart", "ex", "exs", "zig", "nim", "pl", "pm", "proto", "graphql", "gql",
  "tf", "hcl", "dockerfile", "makefile",
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tiff", "tif",
  "mp3", "wav", "ogg", "flac", "m4a", "aac", "wma",
  "mp4", "webm", "mov", "avi", "mkv",
];

function sourceIcon(sourceType: string) {
  switch (sourceType) {
    case "code":
      return <Code className="w-4 h-4 text-text-muted shrink-0" />;
    case "image":
      return <Image className="w-4 h-4 text-text-muted shrink-0" />;
    case "audio":
      return <Music className="w-4 h-4 text-text-muted shrink-0" />;
    case "video":
      return <Video className="w-4 h-4 text-text-muted shrink-0" />;
    case "paste":
      return <ClipboardPaste className="w-4 h-4 text-text-muted shrink-0" />;
    case "url":
      return <Link className="w-4 h-4 text-text-muted shrink-0" />;
    default:
      return <FileText className="w-4 h-4 text-text-muted shrink-0" />;
  }
}

function groupSources(sources: Source[]): Map<string, Source[]> {
  const groups = new Map<string, Source[]>();
  for (const source of sources) {
    const parts = source.title.split("/");
    const group = parts.length > 1 ? parts[0] : "(ungrouped)";
    if (!groups.has(group)) groups.set(group, []);
    groups.get(group)!.push(source);
  }
  return groups;
}

type SourceListRow =
  | {
      kind: "group";
      id: string;
      group: string;
      sourceCount: number;
      isCollapsed: boolean;
      allSelected: boolean;
    }
  | {
      kind: "source";
      id: string;
      source: Source;
    };

function sourceTitle(source: Source, grouped: boolean) {
  if (grouped) {
    const parts = source.title.split("/");
    return parts.length > 1 ? parts.slice(1).join("/") : source.title;
  }
  return source.title;
}

function statusColor(status: string) {
  switch (status) {
    case "ready":
      return "text-success";
    case "error":
      return "text-error";
    case "pending":
      return "text-warning";
    case "describing":
    case "described":
      return "text-accent";
    default:
      return "text-accent";
  }
}

function statusNote(source: Source) {
  if (source.source_type === "audio") {
    if (source.status === "pending") return " · Queued for metadata extraction";
    if (source.status === "describing") return " · Extracting metadata...";
  }
  if (source.source_type === "image" || source.source_type === "video") {
    if (source.status === "pending") return " · Queued for vision analysis";
    if (source.status === "describing") return " · Describing with vision model...";
    if (source.status === "described") return " · Embedding...";
  }
  return "";
}

const SourceListItem = memo(function SourceListItem({
  source,
  isSelected,
  grouped,
  onToggle,
  onRetry,
  onReindex,
  onDelete,
}: {
  source: Source;
  isSelected: boolean;
  grouped: boolean;
  onToggle: (sourceId: string) => void;
  onRetry: (sourceId: string) => void;
  onReindex: (sourceId: string) => void;
  onDelete: (sourceId: string) => void;
}) {
  return (
    <div className="group flex items-center gap-2 px-2 py-1.5 rounded hover:bg-bg-tertiary">
      <button
        onClick={() => onToggle(source.id)}
        className="shrink-0"
        aria-label={`${isSelected ? "Deselect" : "Select"} ${source.title}`}
      >
        {isSelected ? (
          <CheckSquare className="w-4 h-4 text-accent" />
        ) : (
          <Square className="w-4 h-4 text-text-muted" />
        )}
      </button>
      {sourceIcon(source.source_type)}
      <div className="flex-1 min-w-0">
        <p className="text-xs text-text truncate" title={source.title}>
          {sourceTitle(source, grouped)}
        </p>
        <p className="text-[10px] text-text-muted">
          <span className={statusColor(source.status)}>{source.status}</span>
          {source.word_count ? ` · ${source.word_count} words` : ""}
          {statusNote(source)}
          {source.status === "error" && source.error_message && (
            <span title={source.error_message}>
              <AlertCircle className="w-3 h-3 inline text-error" />
            </span>
          )}
          {!source.summary && source.status === "ready" && (
            <span className="text-warning"> · no summary</span>
          )}
          {source.processing_state && (
            <span className="text-text-muted">
              {" · dense "}
              {source.processing_state.dense_index_status}
              {" · projection "}
              {source.processing_state.semantic_projection_status}
            </span>
          )}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        {source.status === "error" && (
          <button
            onClick={() => onRetry(source.id)}
            className="px-1 py-1.5 rounded hover:bg-accent/20 text-text-muted hover:text-accent flex shrink-0 items-center gap-1"
            title="Retry ingestion"
            aria-label={`Retry ingestion of ${source.title}`}
          >
            <RotateCcw className="w-3 h-3" />
            <span className="text-[10px]">Retry</span>
          </button>
        )}
        <button
          onClick={() => onReindex(source.id)}
          className="p-1.5 rounded hover:bg-accent/20 text-text-muted hover:text-accent"
          title="Reindex for semantic-memory preview"
          aria-label={`Reindex ${source.title} for semantic-memory preview`}
        >
          <Layers className="w-3 h-3" />
        </button>
        <button
          onClick={() => onDelete(source.id)}
          className="p-1.5 rounded hover:bg-error/20 text-text-muted hover:text-error"
          title="Delete source"
          aria-label={`Delete ${source.title}`}
        >
          <Trash2 className="w-3 h-3" />
        </button>
      </div>
    </div>
  );
});

export function SourcesPanel({ notebookId }: SourcesPanelProps) {
  const sources = useSourceStore((s: SourceStoreState) => s.sources);
  const selectedSourceIds = useSourceStore((s: SourceStoreState) => s.selectedSourceIds);
  const sourceListStatus = useSourceStore((s: SourceStoreState) => s.sourceListStatus);
  const sourceListError = useSourceStore((s: SourceStoreState) => s.sourceListError);
  const stats = useSourceStore((s: SourceStoreState) => s.stats);
  const toggleSource = useSourceStore((s: SourceStoreState) => s.toggleSource);
  const toggleGroup = useSourceStore((s: SourceStoreState) => s.toggleGroup);
  const selectAll = useSourceStore((s: SourceStoreState) => s.selectAll);
  const selectNone = useSourceStore((s: SourceStoreState) => s.selectNone);
  const addSourceFiles = useSourceStore((s: SourceStoreState) => s.addSourceFiles);
  const addSourceFolder = useSourceStore((s: SourceStoreState) => s.addSourceFolder);
  const deleteSource = useSourceStore((s: SourceStoreState) => s.deleteSource);
  const addSourcePaste = useSourceStore((s: SourceStoreState) => s.addSourcePaste);
  const addSourceUrl = useSourceStore((s: SourceStoreState) => s.addSourceUrl);
  const addSourceYouTubeTranscript = useSourceStore((s: SourceStoreState) => s.addSourceYouTubeTranscript);
  const quarantineFailedImports = useSourceStore((s: SourceStoreState) => s.quarantineFailedImports);
  const deleteFailedImports = useSourceStore((s: SourceStoreState) => s.deleteFailedImports);
  const retrySource = useSourceStore((s: SourceStoreState) => s.retrySource);
  const reindexSource = useSourceStore((s: SourceStoreState) => s.reindexSource);
  const reindexNotebook = useSourceStore((s: SourceStoreState) => s.reindexNotebook);
  const bulkDeleteSelected = useSourceStore((s: SourceStoreState) => s.bulkDeleteSelected);
  const loadSources = useSourceStore((s: SourceStoreState) => s.loadSources);
 
  const [showPaste, setShowPaste] = useState(false);
  const [showUrl, setShowUrl] = useState(false);
  const [pasteTitle, setPasteTitle] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [urlInput, setUrlInput] = useState("");
  const [urlConsent, setUrlConsent] = useState(false);
  const [youtubeLanguage, setYoutubeLanguage] = useState("en");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");
  const [typeFilter, setTypeFilter] = useState("all");
  const [dragActive, setDragActive] = useState(false);
  const [summarizing, setSummarizing] = useState(false);

  // Feature flags
  const featureFlags = useSettingsStore((s) => s.featureFlags);
  const visionEnabled = featureById(featureFlags, "feature_vision_jobs_enabled")?.active === true;
  const videoImportEnabled = featureById(featureFlags, "feature_video_import_enabled")?.active === true;

  // Error retry state for source operations
  const [operationErrors, setOperationErrors] = useState<
    Record<string, { message: string; retry: () => Promise<void> }>
  >({});

  const addOperationError = useCallback(
    (key: string, message: string, retry: () => Promise<void>) => {
      setOperationErrors((prev) => ({ ...prev, [key]: { message, retry } }));
    },
    []
  );

  const clearOperationError = useCallback((key: string) => {
    setOperationErrors((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
  }, []);

  const handleRetryOperation = useCallback(
    async (key: string) => {
      const entry = operationErrors[key];
      if (!entry) return;
      clearOperationError(key);
      try {
        await entry.retry();
      } catch (e) {
        addOperationError(key, String(e), entry.retry);
      }
    },
    [operationErrors, clearOperationError, addOperationError]
  );

  const filteredSources = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return sources.filter((source) => {
      if (needle && !source.title.toLowerCase().includes(needle)) return false;
      if (statusFilter !== "all" && source.status !== statusFilter) return false;
      if (typeFilter !== "all" && source.source_type !== typeFilter) return false;
      return true;
    });
  }, [query, sources, statusFilter, typeFilter]);
  const groups = useMemo(() => groupSources(filteredSources), [filteredSources]);
  const hasGroups = groups.size > 1 || (groups.size === 1 && !groups.has("(ungrouped)"));
  const sourceTypes = useMemo(
    () => Array.from(new Set(sources.map((source) => source.source_type))).sort(),
    [sources]
  );
  const sourceStatuses = useMemo(
    () => Array.from(new Set(sources.map((source) => source.status))).sort(),
    [sources]
  );
  const expectedSourceCount = stats?.source_count ?? sources.length;
  const hasStatsSources = expectedSourceCount > 0;
  const noLoadedSources = sources.length === 0;
  const hasLoadedSources = sources.length > 0;
  const sourceListUnavailable = sourceListStatus === "error";
  const sourceListLoading = sourceListStatus === "loading";
  const sourceListPartial = sourceListStatus === "partial" || sources.length < expectedSourceCount;
  const failedSources = useMemo(
    () => sources.filter((source) => source.status === "error"),
    [sources]
  );

  const useGroupedHeaders = hasGroups;
  const listRows = useMemo<SourceListRow[]>(() => {
    if (!useGroupedHeaders) {
      return filteredSources.map((source) => ({
        kind: "source",
        id: source.id,
        source,
      }));
    }
    return Array.from(groups.entries()).flatMap(([group, groupSources]) => {
      const isCollapsed = collapsed[group] ?? false;
      const allSelected = groupSources.every((source) => selectedSourceIds.has(source.id));
      const headerRow: SourceListRow = {
        kind: "group",
        id: `group:${group}`,
        group,
        sourceCount: groupSources.length,
        isCollapsed,
        allSelected,
      };
      if (isCollapsed) return [headerRow];
        return [
          headerRow,
          ...groupSources.map((source) => ({
          kind: "source" as const,
          id: source.id,
          source,
        })),
      ];
    });
  }, [collapsed, filteredSources, groups, useGroupedHeaders, selectedSourceIds]);

  const handleFileUpload = async () => {
    const selected = await open({
      multiple: true,
      filters: [
        { name: "All Supported", extensions: SUPPORTED_EXTENSIONS },
      ],
    });
    if (selected) {
      const paths = Array.isArray(selected) ? selected : [selected];
      const validPaths = paths.filter(Boolean);
      clearOperationError("fileUpload");
      try {
        await addSourceFiles(notebookId, validPaths);
      } catch (e) {
        const retry = () => addSourceFiles(notebookId, validPaths);
        addOperationError("fileUpload", String(e), retry);
      }
    }
  };

  const handleFolderUpload = async () => {
    const selected = await open({ directory: true });
    if (selected) {
      clearOperationError("folderUpload");
      try {
        await addSourceFolder(notebookId, selected);
      } catch (e) {
        const retry = () => addSourceFolder(notebookId, selected);
        addOperationError("folderUpload", String(e), retry);
      }
    }
  };

  const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp"];
  const VIDEO_EXTENSIONS = ["mp4", "mkv", "webm", "avi", "mov"];

  const handleImageImport = async () => {
    const selected = await open({
      multiple: true,
      filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS }],
    });
    if (selected) {
      const paths = Array.isArray(selected) ? selected : [selected];
      const validPaths = paths.filter(Boolean);
      clearOperationError("imageImport");
      try {
        await addSourceFiles(notebookId, validPaths);
      } catch (e) {
        const retry = () => addSourceFiles(notebookId, validPaths);
        addOperationError("imageImport", String(e), retry);
      }
    }
  };

  const handleVideoImport = async () => {
    const selected = await open({
      multiple: true,
      filters: [{ name: "Videos", extensions: VIDEO_EXTENSIONS }],
    });
    if (selected) {
      const paths = Array.isArray(selected) ? selected : [selected];
      const validPaths = paths.filter(Boolean);
      clearOperationError("videoImport");
      try {
        await addSourceFiles(notebookId, validPaths);
      } catch (e) {
        const retry = () => addSourceFiles(notebookId, validPaths);
        addOperationError("videoImport", String(e), retry);
      }
    }
  };

  const handlePaste = async () => {
    if (!pasteText.trim()) return;
    const title = pasteTitle || "Pasted Text";
    const text = pasteText;
    clearOperationError("paste");
    try {
      await addSourcePaste(notebookId, title, text);
      setPasteTitle("");
      setPasteText("");
      setShowPaste(false);
    } catch (e) {
      const retry = () => addSourcePaste(notebookId, title, text);
      addOperationError("paste", String(e), retry);
    }
  };

  const handleUrlImport = async () => {
    const trimmed = urlInput.trim();
    if (!trimmed) return;
    const consent = urlConsent;
    clearOperationError("urlImport");
    try {
      await addSourceUrl(notebookId, trimmed, consent);
      setUrlInput("");
      setUrlConsent(false);
      setShowUrl(false);
    } catch (e) {
      const retry = () => addSourceUrl(notebookId, trimmed, consent);
      addOperationError("urlImport", String(e), retry);
    }
  };

  const handleYouTubeTranscriptImport = async () => {
    const trimmed = urlInput.trim();
    if (!trimmed) return;
    const lang = youtubeLanguage.trim() || "en";
    const consent = urlConsent;
    clearOperationError("youtubeImport");
    try {
      await addSourceYouTubeTranscript(notebookId, trimmed, lang, consent);
      setUrlInput("");
      setUrlConsent(false);
      setShowUrl(false);
    } catch (e) {
      const retry = () =>
        addSourceYouTubeTranscript(notebookId, trimmed, lang, consent);
      addOperationError("youtubeImport", String(e), retry);
    }
  };

  const handleDrop = async (event: React.DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDragActive(false);
    const files = Array.from(event.dataTransfer.files);
    const paths = files
      .map((file) => (file as File & { path?: string }).path)
      .filter((path): path is string => Boolean(path));
    clearOperationError("drop");
    try {
      await addSourceFiles(notebookId, paths);
    } catch (e) {
      const retry = () => addSourceFiles(notebookId, paths);
      addOperationError("drop", String(e), retry);
    }
  };

  const handleSummarize = async () => {
    if (summarizing) return;
    setSummarizing(true);
    clearOperationError("summarize");
    try {
      await api.regenerateMissingSummaries(notebookId);
    } catch (e) {
      const retry = async () => { await api.regenerateMissingSummaries(notebookId); };
      addOperationError("summarize", String(e), retry);
    } finally {
      setSummarizing(false);
    }
  };

  const listRow = (row: SourceListRow) => {
    if (row.kind === "group") {
      return (
        <div
          key={row.id}
          role="button"
          tabIndex={0}
          onClick={() =>
            setCollapsed((c) => ({
              ...c,
              [row.group]: !row.isCollapsed,
            }))
          }
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              setCollapsed((c) => ({
                ...c,
                [row.group]: !row.isCollapsed,
              }));
            }
          }}
          className="sticky top-0 z-10 flex items-center gap-1.5 w-full px-2 py-1 text-xs font-medium text-text-muted hover:text-text cursor-pointer bg-bg-primary border-b border-border/40"
        >
          <ChevronRight
            className={`w-3 h-3 transition-transform ${
              !row.isCollapsed ? "rotate-90" : ""
            }`}
          />
          <button
            onClick={(e) => {
              e.stopPropagation();
              toggleGroup(row.group);
            }}
            className="shrink-0"
          >
            {row.allSelected ? (
              <CheckSquare className="w-3.5 h-3.5 text-accent" />
            ) : (
              <Square className="w-3.5 h-3.5 text-text-muted" />
            )}
          </button>
          <FolderOpen className="w-3 h-3" />
          <span className="truncate">{row.group}</span>
          <span className="text-[10px] text-text-muted ml-auto shrink-0">
            {row.sourceCount}
          </span>
        </div>
      );
    }

    const isSelected = selectedSourceIds.has(row.source.id);
    return (
      <SourceListItem
        key={row.id}
        source={row.source}
        isSelected={isSelected}
        grouped={useGroupedHeaders}
        onToggle={toggleSource}
        onRetry={retrySource.bind(null, notebookId)}
        onReindex={reindexSource.bind(null, notebookId)}
        onDelete={deleteSource.bind(null, notebookId)}
      />
    );
  };

  return (
    <div
      className={`flex flex-col h-full ${dragActive ? "outline outline-2 outline-accent outline-offset-[-2px]" : ""}`}
      onDragOver={(event) => {
        event.preventDefault();
        setDragActive(true);
      }}
      onDragLeave={() => setDragActive(false)}
      onDrop={handleDrop}
    >
      <div className="border-b border-border p-2">
        <div className="flex gap-1">
          <button
            onClick={handleFileUpload}
            className="flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
          >
            <Upload className="w-3 h-3" /> Upload
          </button>
          <button
            onClick={handleFolderUpload}
            className="flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
          >
            <FolderOpen className="w-3 h-3" /> Folder
          </button>
          <button
            onClick={() => setShowPaste(!showPaste)}
            className="flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
          >
            <ClipboardPaste className="w-3 h-3" /> Paste
          </button>
          <button
            onClick={() => setShowUrl(!showUrl)}
            className="flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
          >
            <Link className="w-3 h-3" /> URL
          </button>
          {visionEnabled && (
            <button
              onClick={handleImageImport}
              className="flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
            >
              <Image className="w-3 h-3" /> Image
            </button>
          )}
          {videoImportEnabled && (
            <button
              onClick={handleVideoImport}
              className="flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
            >
              <Video className="w-3 h-3" /> Video
            </button>
          )}
        </div>
        <details className="mt-2 rounded border border-border bg-bg-tertiary px-2 py-1 text-[10px] leading-snug text-text-muted">
          <summary className="flex cursor-pointer items-start gap-1.5">
            <AlertCircle className="mt-0.5 h-3 w-3 shrink-0" />
            <span>Strict import — what's supported</span>
          </summary>
          <span className="ml-5 mt-1 block">
            text, markdown, code, paste, local PDF/DOCX/DOC/XLSX/XLS/PPTX/PPT/EPUB extraction, URL text fetch, YouTube transcript fetch, audio metadata, and cached Whisper audio transcription are supported; CSV, HTML, image, video, and legacy Office CLI extraction are degraded.
          </span>
        </details>

        {hasLoadedSources && (
          <div className="mt-2 flex flex-wrap gap-2 text-xs text-text-muted">
            <button onClick={selectAll} className="hover:text-text">
              Select all
            </button>
            <span>|</span>
            <button onClick={selectNone} className="hover:text-text">
              None
            </button>
            <span>|</span>
            <button onClick={() => bulkDeleteSelected(notebookId)} className="hover:text-error">
              Delete selected
            </button>
            <button onClick={() => reindexNotebook(notebookId)} className="hover:text-accent">
              Reindex all
            </button>
            <button onClick={handleSummarize} className="hover:text-accent" disabled={summarizing}>
              {summarizing ? "Queuing..." : "Summarize missing"}
            </button>
            <span className="gloss-mono ml-auto text-[10px]">
              Loaded {sources.length} of {expectedSourceCount}
            </span>
          </div>
        )}
        {failedSources.length > 0 && (
          <div className="mt-2 rounded border border-error/40 bg-error/10 p-2">
            <div className="flex items-center gap-2">
              <AlertCircle className="h-3.5 w-3.5 text-error" />
              <span className="text-xs font-medium text-text">
                Failed imports
              </span>
              <span className="ml-auto text-[10px] text-text-muted">
                {failedSources.length}
              </span>
            </div>
            <div className="mt-2 flex flex-wrap gap-2 text-[10px]">
              <button
                onClick={() => {
                  for (const source of failedSources) {
                    retrySource(notebookId, source.id);
                  }
                }}
                className="rounded border border-accent/50 px-2 py-0.5 text-accent hover:bg-accent/10"
              >
                Retry All
              </button>
              <button
                onClick={() => setStatusFilter("error")}
                className="rounded border border-border px-2 py-0.5 text-text-secondary hover:bg-border hover:text-text"
              >
                Review
              </button>
              <button
                onClick={() => quarantineFailedImports(notebookId)}
                className="rounded border border-border px-2 py-0.5 text-text-secondary hover:bg-border hover:text-text"
              >
                Quarantine
              </button>
              <button
                onClick={() => deleteFailedImports(notebookId)}
                className="rounded border border-error/50 px-2 py-0.5 text-error hover:bg-error/10"
              >
                Delete Failed
              </button>
            </div>
          </div>
        )}
        {Object.keys(operationErrors).length > 0 && (
          <div className="mt-2 space-y-1">
            {Object.entries(operationErrors).map(([key, { message }]) => (
              <div
                key={key}
                className="flex items-center gap-2 rounded border border-error/40 bg-error/10 px-2 py-1.5 text-xs text-text-secondary"
              >
                <AlertCircle className="h-3 w-3 text-error shrink-0" />
                <span className="min-w-0 flex-1 truncate" title={message}>
                  {message}
                </span>
                <button
                  onClick={() => handleRetryOperation(key)}
                  className="shrink-0 flex items-center gap-1 rounded border border-accent/50 px-2 py-0.5 text-[10px] text-accent hover:bg-accent/10"
                >
                  <RotateCcw className="w-3 h-3" />
                  Retry
                </button>
                <button
                  onClick={() => clearOperationError(key)}
                  className="shrink-0 rounded border border-border px-2 py-0.5 text-[10px] text-text-muted hover:bg-border"
                >
                  Dismiss
                </button>
              </div>
            ))}
          </div>
        )}
        {(sourceListLoading || sourceListPartial || sourceListUnavailable || hasStatsSources) && (
          <div className="mt-2 flex items-center gap-2 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary">
            <span className="min-w-0 flex-1 truncate">
              {sourceListUnavailable
                ? sourceListError || "Source list failed to load."
                : sourceListLoading
                  ? "Loading sources..."
                  : sourceListPartial
                    ? `Loaded ${sources.length} of ${expectedSourceCount}`
                    : `${sources.length} sources loaded`}
            </span>
            <button
              onClick={() => loadSources(notebookId)}
              className="shrink-0 rounded border border-border px-2 py-0.5 text-[10px] text-text hover:bg-border"
            >
              Reload Sources
            </button>
          </div>
        )}
        <div className="mt-2 space-y-1.5">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-text-muted" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Filter sources"
              aria-label="Filter sources"
              className="w-full rounded border border-border bg-bg-tertiary py-1 pl-7 pr-2 text-xs text-text placeholder:text-text-muted focus:border-accent focus:outline-none"
            />
          </div>
          <div className="grid grid-cols-2 gap-1">
            <select
              value={statusFilter}
              onChange={(event) => setStatusFilter(event.target.value)}
              aria-label="Filter by status"
              className="rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text focus:border-accent focus:outline-none"
            >
              <option value="all">All statuses</option>
              {sourceStatuses.map((status) => (
                <option key={status} value={status}>{status}</option>
              ))}
            </select>
            <select
              value={typeFilter}
              onChange={(event) => setTypeFilter(event.target.value)}
              aria-label="Filter by type"
              className="rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text focus:border-accent focus:outline-none"
            >
              <option value="all">All types</option>
              {sourceTypes.map((type) => (
                <option key={type} value={type}>{type}</option>
              ))}
            </select>
          </div>
        </div>
        {dragActive && (
          <div className="mt-2 flex items-center gap-1.5 rounded border border-accent/40 bg-accent/10 px-2 py-1 text-xs text-accent">
            <Upload className="h-3 w-3" />
            Drop files to import
          </div>
        )}
      </div>

      {showPaste && (
        <div className="p-2 border-b border-border space-y-1">
          <input
            type="text"
            value={pasteTitle}
            onChange={(e) => setPasteTitle(e.target.value)}
            placeholder="Title (optional)"
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
          />
          <textarea
            value={pasteText}
            onChange={(e) => setPasteText(e.target.value)}
            placeholder="Paste text here..."
            rows={4}
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent resize-none"
          />
          <button
            onClick={handlePaste}
            className="w-full py-1 text-xs bg-accent text-white rounded hover:bg-accent-hover"
          >
            Add Source
          </button>
        </div>
      )}

      {showUrl && (
        <div className="p-2 border-b border-border space-y-2">
          <input
            type="url"
            value={urlInput}
            onChange={(e) => setUrlInput(e.target.value)}
            placeholder="https://example.com/article"
            aria-label="URL to import"
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
          />
          <label className="flex items-start gap-2 text-[10px] leading-snug text-text-muted">
            <input
              type="checkbox"
              checked={urlConsent}
              onChange={(e) => setUrlConsent(e.target.checked)}
              className="mt-0.5"
            />
            <span>Allow this one web fetch. No crawling, credentials, localhost, intranet hosts, video download, or authenticated YouTube access.</span>
          </label>
          <input
            type="text"
            value={youtubeLanguage}
            onChange={(e) => setYoutubeLanguage(e.target.value)}
            placeholder="Transcript language, e.g. en"
            aria-label="YouTube transcript language"
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
          />
          <button
            onClick={handleUrlImport}
            disabled={!urlInput.trim() || !urlConsent}
            className="w-full py-1 text-xs bg-accent text-white rounded hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
          >
            Add URL
          </button>
          <button
            onClick={handleYouTubeTranscriptImport}
            disabled={!urlInput.trim() || !urlConsent}
            className="w-full py-1 text-xs bg-bg-tertiary border border-border text-text rounded hover:bg-border disabled:cursor-not-allowed disabled:opacity-50"
          >
            Add YouTube Transcript
          </button>
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-1">
        {listRows.length > 0 ? (
          <Virtuoso
            data={listRows}
            itemContent={(_index, row) => listRow(row)}
          />
        ) : (
          <>
            {hasLoadedSources && filteredSources.length === 0 && (
              <p className="text-xs text-text-muted text-center mt-4 px-2">
                No sources match the current filters.
              </p>
            )}
          </>
        )}

        {sourceListLoading && noLoadedSources && (
          <p className="text-xs text-text-muted text-center mt-4 px-2">
            Loading sources...
          </p>
        )}
        {sourceListUnavailable && noLoadedSources && (
          <div className="mt-4 px-2 text-center text-xs text-text-muted">
            <p className="text-error">{sourceListError || "Source list failed to load."}</p>
            <button
              onClick={() => loadSources(notebookId)}
              className="mt-2 inline-flex items-center gap-1 rounded border border-accent/50 px-3 py-1 text-accent hover:bg-accent/10"
            >
              <RotateCcw className="w-3 h-3" />
              Retry Load Sources
            </button>
          </div>
        )}
        {!sourceListLoading && !sourceListUnavailable && hasStatsSources && noLoadedSources && (
          <div className="mt-4 px-2 text-center text-xs text-text-muted">
            <p>Notebook stats report {expectedSourceCount} sources, but the source list is not loaded.</p>
            <button
              onClick={() => loadSources(notebookId)}
              className="mt-2 inline-flex items-center gap-1 rounded border border-accent/50 px-3 py-1 text-accent hover:bg-accent/10"
            >
              <RotateCcw className="w-3 h-3" />
              Retry Load Sources
            </button>
          </div>
        )}
        {!sourceListLoading && !sourceListUnavailable && !hasStatsSources && noLoadedSources && (
          <p className="text-xs text-text-muted text-center mt-4 px-2">
            No sources yet. Upload files, add a folder, or paste text.
          </p>
        )}
      </div>
    </div>
  );
}
