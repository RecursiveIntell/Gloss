import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { NotebookSidebar } from "./components/notebooks/NotebookSidebar";
import { PanelLayout } from "./components/layout/PanelLayout";
import { StatusBar } from "./components/layout/StatusBar";
import { ToastContainer } from "./components/layout/ToastContainer";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { CommandPalette } from "./components/CommandPalette";
import { EmptyStateOnboarding } from "./components/EmptyStateOnboarding";
import { SettingsDialog } from "./components/settings/SettingsDialog";
import { useNotebookStore } from "./stores/notebookStore";
import { useSettingsStore } from "./stores/settingsStore";
import { useChatStore } from "./stores/chatStore";
import { useToastStore } from "./stores/toastStore";
import { useUiStore } from "./stores/uiStore";
import { useSourceStore } from "./stores/sourceStore";
import * as api from "./lib/tauri";
import { onChatToken, onChatStatus, onChatError, onChatCancelled, onChatEvidence, onSourceStatus, onSourcesBatchCreated, onBatchIngestionComplete, onJobCompleted } from "./lib/events";
import { BookOpen, Database, Search, Sparkles } from "lucide-react";

const SAMPLE_NOTEBOOK_SOURCES = [
  {
    title: "Welcome to Gloss",
    text: "Gloss is a local-first notebook for chat, sources, and notes. Start by asking questions about your files and watching references stay grounded in imported sources.",
  },
  {
    title: "Get started with sources",
    text: "Drop notes, PDFs, markdown, or URLs in Sources. Then switch source scope in chat (All / Selected) to compare context-heavy and focused answers.",
  },
  {
    title: "Try notes and studio",
    text: "Use Notes to pin useful answers and Studio to generate structured outputs. This project keeps everything tied to your current notebook by default.",
  },
] as const;

function isNotebookStoreAvailable(target: EventTarget | null): target is HTMLElement {
  return target instanceof HTMLElement;
}

function isHotkeyAllowed(event: KeyboardEvent): boolean {
  if (!isNotebookStoreAvailable(event.target)) return false;
  const editableTag = new Set(["INPUT", "TEXTAREA", "SELECT"]).has(event.target.tagName);
  if (editableTag || event.target.isContentEditable) return false;
  if (event.defaultPrevented) return false;
  if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) {
    // continue
  }
  return true;
}

function isMacLikePlatform(): boolean {
  return /Mac|iPad|iPhone|iPod/.test(navigator.platform);
}

export function App() {
  const { notebooks, activeNotebookId, loadNotebooks, setActive, createNotebook } = useNotebookStore();
  const { createConversation } = useChatStore();
  const { activeModel, models, settings, loadSettings, loadProviders, loadModels, loadFeatureFlags } = useSettingsStore();
  const stats = useSourceStore((s) => s.stats);
  const addToast = useToastStore((s) => s.addToast);
  const { commandPaletteOpen, setCommandPaletteOpen, toggleCommandPaletteOpen, toggleTheme } = useUiStore();
  const [showSettings, setShowSettings] = useState(false);
  const activeNotebook = notebooks.find((notebook) => notebook.id === activeNotebookId) ?? null;
  const activeProvider =
    settings["default_provider"] ||
    models.find((model) => model.id === activeModel)?.provider_id ||
    null;

  const activeTheme = useUiStore((state) => state.theme);

  const pendingStatusRef = useRef<Map<string, { status: string; errorMessage?: string }>>(new Map());
  const statusFlushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const statsDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const batchReadyCountRef = useRef(0);
  const batchToastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const batchCreatedDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const jobCompletedDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (typeof document !== "undefined") {
      document.documentElement.setAttribute("data-gloss-theme", activeTheme);
    }
  }, [activeTheme]);

  useEffect(() => {
    loadNotebooks().then(() => {
      // Sync persisted activeNotebookId to backend on startup
      const nbId = useNotebookStore.getState().activeNotebookId;
      if (nbId) {
        const exists = useNotebookStore.getState().notebooks.some((n) => n.id === nbId);
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
    loadFeatureFlags();
  }, []);

  useEffect(() => {
    const isMac = isMacLikePlatform();
    const handleGlobalKeyDown = (event: KeyboardEvent) => {
      if (!isHotkeyAllowed(event)) return;
      const mod = (isMac && event.metaKey) || (!isMac && event.ctrlKey);
      if (!mod) return;
      const key = event.key.toLowerCase();
      // Cmd/Ctrl+K — command palette
      if (key === "k" && !event.shiftKey) {
        event.preventDefault();
        toggleCommandPaletteOpen();
        return;
      }
      // D7 — Cmd/Ctrl+N or Cmd/Ctrl+T — new chat conversation
      if ((key === "n" || key === "t") && !event.shiftKey) {
        event.preventDefault();
        const notebookId = useNotebookStore.getState().activeNotebookId;
        if (notebookId) {
          useChatStore.getState().createConversation(notebookId).catch((e) => {
            // surfaced via toast
            useToastStore.getState().addToast({
              type: "error",
              title: "New chat failed",
              message: `Failed to create conversation: ${String(e)}`,
              duration: 6000,
            });
          });
        }
        return;
      }
      // D7 — Cmd/Ctrl+, — open settings dialog
      if (key === ",") {
        event.preventDefault();
        setShowSettings((s) => !s);
        return;
      }
      // D7 — Cmd/Ctrl+Shift+T — toggle theme
      if (key === "t" && event.shiftKey) {
        event.preventDefault();
        useUiStore.getState().toggleTheme();
        return;
      }
    };

    window.addEventListener("keydown", handleGlobalKeyDown);
    return () => window.removeEventListener("keydown", handleGlobalKeyDown);
  }, [toggleCommandPaletteOpen]);

  // Listen for all Tauri events — consolidated cleanup
  useEffect(() => {
    const unlisteners: Promise<VoidFunction>[] = [];
    const replayThenRehydrate = (notebookId: string, conversationId: string) => {
      const chatStore = useChatStore.getState();
      void chatStore
        .replayChatEvents(notebookId, conversationId)
        .catch((error) => {
          console.warn("Failed to replay chat events:", error);
        })
        .finally(() => {
          void useChatStore.getState().rehydrateConversation(notebookId, conversationId);
        });
    };

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
        void chatStore.finalizeMessage(
          payload.notebook_id,
          payload.conversation_id,
          payload.message_id
        ).finally(() => replayThenRehydrate(payload.notebook_id, payload.conversation_id));
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
      replayThenRehydrate(payload.notebook_id, payload.conversation_id);
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
      replayThenRehydrate(payload.notebook_id, payload.conversation_id);
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

  useEffect(() => {
    const rehydrateActiveConversation = () => {
      const notebookId = useNotebookStore.getState().activeNotebookId;
      const conversationId = useChatStore.getState().activeConversationId;
      if (!notebookId || !conversationId) return;
      void useChatStore.getState()
        .replayChatEvents(notebookId, conversationId)
        .catch((error) => {
          console.warn("Failed to replay chat events on focus:", error);
        })
        .finally(() => {
          void useChatStore.getState().rehydrateConversation(notebookId, conversationId);
        });
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState === "visible") {
        rehydrateActiveConversation();
      }
    };
    window.addEventListener("focus", rehydrateActiveConversation);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      window.removeEventListener("focus", rehydrateActiveConversation);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, []);

  const handleCreateEmptyNotebook = async () => {
    try {
      await createNotebook("New Notebook");
    } catch (error) {
      addToast({
        type: "error",
        title: "Failed to create notebook",
        message: error instanceof Error ? error.message : String(error),
        duration: 6000,
      });
    }
  };

  const handleCreateSampleNotebook = async () => {
    try {
      const notebookId = await createNotebook("Sample Notebook");
      await Promise.all(
        SAMPLE_NOTEBOOK_SOURCES.map((snippet) =>
          api.addSourcePaste(notebookId, snippet.title, snippet.text)
        )
      );
      await useSourceStore.getState().loadSources(notebookId);
      await useSourceStore.getState().loadStats(notebookId);
      addToast({
        type: "success",
        title: "Sample notebook ready",
        message: "Created Sample Notebook with starter sources.",
        duration: 5000,
      });
    } catch (error) {
      addToast({
        type: "error",
        title: "Sample notebook creation failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 6000,
      });
    }
  };

  const handleImportSource = async () => {
    if (!activeNotebookId) {
      addToast({
        type: "warning",
        title: "Select a notebook first",
        message: "Create or open a notebook before importing files.",
        duration: 5000,
      });
      return;
    }

    const selected = await open({
      title: "Import source files",
      multiple: true,
      directory: false,
    });

    if (!selected) return;

    const files = Array.isArray(selected) ? selected : [selected];
    try {
      await api.addSourceFiles(activeNotebookId, files);
      await useSourceStore.getState().loadSources(activeNotebookId);
      await useSourceStore.getState().loadStats(activeNotebookId);
      addToast({
        type: "success",
        title: "Files queued for import",
        message: `Added ${files.length} file${files.length === 1 ? "" : "s"} to Sources.`,
        duration: 5000,
      });
    } catch (error) {
      addToast({
        type: "error",
        title: "Import failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 6000,
      });
    }
  };

  const handleOpenSettings = () => {
    setCommandPaletteOpen(false);
    setShowSettings(true);
  };

  const handleNewChat = async () => {
    if (!activeNotebookId) {
      addToast({
        type: "warning",
        title: "Select a notebook first",
        message: "Create or open a notebook to start chatting.",
        duration: 5000,
      });
      return;
    }

    try {
      await createConversation(activeNotebookId);
    } catch (error) {
      addToast({
        type: "error",
        title: "Failed to start chat",
        message: error instanceof Error ? error.message : String(error),
        duration: 6000,
      });
    }
  };

  const handleViewSources = () => {
    const sourceButton =
      document.querySelector<HTMLButtonElement>(
        'button[title="Open sources"], button[title="Close sources"]'
      );
    if (!sourceButton) {
      addToast({
        type: "info",
        title: "Sources panel unavailable",
        message: "Open a notebook to access Sources.",
        duration: 4000,
      });
      return;
    }
    if (sourceButton.title === "Open sources") {
      sourceButton.click();
    }
  };

  const handleViewNotes = () => {
    const notesButton =
      document.querySelector<HTMLButtonElement>(
        'button[title="Open notes"], button[title="Close notes"]'
      );
    if (!notesButton) {
      addToast({
        type: "info",
        title: "Notes panel unavailable",
        message: "Open a notebook to access Notes.",
        duration: 4000,
      });
      return;
    }
    if (notesButton.title === "Open notes") {
      notesButton.click();
    }
  };

  const handleViewStudio = () => {
    handleViewNotes();
    window.setTimeout(() => {
      const studioTab = Array.from(document.querySelectorAll("button")).find(
        (button) => button.textContent?.trim() === "Studio"
      );
      if (studioTab) {
        studioTab.click();
      }
    }, 0);
  };

  return (
    <ErrorBoundary>
    <div className="gloss-root flex h-screen flex-col bg-bg">
      <GlossTopBar
        activeNotebookName={activeNotebook?.name ?? null}
        activeModel={activeModel}
        activeProvider={activeProvider}
        sourceCount={stats?.source_count ?? activeNotebook?.source_count ?? 0}
        chunkCount={stats?.chunk_count ?? 0}
        onOpenCommandPalette={() => setCommandPaletteOpen(true)}
      />
      <div className="flex flex-1 overflow-hidden">
        <NotebookSidebar />
        {activeNotebookId ? (
          <PanelLayout key={activeNotebookId} notebookId={activeNotebookId} />
        ) : (
          <EmptyStateOnboarding
            onCreateEmptyNotebook={handleCreateEmptyNotebook}
            onTrySampleNotebook={handleCreateSampleNotebook}
            onImportFiles={handleImportSource}
          />
        )}
      </div>
      <StatusBar />
      <ToastContainer />
      <CommandPalette
        open={commandPaletteOpen}
        onClose={() => setCommandPaletteOpen(false)}
        notebooks={notebooks}
        activeNotebookId={activeNotebookId}
        onNewChat={handleNewChat}
        onNewNotebook={handleCreateEmptyNotebook}
        onSwitchNotebook={setActive}
        onOpenSettings={handleOpenSettings}
        onToggleTheme={toggleTheme}
        onImportSource={handleImportSource}
        onViewSources={handleViewSources}
        onViewNotes={handleViewNotes}
        onViewStudio={handleViewStudio}
      />
      <SettingsDialog open={showSettings} onClose={() => setShowSettings(false)} />
    </div>
    </ErrorBoundary>
  );
}

function GlossTopBar({
  activeNotebookName,
  activeModel,
  activeProvider,
  sourceCount,
  chunkCount,
  onOpenCommandPalette,
}: {
  activeNotebookName: string | null;
  activeModel: string;
  activeProvider: string | null;
  sourceCount: number;
  chunkCount: number;
  onOpenCommandPalette: () => void;
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
        <button
          type="button"
          onClick={onOpenCommandPalette}
          className="gloss-mono ml-auto rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted hover:border-accent/50 hover:text-text"
          title="Open command palette"
        >
          Cmd K
        </button>
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
