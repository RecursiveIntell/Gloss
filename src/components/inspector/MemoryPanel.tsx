import { useState, useEffect } from "react";
import { Database, Search, RefreshCw, Activity } from "lucide-react";
import { useHealthStore } from "../../stores/healthStore";
import { useNotebookStore } from "../../stores/notebookStore";
import { runRetrievalProbe } from "../../lib/tauri";
import type { RetrievalProbeReceipt, MemoryBackendStatus, SemanticMemoryProfileStatus } from "../../lib/types";

/**
 * Memory panel — shows semantic-memory backend status, embedding index health,
 * and a retrieval probe interface. Adapted from Kirsten's MemoryPanel with
 * Gloss-compatible types, stores, and styling conventions.
 */
export function MemoryPanel() {
  const activeNotebookId = useNotebookStore((s) => s.activeNotebookId);
  const memoryStatus = useHealthStore((s) => s.memoryStatus);
  const profileStatus = useHealthStore((s) => s.profileStatus);
  const poll = useHealthStore((s) => s.poll);

  const [probeQuery, setProbeQuery] = useState("");
  const [probeResult, setProbeResult] = useState<RetrievalProbeReceipt | null>(null);
  const [probeLoading, setProbeLoading] = useState(false);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    if (activeNotebookId) {
      void poll();
    }
  }, [activeNotebookId, poll]);

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await poll();
    } finally {
      setRefreshing(false);
    }
  };

  const handleProbe = async () => {
    if (!probeQuery.trim() || !activeNotebookId) return;
    setProbeLoading(true);
    setProbeError(null);
    try {
      const result = await runRetrievalProbe(activeNotebookId, probeQuery, { kind: "all" }, 10);
      setProbeResult(result);
    } catch (err) {
      setProbeError(err instanceof Error ? err.message : String(err));
      setProbeResult(null);
    } finally {
      setProbeLoading(false);
    }
  };

  if (!activeNotebookId) {
    return (
      <div className="p-4 text-xs text-[var(--text-3)]">
        Select a notebook to view memory status.
      </div>
    );
  }

  return (
    <div className="p-3 space-y-3 text-xs overflow-y-auto h-full">
      {/* Header */}
      <div className="flex items-center gap-2">
        <Database size={14} style={{ color: "var(--iris-soft)" }} />
        <span className="font-medium">Memory Backend</span>
        <button
          className="ml-auto p-1 rounded hover:bg-[var(--hover)]"
          onClick={handleRefresh}
          disabled={refreshing}
          title="Refresh status"
        >
          <RefreshCw size={12} className={refreshing ? "animate-spin" : ""} />
        </button>
      </div>

      {/* Backend status */}
      {memoryStatus ? (
        <BackendStatusView status={memoryStatus} />
      ) : (
        <div className="text-[var(--text-4)]">No memory status available.</div>
      )}

      {/* Profile status */}
      {profileStatus && (
        <ProfileStatusView status={profileStatus} />
      )}

      {/* Retrieval probe */}
      <div className="space-y-2 pt-2 border-t border-[var(--hairline)]">
        <div className="flex items-center gap-2">
          <Search size={12} style={{ color: "var(--text-3)" }} />
          <span className="font-medium">Retrieval Probe</span>
        </div>
        <div className="flex gap-2">
          <input
            className="flex-1 bg-[var(--surface-2)] border border-[var(--hairline)] rounded px-2 py-1 text-xs outline-none focus:border-[var(--iris)]"
            placeholder="Test a retrieval query..."
            value={probeQuery}
            onChange={(e) => setProbeQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleProbe()}
            style={{ userSelect: "text" }}
          />
          <button
            className="px-3 py-1 rounded bg-[var(--iris)] text-white text-xs font-medium hover:opacity-90 disabled:opacity-50"
            onClick={handleProbe}
            disabled={probeLoading || !probeQuery.trim()}
          >
            {probeLoading ? "..." : "Probe"}
          </button>
        </div>

        {probeError && (
          <div className="text-[var(--coral)] break-words">{probeError}</div>
        )}

        {probeResult && (
          <ProbeResultView result={probeResult} />
        )}
      </div>
    </div>
  );
}

function BackendStatusView({ status }: { status: MemoryBackendStatus }) {
  const available = status.available;
  const semanticAvailable = status.semantic_memory_available;
  const degraded = status.degraded;
  const fallback = status.fallback_reason;

  return (
    <div className="space-y-1.5">
      <StatusRow label="Backend" value={status.backend_used} ok={available} />
      <StatusRow label="Semantic Memory" value={semanticAvailable ? "available" : "unavailable"} ok={semanticAvailable} />
      <StatusRow label="Index Sync" value={status.index_sync_status} ok={status.index_sync_status === "synced"} />
      <StatusRow label="Degraded" value={degraded ? "yes" : "no"} ok={!degraded} />
      {fallback && (
        <div className="text-[var(--amber)]">
          Fallback: {fallback}
        </div>
      )}
      {status.degradation_markers.length > 0 && (
        <div className="text-[var(--amber)]">
          Markers: {status.degradation_markers.join(", ")}
        </div>
      )}
      {status.embedding_index_metadata.length > 0 && (
        <div className="pt-1 space-y-1">
          <div className="text-[var(--text-2)] font-medium">Embedding Indexes</div>
          {status.embedding_index_metadata.map((idx, i) => (
            <div key={i} className="text-[var(--text-3)] pl-2">
              <span className={idx.status === "ready" ? "text-[var(--mint)]" : "text-[var(--amber)]"}>
                {idx.status}
              </span>{" "}
              — {idx.provider}/{idx.model}
              {idx.dimensions && ` (${idx.dimensions}d)`}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ProfileStatusView({ status }: { status: SemanticMemoryProfileStatus }) {
  return (
    <div className="space-y-1">
      <div className="font-medium text-[var(--text-2)]">Semantic Memory Profile</div>
      <StatusRow label="Selected" value={status.selected_backend} ok={true} />
      <StatusRow label="Effective" value={status.effective_backend} ok={true} />
      <StatusRow label="Strict" value={status.strict_testing ? "yes" : "no"} ok={status.strict_testing} />
      {status.compiled_semantic_memory && (
        <div className="text-[var(--mint)]">Semantic memory: compiled</div>
      )}
      {status.compiled_turbo_quant && (
        <div className="text-[var(--mint)]">TurboQuant: compiled</div>
      )}
      {status.blocking_reasons.length > 0 && (
        <div className="text-[var(--coral)]">
          Blocked: {status.blocking_reasons.join(", ")}
        </div>
      )}
      {status.next_actions.length > 0 && (
        <div className="text-[var(--text-3)]">
          Next: {status.next_actions.join("; ")}
        </div>
      )}
    </div>
  );
}

function StatusRow({ label, value, ok }: { label: string; value: string; ok: boolean }) {
  return (
    <div className="flex items-center gap-2">
      <span className="text-[var(--text-4)] w-28">{label}</span>
      <span className={ok ? "text-[var(--mint)]" : "text-[var(--coral)]"}>
        {ok && <Activity size={10} style={{ display: "inline", marginRight: 4 }} />}
        {value}
      </span>
    </div>
  );
}

function ProbeResultView({ result }: { result: RetrievalProbeReceipt }) {
  return (
    <div className="space-y-1 text-[var(--text-3)]">
      <div className="font-medium text-[var(--text-2)]">Probe Result</div>
      <div>Backend: {result.backend_used}</div>
      <div>Scope: {result.source_scope_kind} ({result.scoped_sources} sources, {result.scoped_chunks} chunks)</div>
      <div>BM25 candidates: {result.bm25_candidates}</div>
      <div>Vector candidates: {result.vector_candidates}</div>
      {result.tq_candidates > 0 && (
        <div>TurboQuant candidates: {result.tq_candidates}</div>
      )}
      <div>Exact rerank: {result.exact_rerank ? "yes" : "no"} ({result.exact_rerank_count})</div>
      {result.fallback_used && result.fallback_reason && (
        <div className="text-[var(--amber)]">Fallback: {result.fallback_reason}</div>
      )}
      {result.degradation_markers.length > 0 && (
        <div className="text-[var(--amber)]">
          Markers: {result.degradation_markers.join(", ")}
        </div>
      )}
      <div className="text-[var(--text-4)] text-[10px] pt-1">
        Receipt: {result.receipt_id}
      </div>
    </div>
  );
}