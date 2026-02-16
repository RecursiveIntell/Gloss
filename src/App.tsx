import { useEffect } from "react";
import { NotebookSidebar } from "./components/notebooks/NotebookSidebar";
import { PanelLayout } from "./components/layout/PanelLayout";
import { StatusBar } from "./components/layout/StatusBar";
import { useNotebookStore } from "./stores/notebookStore";
import { useSettingsStore } from "./stores/settingsStore";
import { useChatStore } from "./stores/chatStore";
import { onChatToken, onChatError, onSourceStatus, onJobCompleted } from "./lib/events";
import { useSourceStore } from "./stores/sourceStore";
import { setActiveNotebook, regenerateMissingSummaries } from "./lib/tauri";

export function App() {
  const { activeNotebookId, loadNotebooks } = useNotebookStore();
  const { loadSettings, loadProviders, loadModels } = useSettingsStore();

  useEffect(() => {
    loadNotebooks().then(() => {
      // Sync persisted activeNotebookId to backend on startup
      const nbId = useNotebookStore.getState().activeNotebookId;
      if (nbId) {
        const exists = useNotebookStore.getState().notebooks.some(n => n.id === nbId);
        if (exists) {
          // Re-notify backend (frontend already has it from localStorage)
          setActiveNotebook(nbId);
          // Auto-queue any missing summaries on startup
          regenerateMissingSummaries(nbId).catch(() => {});
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
      const chatStore = useChatStore.getState();
      if (payload.token) {
        chatStore.appendToken(payload.message_id, payload.token);
      }
      if (payload.done) {
        chatStore.finalizeMessage(payload.message_id);
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Listen for chat error events
  useEffect(() => {
    const unlisten = onChatError((payload) => {
      const chatStore = useChatStore.getState();
      chatStore.setStreamingError(payload.message_id, payload.error);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Listen for source status events + refresh stats
  useEffect(() => {
    const unlisten = onSourceStatus((payload) => {
      useSourceStore.getState().updateSourceStatus(payload.source_id, payload.status);
      // Refresh stats when a source finishes ingestion
      if (payload.status === "ready" || payload.status === "error") {
        const nbId = useNotebookStore.getState().activeNotebookId;
        if (nbId) useSourceStore.getState().loadStats(nbId);
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Listen for job completion events (e.g., summary generation)
  useEffect(() => {
    const unlisten = onJobCompleted((payload) => {
      if (!payload.output) return;
      try {
        const data = JSON.parse(payload.output) as { notebook_id?: string };
        const nbId = useNotebookStore.getState().activeNotebookId;
        if (data.notebook_id && data.notebook_id === nbId) {
          useSourceStore.getState().loadSources(nbId);
          useSourceStore.getState().loadStats(nbId);
        }
      } catch {
        // Ignore unparseable output
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  return (
    <div className="flex flex-col h-screen bg-bg">
      <div className="flex flex-1 overflow-hidden">
        <NotebookSidebar />
        {activeNotebookId ? (
          <PanelLayout notebookId={activeNotebookId} />
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
    </div>
  );
}
