import { findSelectedModel, useSettingsStore } from "../../stores/settingsStore";
import { useSourceStore } from "../../stores/sourceStore";
import { useNotebookStore } from "../../stores/notebookStore";
import * as api from "../../lib/tauri";
import {
  Wifi, WifiOff, RefreshCw,
} from "lucide-react";
import { useState } from "react";
import { useHealthStore } from "../../stores/healthStore";

/**
 * Diagnostics panel — shows provider health, model availability,
 * memory backend status, index status, and semantic-memory/TurboQuant
 * compilation/runtime truth.
 */
export function DiagnosticsPanel({ notebookId }: { notebookId: string }) {
  const activeModel = useSettingsStore((s) => s.activeModel);
  const models = useSettingsStore((s) => s.models);
  const settings = useSettingsStore((s) => s.settings);
  const refreshModels = useSettingsStore((s) => s.refreshModels);
  const loading = useSettingsStore((s) => s.loading);
  const stats = useSourceStore((s) => s.stats);
  const selectedProviderId = settings["default_provider"] || null;
  const activeModelRecord = findSelectedModel(models, selectedProviderId, activeModel);
  const activeProviderId = selectedProviderId;
  const selectedModelPresent = Boolean(activeModelRecord);
  const selectedModelAvailable = Boolean(activeModelRecord && activeModelRecord.available && !activeModelRecord.stale);
  const selectedModelError = activeModelRecord?.last_error ?? null;

  const chatConnected = useHealthStore((s) => s.chatConnected);
  const memoryStatus = useHealthStore((s) => s.memoryStatus);
  const profileStatus = useHealthStore((s) => s.profileStatus);
  const queueStatus = useHealthStore((s) => s.queueStatus);
  const [refreshing, setRefreshing] = useState(false);
  const poll = useHealthStore((s) => s.poll);
  const [rebuilding, setRebuilding] = useState(false);
  const [rebuildError, setRebuildError] = useState<string | null>(null);
  const [rebuildReceipt, setRebuildReceipt] = useState<Awaited<ReturnType<typeof api.nativeDenseRebuild>> | null>(null);

  const handleRebuild = async () => {
    if (rebuilding) return;
    setRebuilding(true);
    setRebuildError(null);
    setRebuildReceipt(null);
    try {
      const receipt = await api.nativeDenseRebuild(notebookId);
      if (useNotebookStore.getState().activeNotebookId !== notebookId) return;
      setRebuildReceipt(receipt);
    } catch (error) {
      if (useNotebookStore.getState().activeNotebookId !== notebookId) return;
      setRebuildError(error instanceof Error ? error.message : String(error));
    } finally {
      if (useNotebookStore.getState().activeNotebookId === notebookId) {
        setRebuilding(false);
        await Promise.all([useSourceStore.getState().loadSources(notebookId), poll()]);
      }
    }
  };

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await refreshModels();
      await poll();
    } finally {
      setRefreshing(false);
    }
  };

  const memoryBackendLabel = memoryStatus
    ? memoryStatus.backend_used !== memoryStatus.active_backend
      ? `${memoryStatus.backend_used} (fallback from ${memoryStatus.active_backend})`
      : memoryStatus.active_backend
    : "unknown";

  return (
    <div className="p-3 space-y-3 text-xs overflow-y-auto h-full">
      {/* Refresh All */}
      <button
        onClick={handleRefresh}
        disabled={refreshing || loading}
        className="w-full flex items-center justify-center gap-1.5 rounded border border-border bg-bg-secondary px-2 py-1.5 text-[11px] text-text-muted hover:bg-bg-tertiary hover:text-text disabled:opacity-50"
      >
        <RefreshCw className={`w-3 h-3 ${refreshing ? "animate-spin" : ""}`} />
        {refreshing ? "Refreshing..." : "Refresh models & status"}
      </button>

      {/* Provider Health */}
      <Section title="Provider Health">
        <StatusRow
          label="Chat provider"
          value={activeProviderId ?? "not configured"}
          ok={chatConnected === true}
          warn={chatConnected === false}
          icon={chatConnected ? <Wifi className="w-3 h-3 text-success" /> : chatConnected === false ? <WifiOff className="w-3 h-3 text-error" /> : null}
        />
        <KVRow label="Selected model" value={activeModel} />
        <StatusRow
          label="Model available"
          value={selectedModelPresent ? (selectedModelAvailable ? "yes" : "no") : "not found"}
          ok={selectedModelAvailable}
          warn={selectedModelPresent && !selectedModelAvailable}
          error={!selectedModelPresent}
        />
        {selectedModelError && (
          <div className="rounded bg-error/10 px-2 py-1 text-error">
            {selectedModelError}
          </div>
        )}
        <KVRow label="Models in registry" value={`${models.filter(m => m.available && !m.stale).length}/${models.length}`} />
      </Section>

      {/* Memory Backend */}
      <Section title="Memory Backend">
        <StatusRow
          label="Backend"
          value={memoryBackendLabel}
          ok={memoryStatus?.active_backend === "semantic-memory-preview" && !memoryStatus?.degraded}
          warn={memoryStatus?.degraded || memoryStatus?.fallback_reason != null}
        />
        {memoryStatus?.fallback_reason && (
          <div className="rounded bg-warning/10 px-2 py-1 text-warning">
            Fallback: {memoryStatus.fallback_reason_code ?? "unclassified"} · {memoryStatus.fallback_reason}
          </div>
        )}
        <KVRow label="Default" value={memoryStatus?.default_backend ?? "—"} />
        <KVRow label="Index sync" value={memoryStatus?.index_sync_status ?? "unknown"} />
        {memoryStatus?.embedding_index_metadata?.map((metadata) => (
          <KVRow
            key={metadata.index_id}
            label={metadata.index_id}
            value={`${metadata.status} ${metadata.provider}:${metadata.model}${metadata.dimensions ? ` (${metadata.dimensions}d)` : ""}${metadata.status_reason ? ` - ${metadata.status_reason}` : ""}`}
            warn={["stale", "blocked", "unknown"].includes(metadata.status)}
          />
        ))}
        {memoryStatus?.degradation_markers.length ? (
          <KVRow label="Degradation" value={memoryStatus.degradation_markers.join(", ")} warn />
        ) : null}
      </Section>

      {/* Semantic Memory / TurboQuant Build Truth */}
      <Section title="Semantic Memory Build">
        <StatusRow
          label="Feature enabled"
          value={memoryStatus?.semantic_memory_feature_enabled ? "yes" : "no"}
          ok={memoryStatus?.semantic_memory_feature_enabled}
          warn={!memoryStatus?.semantic_memory_feature_enabled}
        />
        <StatusRow
          label="SM available"
          value={memoryStatus?.semantic_memory_available ? "yes" : "no"}
          ok={memoryStatus?.semantic_memory_available}
          warn={!memoryStatus?.semantic_memory_available && memoryStatus?.semantic_memory_feature_enabled}
        />
        <KVRow label="Compiled semantic" value={profileStatus?.compiled_semantic_memory ? "yes" : "no"} warn={!profileStatus?.compiled_semantic_memory} />
        <KVRow label="Compiled TQ" value={profileStatus?.compiled_turbo_quant ? "yes" : "no"} warn={!profileStatus?.compiled_turbo_quant} />
        {profileStatus?.projection_summary && (
          <KVRow
            label="Projection"
            value={`${profileStatus.projection_summary.projected_chunks}/${profileStatus.projection_summary.total_chunks} chunks`}
          />
        )}
        {profileStatus?.turbo_quant_status && (
          <KVRow
            label="TQ exact rerank"
            value={profileStatus.turbo_quant_status.exact_rerank ? `proven (${profileStatus.turbo_quant_status.exact_rerank_count})` : "not proven"}
            warn={!profileStatus.turbo_quant_status.exact_rerank}
          />
        )}
      </Section>

      {/* Index / Notebook Stats */}
      <Section title="Notebook Index">
        <p className="text-text-muted">Rebuild dense search from this notebook’s saved chunks using the configured embedding model. Sources and notes are preserved. Dense retrieval is unavailable during the rebuild.</p>
        <button onClick={() => void handleRebuild()} disabled={rebuilding}
          className="rounded border border-border px-2 py-1.5 text-text-secondary hover:bg-bg-tertiary disabled:opacity-50">
          {rebuilding ? "Rebuilding dense index…" : "Rebuild dense index"}
        </button>
        {rebuilding && <p role="status" className="text-text-muted">Rebuilding. Large notebooks may take several minutes.</p>}
        {rebuildError && <p role="alert" className="text-error">Dense rebuild failed: {rebuildError}</p>}
        {rebuildReceipt && <div role="status" className="rounded border border-border p-2">
          <p>Dense index {rebuildReceipt.status}: {rebuildReceipt.chunks_indexed} chunks.</p>
          <p className="break-words text-text-muted">{rebuildReceipt.provider} · {rebuildReceipt.model} · {rebuildReceipt.dimensions} dimensions</p>
          <details className="mt-1 text-text-muted"><summary>Rebuild receipt</summary>
            <p className="break-all">{rebuildReceipt.rebuild_id}</p>
            <p className="break-all">SHA-256: {rebuildReceipt.artifact_sha256}</p>
            <p>Previous artifact quarantined: {rebuildReceipt.previous_artifact_quarantined ? "yes" : "no"}</p>
          </details>
        </div>}
        <KVRow label="Sources" value={`${stats?.source_count ?? 0}`} />
        <KVRow label="Chunks" value={`${stats?.chunk_count ?? 0}`} />
        <KVRow label="Total words" value={`${(stats?.total_words ?? 0).toLocaleString()}`} />
        {stats && stats.error_count > 0 && (
          <KVRow label="Errors" value={`${stats.error_count}`} warn />
        )}
        {stats && stats.missing_summaries > 0 && (
          <KVRow label="Missing summaries" value={`${stats.missing_summaries}`} warn />
        )}
      </Section>

      {/* Summary Queue */}
      {queueStatus && (
        <Section title="Summary Queue">
          <KVRow label="Mode" value={queueStatus.paused ? "manual" : "auto"} />
          <KVRow label="Pending" value={`${queueStatus.pending}`} />
          <KVRow label="Processing" value={`${queueStatus.processing}`} />
          {queueStatus.summary_backend && (
            <>
              <KVRow label="Summary model" value={queueStatus.summary_backend.model ?? "—"} />
              <StatusRow
                label="Summary backend"
                value={queueStatus.summary_backend.ready ? "ready" : "not ready"}
                ok={queueStatus.summary_backend.ready}
                warn={!queueStatus.summary_backend.ready}
              />
              {queueStatus.summary_backend.diagnostic && (
                <div className="text-text-muted">{queueStatus.summary_backend.diagnostic}</div>
              )}
            </>
          )}
          {queueStatus.gate_owners.length > 0 && (
            <KVRow label="Gate owners" value={queueStatus.gate_owners.map(o => `${o.gate}: ${o.owner}`).join(", ")} />
          )}
        </Section>
      )}
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1.5">
      <div className="font-medium text-text">{title}</div>
      <div className="space-y-1 pl-1">{children}</div>
    </div>
  );
}

function KVRow({ label, value, warn, mono }: { label: string; value: string; warn?: boolean; mono?: boolean }) {
  return (
    <div className="grid grid-cols-[6rem_1fr] gap-1">
      <span className="text-text-muted truncate">{label}</span>
      <span className={`break-words ${mono ? "font-mono" : ""} ${warn ? "text-warning" : "text-text-secondary"}`}>
        {value}
      </span>
    </div>
  );
}

function StatusRow({
  label, value, ok, warn, error, icon,
}: {
  label: string; value: string; ok?: boolean; warn?: boolean; error?: boolean; icon?: React.ReactNode;
}) {
  const color = ok ? "text-success" : error ? "text-error" : warn ? "text-warning" : "text-text-secondary";
  return (
    <div className="grid grid-cols-[6rem_1fr] gap-1">
      <span className="text-text-muted truncate">{label}</span>
      <span className={`flex items-center gap-1 ${color}`}>
        {icon}
        {value}
      </span>
    </div>
  );
}
