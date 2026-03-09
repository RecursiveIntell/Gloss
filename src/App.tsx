import { useEffect, useRef } from "react";
import { NotebookSidebar } from "./components/notebooks/NotebookSidebar";
import { PanelLayout } from "./components/layout/PanelLayout";
import { StatusBar } from "./components/layout/StatusBar";
import { ToastContainer } from "./components/layout/ToastContainer";
import { useNotebookStore } from "./stores/notebookStore";
import { useSettingsStore } from "./stores/settingsStore";
import { useChatStore } from "./stores/chatStore";
import { useToastStore } from "./stores/toastStore";
import { onChatToken, onChatError, onSourceStatus, onSourcesBatchCreated, onBatchIngestionComplete, onJobCompleted } from "./lib/events";
import { useSourceStore } from "./stores/sourceStore";
import { setActiveNotebook } from "./lib/tauri";

const EAGER_BATCH_SOURCE_LOAD_LIMIT = 200;

export function App() {
  const { activeNotebookId, loadNotebooks } = useNotebookStore();
  const { loadSettings, loadProviders, loadModels } = useSettingsStore();

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
          // Re-notify backend (frontend already has it from localStorage)
          setActiveNotebook(nbId);
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

  // Listen for chat token events
  useEffect(() => {
    const unlisten = onChatToken((payload) => {
      const activeNotebookId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id !== activeNotebookId) return;
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
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Listen for chat error events
  useEffect(() => {
    const unlisten = onChatError((payload) => {
      const activeNotebookId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id !== activeNotebookId) return;
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
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Listen for source status events — BATCHED + THROTTLED
  useEffect(() => {
    const unlisten = onSourceStatus((payload) => {
      const activeNotebookId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id !== activeNotebookId) return;

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
    });

    return () => {
      unlisten.then(fn => fn());
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
    };
  }, []);

  // Listen for batch source creation (folder imports Phase 1) — DEBOUNCED
  useEffect(() => {
    const unlisten = onSourcesBatchCreated((payload) => {
      const nbId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id === nbId) {
        if (batchCreatedDebounceRef.current) clearTimeout(batchCreatedDebounceRef.current);
        batchCreatedDebounceRef.current = setTimeout(() => {
          batchCreatedDebounceRef.current = null;
          useSourceStore.getState().loadStats(nbId);
          if (payload.count <= EAGER_BATCH_SOURCE_LOAD_LIMIT) {
            useSourceStore.getState().loadSources(nbId);
          }
        }, 500);
      }
    });

    return () => {
      unlisten.then(fn => fn());
      if (batchCreatedDebounceRef.current) clearTimeout(batchCreatedDebounceRef.current);
    };
  }, []);

  // Listen for job completion events — DEBOUNCED (3s)
  useEffect(() => {
    const unlisten = onJobCompleted((payload) => {
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
          }, 3000);
        }
      } catch {
        // Ignore unparseable output
      }
    });

    return () => {
      unlisten.then(fn => fn());
      if (jobCompletedDebounceRef.current) clearTimeout(jobCompletedDebounceRef.current);
    };
  }, []);

  // Listen for batch ingestion complete — single reload after all sources finish
  useEffect(() => {
    const unlisten = onBatchIngestionComplete((payload) => {
      const nbId = useNotebookStore.getState().activeNotebookId;
      if (payload.notebook_id === nbId) {
        useSourceStore.getState().loadSources(nbId);
        useSourceStore.getState().loadStats(nbId);
        useToastStore.getState().addToast({
          type: 'success',
          title: 'Folder Import Complete',
          message: `${payload.count} sources ingested`,
          duration: 5000,
        });
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  return (
    <div className="flex flex-col h-screen bg-bg">
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
