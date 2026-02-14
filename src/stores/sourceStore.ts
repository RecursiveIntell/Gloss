import { create } from 'zustand';
import type { Source } from '../lib/types';
import * as api from '../lib/tauri';

interface SourceStore {
  sources: Source[];
  selectedSourceIds: Set<string>;
  loading: boolean;
  loadSources: (notebookId: string) => Promise<void>;
  addSourceFile: (notebookId: string, path: string) => Promise<void>;
  addSourceFolder: (notebookId: string, path: string) => Promise<void>;
  addSourcePaste: (notebookId: string, title: string, text: string) => Promise<void>;
  deleteSource: (notebookId: string, sourceId: string) => Promise<void>;
  toggleSource: (sourceId: string) => void;
  selectAll: () => void;
  selectNone: () => void;
  updateSourceStatus: (sourceId: string, status: string) => void;
}

export const useSourceStore = create<SourceStore>((set, get) => ({
  sources: [],
  selectedSourceIds: new Set<string>(),
  loading: false,

  loadSources: async (notebookId) => {
    set({ loading: true });
    try {
      const sources = await api.listSources(notebookId);
      const selectedIds = new Set(sources.filter(s => s.selected).map(s => s.id));
      set({ sources, selectedSourceIds: selectedIds, loading: false });
    } catch (e) {
      console.error('Failed to load sources:', e);
      set({ loading: false });
    }
  },

  addSourceFile: async (notebookId, path) => {
    await api.addSourceFile(notebookId, path);
    await get().loadSources(notebookId);
  },

  addSourceFolder: async (notebookId, path) => {
    await api.addSourceFolder(notebookId, path);
    await get().loadSources(notebookId);
  },

  addSourcePaste: async (notebookId, title, text) => {
    await api.addSourcePaste(notebookId, title, text);
    await get().loadSources(notebookId);
  },

  deleteSource: async (notebookId, sourceId) => {
    await api.deleteSource(notebookId, sourceId);
    await get().loadSources(notebookId);
  },

  toggleSource: (sourceId) => {
    set((state) => {
      const next = new Set(state.selectedSourceIds);
      if (next.has(sourceId)) next.delete(sourceId);
      else next.add(sourceId);
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
}));
