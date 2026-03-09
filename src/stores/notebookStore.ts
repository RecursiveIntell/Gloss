import { create } from 'zustand';
import type { Notebook } from '../lib/types';
import * as api from '../lib/tauri';
import { useChatStore } from './chatStore';
import { useNoteStore } from './noteStore';
import { useSourceStore } from './sourceStore';

interface NotebookStore {
  notebooks: Notebook[];
  activeNotebookId: string | null;
  loading: boolean;
  loadNotebooks: () => Promise<void>;
  createNotebook: (name: string) => Promise<string>;
  deleteNotebook: (id: string) => Promise<void>;
  setActive: (id: string | null) => void;
}

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';

export const useNotebookStore = create<NotebookStore>((set, get) => ({
  notebooks: [],
  activeNotebookId: localStorage.getItem(ACTIVE_NB_KEY),
  loading: false,

  loadNotebooks: async () => {
    set({ loading: true });
    try {
      const notebooks = await api.listNotebooks();
      set({ notebooks, loading: false });
    } catch (e) {
      console.error('Failed to load notebooks:', e);
      set({ loading: false });
    }
  },

  createNotebook: async (name) => {
    try {
      const id = await api.createNotebook(name);
      await get().loadNotebooks();
      get().setActive(id);
      return id;
    } catch (e) {
      console.error('Failed to create notebook:', e);
      throw e;
    }
  },

  deleteNotebook: async (id) => {
    const { activeNotebookId } = get();
    // Clear active notebook BEFORE deletion so the backend stops summary jobs
    // and the UI resets immediately (prevents race with summary loop)
    if (activeNotebookId === id) {
      get().setActive(null);
    }
    await api.deleteNotebook(id);
    await get().loadNotebooks();
  },

  setActive: (id) => {
    // Reset notebook-scoped frontend state before switching
    useChatStore.getState().resetForNotebookSwitch();
    useNoteStore.getState().resetForNotebookSwitch();
    useSourceStore.getState().resetForNotebookSwitch();
    set({ activeNotebookId: id });
    // Persist across restarts
    if (id) {
      localStorage.setItem(ACTIVE_NB_KEY, id);
    } else {
      localStorage.removeItem(ACTIVE_NB_KEY);
    }
    // Await backend initialization before queuing summaries
    if (id) {
      const targetId = id;
      api.setActiveNotebook(targetId)
        .then(() => {
          if (get().activeNotebookId !== targetId) return;
          return api.regenerateMissingSummaries(targetId);
        })
        .catch((e) => console.error('Notebook activation failed:', e));
    } else {
      api.setActiveNotebook(null).catch((e) => {
        console.error('Failed to clear active notebook:', e);
      });
    }
  },
}));
