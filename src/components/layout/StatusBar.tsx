import { useSettingsStore } from "../../stores/settingsStore";
import { useSourceStore } from "../../stores/sourceStore";
import { useNotebookStore } from "../../stores/notebookStore";
import { useToastStore } from "../../stores/toastStore";
import { onEmbeddingModelStatus } from "../../lib/events";
import * as api from "../../lib/tauri";
import type { QueueStatus } from "../../lib/types";
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

export function StatusBar() {
  const activeModel = useSettingsStore((s) => s.activeModel);
  const models = useSettingsStore((s) => s.models);
  const stats = useSourceStore((s) => s.stats);
  const activeNotebookId = useNotebookStore((s) => s.activeNotebookId);
  const [chatConnected, setChatConnected] = useState(false);
  const [backgroundConnected, setBackgroundConnected] = useState(false);
  const [embeddingStatus, setEmbeddingStatus] = useState<string | null>(null);
  const [queueStatus, setQueueStatus] = useState<QueueStatus | null>(null);
  const [generating, setGenerating] = useState(false);
  const testProvider = useSettingsStore((s) => s.testProvider);
  const activeProviderId =
    models.find((model) => model.id === activeModel)?.provider_id ?? "ollama";
  const backgroundProviderId = queueStatus?.summary_backend.provider_id ?? null;

  useEffect(() => {
    testProvider(activeProviderId).then(setChatConnected);
    const interval = setInterval(() => {
      testProvider(activeProviderId).then(setChatConnected);
    }, 30000);
    return () => clearInterval(interval);
  }, [activeProviderId, testProvider]);

  useEffect(() => {
    if (!backgroundProviderId || !queueStatus?.summary_backend.ready) {
      setBackgroundConnected(false);
      return;
    }

    testProvider(backgroundProviderId).then(setBackgroundConnected);
    const interval = setInterval(() => {
      testProvider(backgroundProviderId).then(setBackgroundConnected);
    }, 30000);
    return () => clearInterval(interval);
  }, [backgroundProviderId, queueStatus?.summary_backend.ready, testProvider]);

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

  // Poll queue status + refresh stats when notebook changes or periodically
  useEffect(() => {
    const poll = () => {
      api.getQueueStatus().then(setQueueStatus).catch(() => {});
      if (activeNotebookId) {
        useSourceStore.getState().loadStats(activeNotebookId);
      }
    };
    poll();
    const interval = setInterval(poll, 5000);
    return () => clearInterval(interval);
  }, [activeNotebookId]);

  const handleTogglePause = useCallback(async () => {
    try {
      if (queueStatus?.paused) {
        await api.resumeSummaries();
      } else {
        await api.pauseSummaries();
      }
      const status = await api.getQueueStatus();
      setQueueStatus(status);
    } catch (e) {
      console.error("Failed to toggle summary pause:", e);
    }
  }, [queueStatus]);

  const handleGenerate = useCallback(async () => {
    if (!activeNotebookId || generating) return;
    setGenerating(true);
    try {
      if (queueStatus?.paused) {
        await api.resumeSummaries();
      }
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
      setQueueStatus(status);
    } catch (e) {
      console.error("Failed to generate summaries:", e);
    } finally {
      setGenerating(false);
    }
  }, [activeNotebookId, generating]);

  const pendingCount = queueStatus
    ? queueStatus.pending + queueStatus.processing
    : 0;
  const missingSummaries = stats?.missing_summaries ?? 0;
  const isProcessing = pendingCount > 0;
  const isPaused = queueStatus?.paused ?? false;
  const isManualMode = queueStatus?.mode === "manual" || isPaused;
  const needsSummaries = !isProcessing && missingSummaries > 0;
  const summaryBackendReady = queueStatus?.summary_backend.ready ?? false;
  const canGenerate = needsSummaries && summaryBackendReady && backgroundConnected;
  const summaryDiagnostic = queueStatus?.summary_backend.diagnostic ?? null;
  const backgroundStatus = !summaryBackendReady
    ? "Config error"
    : backgroundConnected
      ? "Ready"
      : "Disconnected";
  const summaryModelLabel = queueStatus?.summary_backend.model ?? "Not configured";

  return (
    <div className="h-7 bg-bg-secondary border-t border-border flex items-center px-3 text-xs text-text-muted gap-4">
      <div className="flex items-center gap-1.5">
        {chatConnected ? (
          <Wifi className="w-3 h-3 text-success" />
        ) : (
          <WifiOff className="w-3 h-3 text-error" />
        )}
        <span>{chatConnected ? "Chat connected" : "Chat disconnected"}</span>
      </div>
      <div className="flex items-center gap-1.5">
        <span>Chat provider: {activeProviderId}</span>
      </div>
      <div className="flex items-center gap-1.5">
        <span>Chat model: {activeModel}</span>
      </div>
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
            {stats.chunk_count > 0 && ` \u00B7 ${stats.chunk_count} chunks`}
            {stats.total_words > 0 &&
              ` \u00B7 ${stats.total_words.toLocaleString()} words`}
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
