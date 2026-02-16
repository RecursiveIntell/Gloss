import { useState, useMemo } from "react";
import { useSourceStore } from "../../stores/sourceStore";
import { open } from "@tauri-apps/plugin-dialog";
import type { Source } from "../../lib/types";
import {
  FileText,
  Upload,
  FolderOpen,
  ClipboardPaste,
  Code,
  Image,
  Video,
  Trash2,
  CheckSquare,
  Square,
  ChevronRight,
  RefreshCw,
  AlertCircle,
} from "lucide-react";

interface SourcesPanelProps {
  notebookId: string;
}

const SUPPORTED_EXTENSIONS = [
  "txt", "md", "markdown", "rst",
  "py", "js", "jsx", "ts", "tsx", "rs", "go", "java", "c", "cpp", "cc", "cxx",
  "h", "hpp", "cs", "rb", "php", "swift", "kt", "kts", "scala", "lua", "r",
  "sql", "sh", "bash", "zsh", "css", "scss", "sass", "html", "htm", "xml",
  "json", "yaml", "yml", "toml", "ini", "cfg", "conf", "vue", "svelte",
  "dart", "ex", "exs", "zig", "nim", "pl", "pm", "proto", "graphql", "gql",
  "tf", "hcl", "dockerfile", "makefile",
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tiff", "tif",
  "mp4", "webm", "mov", "avi", "mkv",
];

function sourceIcon(sourceType: string) {
  switch (sourceType) {
    case "code":
      return <Code className="w-4 h-4 text-text-muted shrink-0" />;
    case "image":
      return <Image className="w-4 h-4 text-text-muted shrink-0" />;
    case "video":
      return <Video className="w-4 h-4 text-text-muted shrink-0" />;
    case "paste":
      return <ClipboardPaste className="w-4 h-4 text-text-muted shrink-0" />;
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

export function SourcesPanel({ notebookId }: SourcesPanelProps) {
  const {
    sources,
    selectedSourceIds,
    toggleSource,
    toggleGroup,
    selectAll,
    selectNone,
    addSourceFile,
    addSourceFolder,
    deleteSource,
    addSourcePaste,
    retrySource,
  } = useSourceStore();
  const [showPaste, setShowPaste] = useState(false);
  const [pasteTitle, setPasteTitle] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});

  const groups = useMemo(() => groupSources(sources), [sources]);
  const hasGroups = groups.size > 1 || (groups.size === 1 && !groups.has("(ungrouped)"));

  const handleFileUpload = async () => {
    const selected = await open({
      multiple: true,
      filters: [
        { name: "All Supported", extensions: SUPPORTED_EXTENSIONS },
      ],
    });
    if (selected) {
      const paths = Array.isArray(selected) ? selected : [selected];
      for (const path of paths) {
        if (path) await addSourceFile(notebookId, path);
      }
    }
  };

  const handleFolderUpload = async () => {
    const selected = await open({ directory: true });
    if (selected) {
      await addSourceFolder(notebookId, selected);
    }
  };

  const handlePaste = async () => {
    if (!pasteText.trim()) return;
    await addSourcePaste(notebookId, pasteTitle || "Pasted Text", pasteText);
    setPasteTitle("");
    setPasteText("");
    setShowPaste(false);
  };

  const statusColor = (status: string) => {
    switch (status) {
      case "ready":
        return "text-success";
      case "error":
        return "text-error";
      case "pending":
        return "text-warning";
      default:
        return "text-accent";
    }
  };

  const statusNote = (source: Source) => {
    if (
      (source.source_type === "image" || source.source_type === "video") &&
      source.status === "pending"
    ) {
      return " \u00B7 Awaiting vision model";
    }
    return "";
  };

  const displayTitle = (source: Source) => {
    // If grouped, show path after the group prefix
    if (hasGroups) {
      const parts = source.title.split("/");
      return parts.length > 1 ? parts.slice(1).join("/") : source.title;
    }
    return source.title;
  };

  const renderSourceCard = (source: Source) => (
    <div
      key={source.id}
      className="group flex items-center gap-2 px-2 py-1.5 rounded hover:bg-bg-tertiary"
    >
      <button
        onClick={() => toggleSource(source.id)}
        className="shrink-0"
      >
        {selectedSourceIds.has(source.id) ? (
          <CheckSquare className="w-4 h-4 text-accent" />
        ) : (
          <Square className="w-4 h-4 text-text-muted" />
        )}
      </button>
      {sourceIcon(source.source_type)}
      <div className="flex-1 min-w-0">
        <p className="text-xs text-text truncate" title={source.title}>
          {displayTitle(source)}
        </p>
        <p className="text-[10px] text-text-muted">
          <span className={statusColor(source.status)}>
            {source.status}
          </span>
          {source.word_count ? ` \u00B7 ${source.word_count} words` : ""}
          {statusNote(source)}
          {source.status === "error" && source.error_message && (
            <span title={source.error_message}>
              {" "}<AlertCircle className="w-3 h-3 inline text-error" />
            </span>
          )}
          {!source.summary && source.status === "ready" && (
            <span className="text-warning"> \u00B7 no summary</span>
          )}
        </p>
      </div>
      {source.status === "error" && (
        <button
          onClick={() => retrySource(notebookId, source.id)}
          className="p-0.5 rounded hover:bg-accent/20 text-text-muted hover:text-accent"
          title="Retry ingestion"
        >
          <RefreshCw className="w-3 h-3" />
        </button>
      )}
      <button
        onClick={() => deleteSource(notebookId, source.id)}
        className="hidden group-hover:block p-0.5 rounded hover:bg-error/20 text-text-muted hover:text-error"
      >
        <Trash2 className="w-3 h-3" />
      </button>
    </div>
  );

  return (
    <div className="flex flex-col h-full">
      <div className="p-3 border-b border-border">
        <h2 className="text-sm font-semibold text-text mb-2">Sources</h2>
        <div className="flex gap-1">
          <button
            onClick={handleFileUpload}
            className="flex items-center gap-1 px-2 py-1 text-xs bg-bg-tertiary rounded hover:bg-border text-text-secondary hover:text-text"
          >
            <Upload className="w-3 h-3" /> Upload
          </button>
          <button
            onClick={handleFolderUpload}
            className="flex items-center gap-1 px-2 py-1 text-xs bg-bg-tertiary rounded hover:bg-border text-text-secondary hover:text-text"
          >
            <FolderOpen className="w-3 h-3" /> Folder
          </button>
          <button
            onClick={() => setShowPaste(!showPaste)}
            className="flex items-center gap-1 px-2 py-1 text-xs bg-bg-tertiary rounded hover:bg-border text-text-secondary hover:text-text"
          >
            <ClipboardPaste className="w-3 h-3" /> Paste
          </button>
        </div>

        {sources.length > 0 && (
          <div className="flex gap-2 mt-2 text-xs text-text-muted">
            <button onClick={selectAll} className="hover:text-text">
              Select all
            </button>
            <span>|</span>
            <button onClick={selectNone} className="hover:text-text">
              None
            </button>
            <span className="ml-auto">{sources.length} sources</span>
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

      <div className="flex-1 overflow-y-auto p-1">
        {hasGroups ? (
          Array.from(groups.entries()).map(([group, groupSources]) => {
            const isCollapsed = collapsed[group] ?? false;
            const allSelected = groupSources.every(s => selectedSourceIds.has(s.id));
            return (
              <div key={group}>
                <button
                  onClick={() =>
                    setCollapsed((c) => ({ ...c, [group]: !c[group] }))
                  }
                  className="flex items-center gap-1.5 w-full px-2 py-1 text-xs font-medium text-text-muted hover:text-text"
                >
                  <ChevronRight
                    className={`w-3 h-3 transition-transform ${
                      !isCollapsed ? "rotate-90" : ""
                    }`}
                  />
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleGroup(group);
                    }}
                    className="shrink-0"
                  >
                    {allSelected ? (
                      <CheckSquare className="w-3.5 h-3.5 text-accent" />
                    ) : (
                      <Square className="w-3.5 h-3.5 text-text-muted" />
                    )}
                  </button>
                  <FolderOpen className="w-3 h-3" />
                  <span className="truncate">{group}</span>
                  <span className="text-[10px] text-text-muted ml-auto shrink-0">
                    {groupSources.length}
                  </span>
                </button>
                {!isCollapsed && (
                  <div className="pl-4">
                    {groupSources.map(renderSourceCard)}
                  </div>
                )}
              </div>
            );
          })
        ) : (
          sources.map(renderSourceCard)
        )}

        {sources.length === 0 && (
          <p className="text-xs text-text-muted text-center mt-4 px-2">
            No sources yet. Upload files, add a folder, or paste text.
          </p>
        )}
      </div>
    </div>
  );
}
