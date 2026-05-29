import { useEffect, useRef } from "react";
import { NotebookSidebar } from "./components/notebooks/NotebookSidebar";
import { PanelLayout } from "./components/layout/PanelLayout";
import { StatusBar } from "./components/layout/StatusBar";
import { ToastContainer } from "./components/layout/ToastContainer";
import { useNotebookStore } from "./stores/notebookStore";
import { useSettingsStore } from "./stores/settingsStore";
import { useChatStore } from "./stores/chatStore";
import { useToastStore } from "./stores/toastStore";
import { onChatToken, onChatStatus, onChatError, onChatCancelled, onChatEvidence, onSourceStatus, onSourcesBatchCreated, onBatchIngestionComplete, onJobCompleted } from "./lib/events";
import { useSourceStore } from "./stores/sourceStore";
import { BookOpen, Database, Search, Sparkles } from "lucide-react";

export function App() {
  const { notebooks, activeNotebookId, loadNotebooks } = useNotebookStore();
  const { activeModel, models, settings, loadSettings, loadProviders, loadModels } = useSettingsStore();
  const stats = useSourceStore((s) => s.stats);
  const activeNotebook = notebooks.find((notebook) => notebook.id === activeNotebookId) ?? null;
  const activeProvider =
    settings["default_provider"] ||
    models.find((model) => model.id === activeModel)?.provider_id ||
    null;

  // --- Batching/debouncing refs ---
  const pendingStatusRef = useRef<Map<string, { status: string; errorMessage?: string }>>(new Map());
  const statusFlushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const statsDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const batchReadyCountRef = useRef(0);
  const batchToastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const batchCreatedDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const jobCompletedDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    loadNotebooks().then(() => {
      // Sync persisted activeNotebookId to backend on startup
      const nbId = useNotebookStore.getState().activeNotebookId;
      if (nbId) {
        const exists = useNotebookStore.getState().notebooks.some(n => n.id === nbId);
        if (exists) {
          void useNotebookStore.getState().setActive(nbId);
        } else {
          // Stale ID — notebook was deleted
          useNotebookStore.getState().setActive(null);
        }
      }
    });
    loadSettings();
    loadProviders();
    loadModels();
  }, []);

  // Listen for all Tauri events — consolidated cleanup
  useEffect(() => {
    const unlisteners: Promise<VoidFunction>[] = [];

    unlisteners.push(onChatToken((payload) => {
      const chatStore = useChatStore.getState();
      if (payload.token) {
        chatStore.appendToken(
          payload.notebook_id,
          payload.conversation_id,
          payload.message_id,
          payload.token
        );
      }
      if (payload.done) {
        chatStore.finalizeMessage(
          payload.notebook_id,
          payload.conversation_id,
          payload.message_id
        );
      }
    }));

    unlisteners.push(onChatStatus((payload) => {
      useChatStore.getState().setStreamingStatus(payload);
    }));

    unlisteners.push(onChatError((payload) => {
      const chatStore = useChatStore.getState();
      chatStore.setStreamingError(
        payload.notebook_id,
        payload.conversation_id,
        payload.message_id,
        payload.error
      );
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Chat Error',
        message: payload.error,
        duration: 8000,
      });
    }));

    unlisteners.push(onChatCancelled((payload) => {
      const chatStore = useChatStore.getState();
      chatStore.handleChatCancelled(
        payload.notebook_id,
        payload.conversation_id,
        payload.message_id,
        payload.reason
      );
    }));

    unlisteners.push(onChatEvidence((payload) => {
      useChatStore.getState().attachAssistantEvidence(
        payload.notebook_id,
        payload.conversation_id,
        payload.message_id,
        { citations: payload.citations, evidence: payload.evidence }
      );
    }));

    unlisteners.push(onSourceStatus((payload) => {
      const currentNotebookId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id !== currentNotebookId) return;

      // Accumulate into pending map (latest status wins per source)
      pendingStatusRef.current.set(payload.source_id, {
        status: payload.status,
        errorMessage: payload.error_message,
      });

      // Flush accumulated updates every 75ms
      if (statusFlushTimerRef.current == null) {
        statusFlushTimerRef.current = setTimeout(() => {
          statusFlushTimerRef.current = null;
          const updates = Array.from(pendingStatusRef.current.entries()).map(
            ([sourceId, v]) => ({ sourceId, status: v.status, errorMessage: v.errorMessage })
          );
          pendingStatusRef.current.clear();
          if (updates.length > 0) {
            useSourceStore.getState().updateSourceStatusBulk(updates);
          }
        }, 75);
      }

      // Debounced stats reload (1s after last ready/error)
      if (payload.status === "ready" || payload.status === "error") {
        if (statsDebounceRef.current) clearTimeout(statsDebounceRef.current);
        statsDebounceRef.current = setTimeout(() => {
          statsDebounceRef.current = null;
          const nbId = useNotebookStore.getState().activeNotebookId;
          if (nbId) useSourceStore.getState().loadStats(nbId);
        }, 1000);
      }

      // Aggregate "ready" toasts — single toast after 2s of quiet
      if (payload.status === "ready") {
        batchReadyCountRef.current++;
        if (batchToastTimerRef.current) clearTimeout(batchToastTimerRef.current);
        batchToastTimerRef.current = setTimeout(() => {
          batchToastTimerRef.current = null;
          const count = batchReadyCountRef.current;
          batchReadyCountRef.current = 0;
          useToastStore.getState().addToast({
            type: 'success',
            title: 'Ingestion Complete',
            message: count === 1 ? 'Source ingestion complete' : `${count} sources ingested`,
            duration: 3000,
          });
        }, 2000);
      }

      // Individual error toasts are still important (capped by Fix 1)
      if (payload.status === "error" && payload.error_message) {
        useToastStore.getState().addToast({
          type: 'error',
          title: 'Source Ingestion Failed',
          message: payload.error_message,
          duration: 8000,
        });
      }
    }));

    unlisteners.push(onSourcesBatchCreated((payload) => {
      const nbId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id === nbId) {
        if (batchCreatedDebounceRef.current) clearTimeout(batchCreatedDebounceRef.current);
        useSourceStore.getState().markSourceListPartial(payload.count);
        batchCreatedDebounceRef.current = setTimeout(() => {
          batchCreatedDebounceRef.current = null;
          void useNotebookStore.getState().loadNotebooks();
          useSourceStore.getState().loadStats(nbId);
          useSourceStore.getState().loadSources(nbId);
        }, 500);
      }
    }));

    unlisteners.push(onJobCompleted((payload) => {
      if (!payload.output) return;
      try {
        const data = JSON.parse(payload.output) as { notebook_id?: string };
        const nbId = useNotebookStore.getState().activeNotebookId;
        if (data.notebook_id && data.notebook_id === nbId) {
          if (jobCompletedDebounceRef.current) clearTimeout(jobCompletedDebounceRef.current);
            jobCompletedDebounceRef.current = setTimeout(() => {
              jobCompletedDebounceRef.current = null;
              useSourceStore.getState().loadSources(nbId);
              useSourceStore.getState().loadStats(nbId);
              useChatStore.getState().clearSuggestedQuestions();
            }, 3000);
          }
      } catch {
        // Ignore unparseable output
      }
    }));

    unlisteners.push(onBatchIngestionComplete((payload) => {
      const nbId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id === nbId) {
        void useNotebookStore.getState().loadNotebooks();
        useSourceStore.getState().loadSources(nbId);
        useSourceStore.getState().loadStats(nbId);
        useChatStore.getState().clearSuggestedQuestions();
        const ready = payload.ingested_ready ?? payload.count;
        const failed = payload.failed ?? 0;
        const cancelled = payload.cancelled_superseded ?? 0;
        const skipped = (payload.skipped_duplicate ?? 0) + (payload.skipped_unsupported ?? 0);
        const status = payload.status ?? 'completed';
        const toastType = status === 'completed' && failed === 0 && cancelled === 0 ? 'success' : 'warning';
        const perf = payload.performance;
        const elapsed = perf ? `${Math.max(0.1, perf.elapsed_ms / 1000).toFixed(1)}s` : null;
        const suffix = [
          failed ? `${failed} failed` : null,
          skipped ? `${skipped} skipped` : null,
          cancelled ? `${cancelled} cancelled` : null,
          elapsed ? `${elapsed}` : null,
        ].filter(Boolean).join(', ');
        useToastStore.getState().addToast({
          type: toastType,
          title: status === 'cancelled_superseded' ? 'Folder Import Cancelled' : 'Folder Import Complete',
          message: suffix ? `${ready} sources ready; ${suffix}` : `${ready} sources ready`,
          duration: 5000,
        });
      }
    }));

    return () => {
      unlisteners.forEach(p => p.then(fn => fn()));
      // Flush any pending status updates before clearing timers
      if (statusFlushTimerRef.current) {
        clearTimeout(statusFlushTimerRef.current);
        statusFlushTimerRef.current = null;
        const updates = Array.from(pendingStatusRef.current.entries()).map(
          ([sourceId, v]) => ({ sourceId, status: v.status, errorMessage: v.errorMessage })
        );
        pendingStatusRef.current.clear();
        if (updates.length > 0) {
          useSourceStore.getState().updateSourceStatusBulk(updates);
        }
      }
      if (statsDebounceRef.current) clearTimeout(statsDebounceRef.current);
      if (batchToastTimerRef.current) clearTimeout(batchToastTimerRef.current);
      if (batchCreatedDebounceRef.current) clearTimeout(batchCreatedDebounceRef.current);
      if (jobCompletedDebounceRef.current) clearTimeout(jobCompletedDebounceRef.current);
    };
  }, []);

  return (
    <div className="gloss-root flex h-screen flex-col bg-bg">
      <GlossTopBar
        activeNotebookName={activeNotebook?.name ?? null}
        activeModel={activeModel}
        activeProvider={activeProvider}
        sourceCount={stats?.source_count ?? activeNotebook?.source_count ?? 0}
        chunkCount={stats?.chunk_count ?? 0}
      />
      <div className="flex flex-1 overflow-hidden">
        <NotebookSidebar />
        {activeNotebookId ? (
          <PanelLayout key={activeNotebookId} notebookId={activeNotebookId} />
        ) : (
          <div className="flex-1 flex items-center justify-center text-text-muted">
            <div className="text-center">
              <h2 className="text-2xl font-semibold mb-2">Welcome to Gloss</h2>
              <p className="text-text-secondary">
                Create or select a notebook to get started
              </p>
            </div>
          </div>
        )}
      </div>
      <StatusBar />
      <ToastContainer />
    </div>
  );
}

function GlossTopBar({
  activeNotebookName,
  activeModel,
  activeProvider,
  sourceCount,
  chunkCount,
}: {
  activeNotebookName: string | null;
  activeModel: string;
  activeProvider: string | null;
  sourceCount: number;
  chunkCount: number;
}) {
  return (
    <div className="gloss-topbar flex shrink-0 items-center gap-3 px-3 text-xs text-text-muted">
      <div className="flex items-center gap-2">
        <span className="gloss-mark">
          <Sparkles className="h-3.5 w-3.5" />
        </span>
        <span className="gloss-serif text-[18px] text-text">Gloss</span>
      </div>

      <div className="gloss-pill max-w-[260px]">
        <BookOpen className="h-3.5 w-3.5 text-accent" />
        <span className="truncate text-text-secondary">
          {activeNotebookName ?? "No notebook selected"}
        </span>
        <span className="text-text-muted">{sourceCount} sources</span>
      </div>

      <div className="hidden h-7 max-w-[520px] flex-1 items-center gap-2 rounded border border-border bg-bg-secondary px-3 text-text-muted md:flex">
        <Search className="h-3.5 w-3.5" />
        <span className="truncate">Search across notebook</span>
        <span className="gloss-mono ml-auto rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted">
          Cmd K
        </span>
      </div>

      <span className="flex-1" />

      <div className="gloss-pill gloss-pill-accent max-w-[320px]">
        <span className="gloss-status-dot ok" />
        <span className="truncate text-text-secondary">{activeModel}</span>
        {activeProvider && <span className="text-text-muted">{activeProvider}</span>}
      </div>

      <div className="gloss-pill hidden lg:inline-flex">
        <Database className="h-3.5 w-3.5" />
        <span>{chunkCount.toLocaleString()} chunks</span>
      </div>
    </div>
  );
}
