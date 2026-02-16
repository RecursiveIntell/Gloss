import { useSettingsStore } from "../../stores/settingsStore";
import { useSourceStore } from "../../stores/sourceStore";
import { useNotebookStore } from "../../stores/notebookStore";
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
  const stats = useSourceStore((s) => s.stats);
  const activeNotebookId = useNotebookStore((s) => s.activeNotebookId);
  const [connected, setConnected] = useState(false);
  const [embeddingStatus, setEmbeddingStatus] = useState<string | null>(null);
  const [queueStatus, setQueueStatus] = useState<QueueStatus | null>(null);
  const [generating, setGenerating] = useState(false);
  const testProvider = useSettingsStore((s) => s.testProvider);

  useEffect(() => {
    testProvider("ollama").then(setConnected);
    const interval = setInterval(() => {
      testProvider("ollama").then(setConnected);
    }, 30000);
    return () => clearInterval(interval);
  }, []);

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
      await api.regenerateMissingSummaries(activeNotebookId);
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
  // Show "needs summaries" when queue is idle but sources still need them
  const needsSummaries = !isProcessing && !isPaused && missingSummaries > 0;

  return (
    <div className="h-7 bg-bg-secondary border-t border-border flex items-center px-3 text-xs text-text-muted gap-4">
      <div className="flex items-center gap-1.5">
        {connected ? (
          <Wifi className="w-3 h-3 text-success" />
        ) : (
          <WifiOff className="w-3 h-3 text-error" />
        )}
        <span>{connected ? "Connected" : "Disconnected"}</span>
      </div>
      <div className="flex items-center gap-1.5">
        <span>Model: {activeModel}</span>
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
          {isPaused
            ? `Paused${pendingCount > 0 ? ` (${pendingCount} queued)` : ""}`
            : isProcessing
              ? `${pendingCount} ${pendingCount === 1 ? "summary" : "summaries"} running`
              : needsSummaries
                ? `${missingSummaries} need ${missingSummaries === 1 ? "summary" : "summaries"}`
                : "Idle"}
        </span>

        {/* Generate button — shown when summaries are missing and queue is idle */}
        {needsSummaries && connected && (
          <button
            onClick={handleGenerate}
            disabled={generating}
            className="px-1.5 py-0.5 rounded bg-accent/20 text-accent hover:bg-accent/30 disabled:opacity-50"
            title="Generate missing summaries"
          >
            {generating ? "Queuing..." : "Generate"}
          </button>
        )}

        {/* Pause/Resume — shown when processing or paused */}
        {(isProcessing || isPaused) && (
          <button
            onClick={handleTogglePause}
            className="p-0.5 rounded hover:bg-bg-tertiary text-text-muted hover:text-text"
            title={isPaused ? "Resume summaries" : "Pause summaries"}
          >
            {isPaused ? (
              <Play className="w-3 h-3" />
            ) : (
              <Pause className="w-3 h-3" />
            )}
          </button>
        )}
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
