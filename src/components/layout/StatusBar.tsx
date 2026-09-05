import { findSelectedModel, useSettingsStore } from "../../stores/settingsStore";
import { useSourceStore } from "../../stores/sourceStore";
import { useNotebookStore } from "../../stores/notebookStore";
import { useToastStore } from "../../stores/toastStore";
import { onEmbeddingModelStatus } from "../../lib/events";
import * as api from "../../lib/tauri";
import {
  Wifi,
  WifiOff,
  Database,
  AlertTriangle,
  Pause,
  Play,
  Loader2,
  Sparkles,
} from "lucide-react";
import { useState, useEffect, useCallback } from "react";
import { useHealthStore } from "../../stores/healthStore";

export function StatusBar() {
  const activeModel = useSettingsStore((s) => s.activeModel);
  const models = useSettingsStore((s) => s.models);
  const settings = useSettingsStore((s) => s.settings);
  const stats = useSourceStore((s) => s.stats);
  const activeNotebookId = useNotebookStore((s) => s.activeNotebookId);
  const chatConnected = useHealthStore((s) => s.chatConnected);
  const backgroundConnected = useHealthStore((s) => s.backgroundConnected);
  const [embeddingStatus, setEmbeddingStatus] = useState<string | null>(null);
  const queueStatus = useHealthStore((s) => s.queueStatus);
  const memoryStatus = useHealthStore((s) => s.memoryStatus);
  const profileStatus = useHealthStore((s) => s.profileStatus);
  const startHealthPolling = useHealthStore((s) => s.startPolling);
  const stopHealthPolling = useHealthStore((s) => s.stopPolling);
  const [generating, setGenerating] = useState(false);
  const [healthOpen, setHealthOpen] = useState(false);
  const selectedProviderId = settings["default_provider"] || null;
  const activeModelRecord = findSelectedModel(models, selectedProviderId, activeModel);
  const activeProviderId = selectedProviderId;
  const selectedModelPresent = Boolean(activeModelRecord);
  const selectedModelAvailable = Boolean(
    activeModelRecord && activeModelRecord.available && !activeModelRecord.stale,
  );
  const selectedModelIssue = !selectedModelPresent
    ? "Selected model missing"
    : !selectedModelAvailable
      ? activeModelRecord?.last_error || "Selected model unavailable"
      : null;
  useEffect(() => {
    startHealthPolling(activeNotebookId, activeProviderId);
    return stopHealthPolling;
  }, [activeNotebookId, activeProviderId, startHealthPolling, stopHealthPolling]);

  // Listen for embedding model status events
  useEffect(() => {
    const unlisten = onEmbeddingModelStatus((payload) => {
      if (payload.state === "downloading") {
        setEmbeddingStatus(payload.message);
      } else {
        setEmbeddingStatus(null);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleTogglePause = useCallback(async () => {
    try {
      if (queueStatus?.paused) {
        await api.resumeSummaries();
      } else {
        await api.pauseSummaries();
      }
      const status = await api.getQueueStatus();
      useHealthStore.setState({ queueStatus: status });
    } catch (e) {
      console.warn("Failed to toggle summary pause:", e);
    }
  }, [queueStatus]);

  const handleGenerate = useCallback(async () => {
    if (!activeNotebookId || generating) return;
    setGenerating(true);
    try {
      const result = await api.regenerateMissingSummaries(activeNotebookId);
      if (result.queued === 0 && result.diagnostics.length > 0) {
        useToastStore.getState().addToast({
          type: "error",
          title: "Summary Queue Blocked",
          message: result.diagnostics.join(" "),
          duration: 6000,
        });
      }
      // Refresh queue status immediately
      const status = await api.getQueueStatus();
      useHealthStore.setState({ queueStatus: status });
    } catch (e) {
      console.warn("Failed to generate summaries:", e);
    } finally {
      setGenerating(false);
    }
  }, [activeNotebookId, generating]);

  const pendingCount = queueStatus
    ? queueStatus.pending + queueStatus.processing
    : 0;
  const gateOwnerLabel = queueStatus?.gate_owners.length
    ? queueStatus.gate_owners
        .map((owner) => `${owner.gate}: ${owner.owner} (${owner.detail})`)
        .join(", ")
    : null;
  const missingSummaries = stats?.missing_summaries ?? 0;
  const isProcessing = pendingCount > 0;
  const isPaused = queueStatus?.paused ?? false;
  const isManualMode = queueStatus?.mode === "manual" || isPaused;
  const needsSummaries = !isProcessing && missingSummaries > 0;
  const summaryBackendReady = queueStatus?.summary_backend.ready ?? false;
  const canGenerate = needsSummaries && summaryBackendReady && backgroundConnected === true;
  const summaryDiagnostic = queueStatus?.summary_backend.diagnostic ?? null;
  const memoryBackendLabel = memoryStatus
    ? memoryStatus.backend_used !== memoryStatus.active_backend
      ? `${memoryStatus.backend_used} fallback`
      : memoryStatus.active_backend
    : "gloss-local";
  const memoryTooltip =
    memoryStatus?.fallback_reason ||
    memoryStatus?.diagnostic ||
    memoryStatus?.semantic_memory_path ||
    undefined;
  const backgroundStatus = !queueStatus || backgroundConnected === null
    ? "Not checked"
    : !summaryBackendReady
    ? "Config error"
    : backgroundConnected
      ? "Ready"
      : "Disconnected";
  const summaryModelLabel = queueStatus?.summary_backend.model ?? "Not configured";

  return (
    <div className="gloss-mono flex min-h-[var(--statusbar-h)] shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-bg-secondary px-3 py-1 text-[10px] text-text-muted">
      <div className="flex items-center gap-1.5">
        {chatConnected ? (
          <Wifi className="w-3 h-3 text-success" />
        ) : chatConnected === false ? (
          <WifiOff className="w-3 h-3 text-error" />
        ) : <Loader2 className="w-3 h-3 text-text-muted" />}
        <span>
          {activeProviderId
            ? chatConnected === null ? "Provider not checked" : chatConnected
              ? "Provider reachable"
              : "Provider unreachable"
            : "Provider unknown"}
        </span>
      </div>
      <div className="flex items-center gap-1.5">
        <span>Chat provider: {activeProviderId ?? "unknown"}</span>
      </div>
      <div
        className={`flex items-center gap-1.5 ${selectedModelAvailable ? "" : "text-warning"}`}
        title={selectedModelIssue ?? undefined}
      >
        {!selectedModelAvailable && <AlertTriangle className="w-3 h-3 text-warning" />}
        <span>
          Chat model: {activeModel}
          {selectedModelIssue ? ` (${selectedModelIssue})` : ""}
        </span>
      </div>
      <button
        onClick={() => setHealthOpen((open) => !open)}
        className="relative flex items-center gap-1.5 hover:text-text"
        title={memoryTooltip}
        aria-label="Health status"
      >
        {memoryStatus?.degraded ? (
          <AlertTriangle className="w-3 h-3 text-warning" />
        ) : (
          <Database className="w-3 h-3" />
        )}
        <span>
          Memory: {memoryBackendLabel}
          {memoryStatus?.index_sync_status &&
            memoryStatus.index_sync_status !== "unknown" &&
            ` (${memoryStatus.index_sync_status})`}
        </span>
        {healthOpen && (
          <div className="absolute bottom-7 left-0 z-40 w-80 rounded border border-border bg-bg-secondary p-3 text-left text-xs shadow-xl">
            <div className="mb-2 font-medium text-text">Health</div>
            <div className="space-y-1 text-text-secondary">
              <HealthLine label="Backend requested" value={memoryStatus?.active_backend ?? "gloss-local"} />
              <HealthLine label="Backend used" value={memoryStatus?.backend_used ?? "gloss-local"} />
              <HealthLine label="Default backend" value={memoryStatus?.default_backend ?? "gloss-local"} />
              <HealthLine label="Fallback" value={memoryStatus?.fallback_reason ?? "none"} />
              <HealthLine label="Fallback code" value={memoryStatus?.fallback_reason_code ?? "none"} />
              <HealthLine label="Index state" value={memoryStatus?.index_sync_status ?? "unknown"} />
              {memoryStatus?.embedding_index_metadata?.map((metadata) => (
                <HealthLine
                  key={metadata.index_id}
                  label={metadata.index_id}
                  value={`${metadata.status} ${metadata.provider}:${metadata.model}${metadata.dimensions ? ` (${metadata.dimensions}d)` : ""}`}
                />
              ))}
              <HealthLine label="Sources" value={`${stats?.source_count ?? 0}`} />
              <HealthLine label="Preview feature" value={memoryStatus?.semantic_memory_feature_enabled ? "enabled" : "disabled"} />
              <HealthLine label="Preview availability" value={memoryStatus?.semantic_memory_available ? "available" : "not active"} />
              <HealthLine label="Compiled semantic" value={profileStatus?.compiled_semantic_memory ? "yes" : "no"} />
              <HealthLine label="Compiled TQ" value={profileStatus?.compiled_turbo_quant ? "yes" : "no"} />
              <HealthLine
                label="Projection"
                value={
                  profileStatus?.projection_summary
                    ? `${profileStatus.projection_summary.projected_chunks}/${profileStatus.projection_summary.total_chunks} chunks`
                    : "unknown"
                }
              />
              <HealthLine
                label="TQ proof"
                value={
                  profileStatus?.turbo_quant_status?.exact_rerank
                    ? `exact ${profileStatus.turbo_quant_status.exact_rerank_count}`
                    : "not proven"
                }
              />
              {memoryStatus?.degradation_markers.length ? (
                <HealthLine label="Degraded" value={memoryStatus.degradation_markers.join(", ")} />
              ) : (
                <HealthLine label="Degraded" value="no" />
              )}
            </div>
          </div>
        )}
      </button>
      <div
        className="flex items-center gap-1.5"
        title={summaryDiagnostic || summaryModelLabel}
      >
        {!summaryBackendReady ? (
          <AlertTriangle className="w-3 h-3 text-warning" />
        ) : backgroundConnected ? (
          <Wifi className="w-3 h-3 text-success" />
        ) : (
          <WifiOff className="w-3 h-3 text-error" />
        )}
        <span>Background: {backgroundStatus}</span>
      </div>

      {embeddingStatus && (
        <div className="flex items-center gap-1.5 text-accent">
          <span className="animate-pulse">{embeddingStatus}</span>
        </div>
      )}

      {gateOwnerLabel && (
        <div className="flex items-center gap-1.5 text-text-muted" title={gateOwnerLabel}>
          <span>Runtime: {gateOwnerLabel}</span>
        </div>
      )}

      {/* Summary queue status — always visible */}
      <div className="flex items-center gap-1.5">
        {/* Status icon */}
        {isPaused ? (
          <Pause className="w-3 h-3 text-warning" />
        ) : isProcessing ? (
          <Loader2 className="w-3 h-3 animate-spin text-accent" />
        ) : needsSummaries ? (
          <Sparkles className="w-3 h-3 text-warning" />
        ) : null}

        {/* Status text */}
        <span
          className={
            isPaused
              ? "text-warning"
              : isProcessing
                ? "text-accent"
                : needsSummaries
                  ? "text-warning"
                  : "text-text-muted"
          }
        >
          {isManualMode
            ? missingSummaries > 0
              ? `Manual - ${missingSummaries} need ${missingSummaries === 1 ? "summary" : "summaries"}`
              : `Manual${pendingCount > 0 ? ` (${pendingCount} queued)` : ""}`
            : isProcessing
              ? `${pendingCount} ${pendingCount === 1 ? "summary" : "summaries"} running`
              : needsSummaries
                ? `${missingSummaries} need ${missingSummaries === 1 ? "summary" : "summaries"}`
                : "Idle"}
        </span>

        {needsSummaries && (
          <button
            onClick={handleGenerate}
            disabled={!canGenerate || generating}
            className="px-1.5 py-0.5 rounded bg-accent/20 text-accent hover:bg-accent/30 disabled:opacity-50"
            title={canGenerate ? "Generate missing summaries" : summaryDiagnostic || "Background summary backend is not ready"}
          >
            {generating ? "Queuing..." : "Generate"}
          </button>
        )}

        <button
          onClick={handleTogglePause}
          className="p-0.5 rounded hover:bg-bg-tertiary text-text-muted hover:text-text"
          title={isPaused ? "Switch to automatic summaries" : "Switch to manual summary mode"}
        >
          {isPaused ? (
            <Play className="w-3 h-3" />
          ) : (
            <Pause className="w-3 h-3" />
          )}
        </button>
      </div>

      {stats && (
        <div className="flex items-center gap-1.5 ml-auto">
          <Database className="w-3 h-3" />
          <span>
            {stats.source_count} sources
            {stats.chunk_count > 0 && ` · ${stats.chunk_count} chunks`}
            {stats.total_words > 0 &&
              ` · ${stats.total_words.toLocaleString()} words`}
          </span>
          {stats.error_count > 0 && (
            <span className="text-error flex items-center gap-0.5">
              <AlertTriangle className="w-3 h-3" />
              {stats.error_count} errors
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function HealthLine({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid grid-cols-[7.5rem_1fr] gap-2">
      <span className="text-text-muted">{label}</span>
      <span className="break-words text-text-secondary">{value}</span>
    </div>
  );
}
