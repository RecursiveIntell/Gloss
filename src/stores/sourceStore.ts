import { create } from 'zustand';
import type { Source, NotebookStats, SourceScope } from '../lib/types';
import * as api from '../lib/tauri';
import { useChatStore } from './chatStore';
import { useToastStore } from './toastStore';

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';
const SELECTION_PERSIST_DEBOUNCE_MS = 350;
let persistSelectedSourcesTimer: ReturnType<typeof setTimeout> | null = null;
let persistSelectedSourcesInFlight = false;
let persistSelectedSourcesPending: { notebookId: string; ids: string[] } | null = null;

async function refreshNotebookList() {
  const { useNotebookStore } = await import('./notebookStore');
  await useNotebookStore.getState().loadNotebooks();
}

function buildSourceScope(sources: Source[], selectedSourceIds: Set<string>): SourceScope {
  if (sources.length === 0) {
    return { kind: 'all' };
  }
  if (selectedSourceIds.size === 0) {
    return { kind: 'none' };
  }
  if (selectedSourceIds.size === sources.length) {
    return { kind: 'all' };
  }
  return { kind: 'explicit', ids: Array.from(selectedSourceIds) };
}

function clearSuggestedQuestions() {
  useChatStore.getState().clearSuggestedQuestions();
}

function persistSelectedSources(selectedSourceIds: Set<string>) {
  const notebookId = localStorage.getItem(ACTIVE_NB_KEY);
  if (!notebookId) return;
  persistSelectedSourcesPending = { notebookId, ids: Array.from(selectedSourceIds) };
  if (persistSelectedSourcesTimer) {
    clearTimeout(persistSelectedSourcesTimer);
  }
  persistSelectedSourcesTimer = setTimeout(() => {
    void flushSelectedSources();
  }, SELECTION_PERSIST_DEBOUNCE_MS);
}

async function flushSelectedSources(): Promise<void> {
  if (persistSelectedSourcesInFlight || !persistSelectedSourcesPending) return;
  persistSelectedSourcesInFlight = true;
  const snapshot = persistSelectedSourcesPending;
  persistSelectedSourcesPending = null;
  try {
    await api.setSelectedSources(snapshot.notebookId, snapshot.ids);
  } catch (e) {
    console.error('Failed to persist selected sources:', e);
  } finally {
    persistSelectedSourcesInFlight = false;
    if (persistSelectedSourcesPending) {
      void flushSelectedSources();
    }
  }
}

interface SourceStore {
  sources: Source[];
  selectedSourceIds: Set<string>;
  loading: boolean;
  stats: NotebookStats | null;
  loadSources: (notebookId: string) => Promise<void>;
  addSourceFile: (notebookId: string, path: string) => Promise<void>;
  addSourceFiles: (notebookId: string, paths: string[]) => Promise<void>;
  addSourceFolder: (notebookId: string, path: string) => Promise<void>;
  addSourcePaste: (notebookId: string, title: string, text: string) => Promise<void>;
  deleteSource: (notebookId: string, sourceId: string) => Promise<void>;
  retrySource: (notebookId: string, sourceId: string) => Promise<void>;
  reindexSource: (notebookId: string, sourceId: string) => Promise<void>;
  reindexNotebook: (notebookId: string) => Promise<void>;
  bulkDeleteSelected: (notebookId: string) => Promise<void>;
  toggleSource: (sourceId: string) => void;
  toggleGroup: (group: string) => void;
  selectAll: () => void;
  selectNone: () => void;
  getSourceScope: () => SourceScope;
  updateSourceStatus: (sourceId: string, status: string) => void;
  updateSourceStatusBulk: (updates: Array<{ sourceId: string; status: string; errorMessage?: string }>) => void;
  loadStats: (notebookId: string) => Promise<void>;
  resetForNotebookSwitch: () => void;
}

export const useSourceStore = create<SourceStore>((set, get) => ({
  sources: [],
  selectedSourceIds: new Set<string>(),
  loading: false,
  stats: null,

  loadSources: async (notebookId) => {
    set({ loading: true });
    try {
      const sources = await api.listSources(notebookId);
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      const selectedIds = new Set(sources.filter(s => s.selected).map(s => s.id));
      set({ sources, selectedSourceIds: selectedIds, loading: false });
    } catch (e) {
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      console.error('Failed to load sources:', e);
      set({ loading: false });
    }
  },

  addSourceFile: async (notebookId, path) => {
    try {
      clearSuggestedQuestions();
      await api.addSourceFile(notebookId, path);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Import Failed',
        message: String(e),
        duration: 5000,
      });
    }
  },

  addSourceFiles: async (notebookId, paths) => {
    if (paths.length === 0) return;
    try {
      clearSuggestedQuestions();
      await api.addSourceFiles(notebookId, paths);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'Import Started',
        message: `${paths.length} files queued for ingestion.`,
        duration: 3000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Import Failed',
        message: String(e),
        duration: 5000,
      });
    }
  },

  addSourceFolder: async (notebookId, path) => {
    try {
      // Schedules the directory walk and ingestion in the background.
      clearSuggestedQuestions();
      await api.addSourceFolder(notebookId, path);
      useToastStore.getState().addToast({
        type: 'info',
        title: 'Folder Import Started',
        message: 'Scanning and ingesting sources in the background.',
        duration: 3000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Folder Import Failed',
        message: String(e),
        duration: 5000,
      });
    }
  },

  addSourcePaste: async (notebookId, title, text) => {
    try {
      clearSuggestedQuestions();
      await api.addSourcePaste(notebookId, title, text);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Paste Failed',
        message: String(e),
        duration: 5000,
      });
    }
  },

  deleteSource: async (notebookId, sourceId) => {
    try {
      clearSuggestedQuestions();
      await api.deleteSource(notebookId, sourceId);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Delete Failed',
        message: String(e),
        duration: 5000,
      });
    }
  },

  retrySource: async (notebookId, sourceId) => {
    try {
      clearSuggestedQuestions();
      await api.retrySourceIngestion(notebookId, sourceId);
      await get().loadSources(notebookId);
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Retry Failed',
        message: String(e),
        duration: 5000,
      });
    }
  },

  reindexSource: async (notebookId, sourceId) => {
    try {
      await api.semanticMemoryReindexSource(notebookId, sourceId);
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'Reindex Complete',
        message: 'Source projected to semantic-memory preview.',
        duration: 3000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Reindex Failed',
        message: String(e),
        duration: 6000,
      });
    }
  },

  reindexNotebook: async (notebookId) => {
    try {
      const receipts = await api.semanticMemoryReindexNotebook(notebookId);
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'Notebook Reindexed',
        message: `${receipts.length} sources processed for semantic-memory preview.`,
        duration: 4000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Notebook Reindex Failed',
        message: String(e),
        duration: 7000,
      });
    }
  },

  bulkDeleteSelected: async (notebookId) => {
    const ids = Array.from(get().selectedSourceIds);
    if (ids.length === 0) return;
    try {
      clearSuggestedQuestions();
      await api.deleteSources(notebookId, ids);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Bulk Delete Failed',
        message: String(e),
        duration: 6000,
      });
    }
  },

  toggleSource: (sourceId) => {
    set((state) => {
      const next = new Set(state.selectedSourceIds);
      if (next.has(sourceId)) next.delete(sourceId);
      else next.add(sourceId);
      persistSelectedSources(next);
      clearSuggestedQuestions();
      return { selectedSourceIds: next };
    });
  },

  toggleGroup: (group) => {
    set((state) => {
      const groupSources = state.sources.filter(s => {
        const parts = s.title.split('/');
        return parts.length > 1 ? parts[0] === group : group === '(ungrouped)';
      });
      const allSelected = groupSources.every(s => state.selectedSourceIds.has(s.id));
      const next = new Set(state.selectedSourceIds);
      for (const s of groupSources) {
        if (allSelected) next.delete(s.id);
        else next.add(s.id);
      }
      persistSelectedSources(next);
      clearSuggestedQuestions();
      return { selectedSourceIds: next };
    });
  },

  selectAll: () => {
    set((state) => {
      const next = new Set(state.sources.map(s => s.id));
      persistSelectedSources(next);
      clearSuggestedQuestions();
      return { selectedSourceIds: next };
    });
  },

  selectNone: () => {
    const next = new Set<string>();
    persistSelectedSources(next);
    clearSuggestedQuestions();
    set({ selectedSourceIds: next });
  },

  getSourceScope: () => {
    const { sources, selectedSourceIds } = get();
    return buildSourceScope(sources, selectedSourceIds);
  },

  updateSourceStatus: (sourceId, status) => {
    set((state) => ({
      sources: state.sources.map(s =>
        s.id === sourceId ? { ...s, status } : s
      ),
    }));
  },

  updateSourceStatusBulk: (updates) => {
    if (updates.length === 0) return;
    const map = new Map(updates.map(u => [u.sourceId, u]));
    set((state) => ({
      sources: state.sources.map((s) => {
        const u = map.get(s.id);
        if (!u) return s;
        return { ...s, status: u.status, error_message: u.errorMessage ?? s.error_message };
      }),
    }));
  },

  loadStats: async (notebookId) => {
    try {
      const stats = await api.getNotebookStats(notebookId);
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      set({ stats });
    } catch {
      // Stats are optional — don't crash on failure
    }
  },

  resetForNotebookSwitch: () => {
    set({
      sources: [],
      selectedSourceIds: new Set<string>(),
      stats: null,
      loading: false,
    });
  },
}));
