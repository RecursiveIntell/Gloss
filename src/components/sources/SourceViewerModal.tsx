import { useEffect, useState } from "react";
import { X, Loader2, BookMarked, AlertCircle } from "lucide-react";
import type { Citation, SourceContent } from "../../lib/types";
import * as api from "../../lib/tauri";

interface SourceViewerModalProps {
  notebookId: string;
  citation: Citation | null;
  open: boolean;
  onClose: () => void;
}

export function SourceViewerModal({
  notebookId,
  citation,
  open,
  onClose,
}: SourceViewerModalProps) {
  const [content, setContent] = useState<SourceContent | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !citation) {
      setContent(null);
      setError(null);
      setLoading(false);
      return;
    }

    if (!citation.source_id) {
      setContent(null);
      setLoading(false);
      setError("This citation does not have a linked source record.");
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);
    setContent(null);

    api.getSourceContent(notebookId, citation.source_id)
      .then((result) => {
        if (cancelled) return;
        setContent(result);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(String(err));
      })
      .finally(() => {
        if (cancelled) return;
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [open, notebookId, citation]);

  if (!open || !citation) {
    return null;
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 p-4">
      <div className="w-full max-w-3xl max-h-[85vh] overflow-hidden rounded-xl border border-border bg-bg-secondary shadow-2xl">
        <div className="flex items-start justify-between gap-4 border-b border-border px-4 py-3">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-xs text-text-muted">
              <BookMarked className="w-3.5 h-3.5" />
              <span>Source Viewer</span>
            </div>
            <h3 className="mt-1 truncate text-sm font-semibold text-text">
              {citation.source_title}
            </h3>
          </div>
          <button
            onClick={onClose}
            className="rounded p-1 text-text-muted hover:bg-bg-tertiary hover:text-text"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="max-h-[calc(85vh-62px)] overflow-y-auto p-4 space-y-4">
          {citation.quote && (
            <div className="rounded-lg border border-accent/25 bg-accent/10 p-3">
              <p className="text-[11px] uppercase tracking-wide text-accent">Cited Excerpt</p>
              <p className="mt-1 text-sm text-text whitespace-pre-wrap">{citation.quote}</p>
            </div>
          )}

          {loading && (
            <div className="flex items-center gap-2 text-sm text-text-muted">
              <Loader2 className="w-4 h-4 animate-spin" />
              <span>Loading source content...</span>
            </div>
          )}

          {error && (
            <div className="flex items-start gap-2 rounded-lg border border-error/30 bg-error/10 p-3 text-sm text-error">
              <AlertCircle className="mt-0.5 w-4 h-4 shrink-0" />
              <span>{error}</span>
            </div>
          )}

          {!loading && !error && (
            <div className="space-y-2">
              <div className="text-xs text-text-muted">
                {content?.word_count ? `${content.word_count.toLocaleString()} words` : "Source text"}
              </div>
              <pre className="whitespace-pre-wrap rounded-lg bg-bg-tertiary p-3 text-xs text-text overflow-x-auto">
                {content?.content_text || "No stored source content is available for this citation."}
              </pre>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
