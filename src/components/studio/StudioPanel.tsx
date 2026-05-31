import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  ClipboardList,
  Download,
  FileText,
  GitCompare,
  HelpCircle,
  ListTree,
  Map,
  Network,
  RefreshCw,
  Rows3,
  Sparkles,
  Timer,
} from "lucide-react";
import { useSourceStore } from "../../stores/sourceStore";
import { useStudioStore } from "../../stores/studioStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { useToastStore } from "../../stores/toastStore";
import type { StudioOutput } from "../../lib/types";
import { featureById } from "../../lib/features";
import ReactMarkdown from "react-markdown";
import { FlashcardWidget } from "./FlashcardWidget";
import { QuizWidget } from "./QuizWidget";
import { MindMapGraph } from "./MindMapGraph";

interface StudioPanelProps {
  notebookId: string;
}

const OUTPUT_TYPES = [
  { id: "report", label: "Report", icon: FileText },
  { id: "summary", label: "Summary", icon: ClipboardList },
  { id: "outline", label: "Outline", icon: ListTree },
  { id: "faq", label: "FAQ", icon: HelpCircle },
  { id: "flashcards", label: "Cards", icon: Rows3 },
  { id: "quiz", label: "Quiz", icon: HelpCircle },
  { id: "mind_map", label: "Map", icon: Network },
  { id: "timeline", label: "Timeline", icon: Timer },
  { id: "compare_table", label: "Compare", icon: GitCompare },
  { id: "action_plan", label: "Actions", icon: Map },
] as const;

export function StudioPanel({ notebookId }: StudioPanelProps) {
  const selectedSourceIds = useSourceStore((state) => state.selectedSourceIds);
  const {
    outputs,
    activeOutputType,
    activeOutputId,
    status,
    error,
    lastExportReceipt,
    setActiveOutputType,
    setActiveOutputId,
    loadOutputs,
    generateOutput,
    exportOutput,
  } = useStudioStore();
  const addToast = useToastStore((state) => state.addToast);
  const [maxItems, setMaxItems] = useState(8);

  useEffect(() => {
    void loadOutputs(notebookId);
  }, [notebookId, loadOutputs]);

  const activeOutput = useMemo(
    () => outputs.find((output) => output.id === activeOutputId) ?? outputs[0] ?? null,
    [outputs, activeOutputId]
  );
  const selectedIds = useMemo(() => Array.from(selectedSourceIds), [selectedSourceIds]);
  const busy = status === "loading" || status === "generating" || status === "exporting";

  const handleGenerate = async () => {
    const output = await generateOutput(notebookId, activeOutputType, selectedIds, maxItems);
    if (output) {
      addToast({
        type: "success",
        title: "Studio Output Ready",
        message: output.title ?? output.output_type,
        duration: 3000,
      });
    }
  };

  const handleExport = async () => {
    if (!activeOutput) return;
    const receipt = await exportOutput(notebookId, activeOutput.id);
    if (receipt) {
      addToast({
        type: "success",
        title: "Studio Export Written",
        message: receipt.file_path,
        duration: 5000,
      });
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <div className="border-b border-border bg-bg-secondary/70 p-2">
        <div className="grid grid-cols-2 gap-1 sm:grid-cols-3">
          {OUTPUT_TYPES.map((item) => {
            const Icon = item.icon;
            return (
              <button
                key={item.id}
                type="button"
                title={item.label}
                onClick={() => setActiveOutputType(item.id)}
                className={`flex h-8 items-center justify-center gap-1 rounded border px-1.5 text-[11px] ${
                  activeOutputType === item.id
                    ? "border-accent bg-accent/10 text-text"
                    : "border-border bg-bg-tertiary text-text-muted hover:text-text"
                }`}
              >
                <Icon className="h-3.5 w-3.5" />
                <span className="truncate">{item.label}</span>
              </button>
            );
          })}
        </div>

        <div className="mt-2 flex items-center gap-2">
          <label className="gloss-mono text-[10px] uppercase tracking-[0.03em] text-text-muted">
            Items
          </label>
          <input
            type="number"
            min={1}
            max={20}
            value={maxItems}
            onChange={(event) => setMaxItems(Math.min(20, Math.max(1, Number(event.target.value) || 1)))}
            className="h-8 w-16 rounded border border-border bg-bg px-2 text-xs text-text"
          />
          <span className="min-w-0 flex-1 truncate text-[11px] text-text-muted">
            {selectedIds.length > 0 ? `${selectedIds.length} scoped` : "selected/all ready"}
          </span>
          <button
            type="button"
            onClick={handleGenerate}
            disabled={busy}
            className="flex h-8 items-center gap-1 rounded border border-accent bg-accent/10 px-2 text-xs text-text hover:bg-accent/20 disabled:opacity-50"
          >
            {status === "generating" ? <RefreshCw className="h-3.5 w-3.5 animate-spin" /> : <Sparkles className="h-3.5 w-3.5" />}
            Generate
          </button>
        </div>
      </div>

      {error && (
        <div className="border-b border-error/30 bg-error/10 px-3 py-2 text-xs text-error">
          {error}
        </div>
      )}

      {/* Compact output selector — replaces the old sidebar */}
      {outputs.length > 0 && (
        <div className="flex gap-1 overflow-x-auto border-b border-border px-2 py-1">
          {outputs.map((output) => (
            <button
              key={output.id}
              type="button"
              onClick={() => setActiveOutputId(output.id)}
              className={`shrink-0 rounded border px-3 py-1 text-xs ${
                activeOutput?.id === output.id
                  ? "border-accent bg-accent/10 text-text"
                  : "border-border bg-bg-tertiary text-text-muted hover:text-text"
              }`}
            >
              {output.title ?? output.output_type}
            </button>
          ))}
        </div>
      )}

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {activeOutput ? (
          <>
            <div className="flex items-center gap-2 border-b border-border px-3 py-1.5">
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium text-text">{activeOutput.title ?? activeOutput.output_type}</div>
              </div>
              <button
                type="button"
                onClick={handleExport}
                disabled={busy}
                title="Export Studio output"
                className="rounded border border-border p-1 text-text-muted hover:bg-bg-tertiary hover:text-text disabled:opacity-50"
              >
                {status === "exporting" ? <RefreshCw className="h-3.5 w-3.5 animate-spin" /> : <Download className="h-3.5 w-3.5" />}
              </button>
            </div>
            <StudioOutputBody output={activeOutput} />
            <div className="border-t border-border px-3 py-1.5 text-[10px] text-text-muted">
              <span className="gloss-mono">sources {activeOutput.source_ids.length}</span>
              {activeOutput.file_path && <span className="ml-2 truncate">export {activeOutput.file_path}</span>}
              {lastExportReceipt?.output_id === activeOutput.id && (
                <span className="ml-2 gloss-mono">sha {lastExportReceipt.sha256.slice(0, 12)}</span>
              )}
            </div>
          </>
        ) : (
          <div className="flex h-full items-center justify-center p-4 text-center text-xs text-text-muted">
            Select an output type
          </div>
        )}
      </div>
    </div>
  );
}

function StudioOutputBody({ output }: { output: StudioOutput }) {
  const artifact = useMemo(() => parseArtifact(output.raw_content), [output.raw_content]);
  const content = artifact?.content ?? artifact ?? output.raw_content ?? "";
  const featureFlags = useSettingsStore((s) => s.featureFlags);
  const prose = output.prose_content;

  const flashcardActive = featureById(featureFlags, "feature_flashcard_widget_enabled")?.active === true;
  const quizActive = featureById(featureFlags, "feature_quiz_widget_enabled")?.active === true;
  const mindMapActive = featureById(featureFlags, "feature_mind_map_widget_enabled")?.active === true;

  if (output.output_type === "flashcards" && flashcardActive) {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto">
        <FlashcardWidget output={output} />
      </div>
    );
  }
  if (output.output_type === "quiz" && quizActive) {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto">
        <QuizWidget output={output} />
      </div>
    );
  }
  if (output.output_type === "mind_map" && mindMapActive) {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto">
        <MindMapGraph output={output} />
      </div>
    );
  }

  // LLM-refined prose takes priority — full width, no sidebar
  if (prose) {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto p-4 prose prose-sm prose-invert max-w-none w-full">
        <ReactMarkdown>{prose}</ReactMarkdown>
        {artifact?.validation != null && (
          <div className="mt-4 border-t border-border pt-2 text-[11px] text-text-muted">
            <span className="gloss-mono">refined ✓</span>
            <span className="ml-2 gloss-mono">schema {String(artifact.validation.schema_validated)}</span>
            <span className="ml-2 gloss-mono">cited {String(artifact.validation.all_items_source_cited)}</span>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-4">
      {renderValue(content)}
      {artifact?.validation != null && (
        <div className="mt-3 border-t border-border pt-2 text-[11px] text-text-muted">
          <span className="gloss-mono">schema {String(artifact.validation.schema_validated)}</span>
          <span className="ml-2 gloss-mono">cited {String(artifact.validation.all_items_source_cited)}</span>
        </div>
      )}
    </div>
  );
}

interface ArtifactCitation {
  source_title?: string;
  source_id?: string;
  [key: string]: unknown;
}

interface ArtifactValidation {
  schema_validated?: boolean;
  all_items_source_cited?: boolean;
  [key: string]: unknown;
}

interface ArtifactContent {
  content?: unknown;
  validation?: ArtifactValidation;
  citations?: ArtifactCitation[];
  [key: string]: unknown;
}

function parseArtifact(raw?: string): ArtifactContent | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as ArtifactContent;
  } catch {
    return null;
  }
}

function renderValue(value: unknown): ReactNode {
  if (value == null) return <span className="text-xs text-text-muted">No content</span>;
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return <p className="whitespace-pre-wrap text-sm leading-relaxed text-text-secondary">{String(value)}</p>;
  }
  if (Array.isArray(value)) {
    return (
      <div className="space-y-2">
        {value.map((item, index) => (
          <div key={index} className="border-b border-border pb-2 last:border-0">
            {renderValue(item)}
          </div>
        ))}
      </div>
    );
  }
  if (typeof value === "object" && value !== null) {
    const obj = value as Record<string, unknown>;
    return (
      <div className="space-y-2">
        {Object.entries(obj)
          .filter(([key]) => key !== "citations")
          .map(([key, item]) => (
            <div key={key}>
              <div className="gloss-mono mb-1 text-[10px] uppercase tracking-[0.03em] text-text-muted">
                {key.replace(/_/g, " ")}
              </div>
              {renderValue(item)}
            </div>
          ))}
        {Array.isArray(obj.citations) && obj.citations.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {obj.citations.map((citation: ArtifactCitation, index: number) => (
              <span key={index} className="rounded border border-border bg-bg-secondary px-1.5 py-0.5 text-[10px] text-text-muted">
                {citation.source_title ?? citation.source_id}
              </span>
            ))}
          </div>
        )}
      </div>
    );
  }
  return <span className="text-xs text-text-muted">{String(value)}</span>;
}
