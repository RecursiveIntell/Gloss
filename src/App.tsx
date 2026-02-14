import { useEffect } from "react";
import { NotebookSidebar } from "./components/notebooks/NotebookSidebar";
import { PanelLayout } from "./components/layout/PanelLayout";
import { StatusBar } from "./components/layout/StatusBar";
import { useNotebookStore } from "./stores/notebookStore";
import { useSettingsStore } from "./stores/settingsStore";
import { useChatStore } from "./stores/chatStore";
import { onChatToken, onSourceStatus } from "./lib/events";
import { useSourceStore } from "./stores/sourceStore";

export function App() {
  const { activeNotebookId, loadNotebooks } = useNotebookStore();
  const { loadSettings, loadProviders, loadModels } = useSettingsStore();

  useEffect(() => {
    loadNotebooks();
    loadSettings();
    loadProviders();
    loadModels();
  }, []);

  // Listen for chat token events
  useEffect(() => {
    const unlisten = onChatToken((payload) => {
      const chatStore = useChatStore.getState();
      if (payload.done) {
        chatStore.finalizeMessage(payload.message_id, "");
      } else {
        chatStore.appendToken(payload.message_id, payload.token);
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Listen for source status events
  useEffect(() => {
    const unlisten = onSourceStatus((payload) => {
      useSourceStore.getState().updateSourceStatus(payload.source_id, payload.status);
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
