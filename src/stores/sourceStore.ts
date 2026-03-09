import { create } from 'zustand';
import type { Source, NotebookStats } from '../lib/types';
import * as api from '../lib/tauri';
import { useToastStore } from './toastStore';

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';

interface SourceStore {
  sources: Source[];
  selectedSourceIds: Set<string>;
  loading: boolean;
  stats: NotebookStats | null;
  loadSources: (notebookId: string) => Promise<void>;
  addSourceFile: (notebookId: string, path: string) => Promise<void>;
  addSourceFolder: (notebookId: string, path: string) => Promise<void>;
  addSourcePaste: (notebookId: string, title: string, text: string) => Promise<void>;
  deleteSource: (notebookId: string, sourceId: string) => Promise<void>;
  retrySource: (notebookId: string, sourceId: string) => Promise<void>;
  toggleSource: (sourceId: string) => void;
  toggleGroup: (group: string) => void;
  selectAll: () => void;
  selectNone: () => void;
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
      await api.addSourceFile(notebookId, path);
      await get().loadSources(notebookId);
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
      await api.addSourcePaste(notebookId, title, text);
      await get().loadSources(notebookId);
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
      await api.deleteSource(notebookId, sourceId);
      await get().loadSources(notebookId);
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

  toggleSource: (sourceId) => {
    set((state) => {
      const next = new Set(state.selectedSourceIds);
      if (next.has(sourceId)) next.delete(sourceId);
      else next.add(sourceId);
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
      return { selectedSourceIds: next };
    });
  },

  selectAll: () => {
    set((state) => ({
      selectedSourceIds: new Set(state.sources.map(s => s.id)),
    }));
  },

  selectNone: () => set({ selectedSourceIds: new Set() }),

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
