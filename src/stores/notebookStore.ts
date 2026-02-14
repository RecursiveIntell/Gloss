import { create } from 'zustand';
import type { Notebook } from '../lib/types';
import * as api from '../lib/tauri';

interface NotebookStore {
  notebooks: Notebook[];
  activeNotebookId: string | null;
  loading: boolean;
  loadNotebooks: () => Promise<void>;
  createNotebook: (name: string) => Promise<string>;
  deleteNotebook: (id: string) => Promise<void>;
  setActive: (id: string) => void;
}

export const useNotebookStore = create<NotebookStore>((set, get) => ({
  notebooks: [],
  activeNotebookId: null,
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
      set({ activeNotebookId: id });
      return id;
    } catch (e) {
      console.error('Failed to create notebook:', e);
      throw e;
    }
  },

  deleteNotebook: async (id) => {
    await api.deleteNotebook(id);
    const { activeNotebookId } = get();
    if (activeNotebookId === id) {
      set({ activeNotebookId: null });
    }
    await get().loadNotebooks();
  },

  setActive: (id) => set({ activeNotebookId: id }),
}));
