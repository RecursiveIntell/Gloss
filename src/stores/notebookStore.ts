import { create } from 'zustand';
import type { Notebook } from '../lib/types';
import * as api from '../lib/tauri';
import { useChatStore } from './chatStore';
import { useNoteStore } from './noteStore';
import { useSourceStore } from './sourceStore';
import { registerNotebookListRefresher } from './notebookRefresh';

interface NotebookStore {
  notebooks: Notebook[];
  activeNotebookId: string | null;
  activationStatus: 'idle' | 'pending' | 'confirmed' | 'error';
  loading: boolean;
  loadNotebooks: () => Promise<void>;
  createNotebook: (name: string) => Promise<string>;
  renameNotebook: (id: string, name: string) => Promise<void>;
  deleteNotebook: (id: string) => Promise<void>;
  setActive: (id: string | null) => Promise<void>;
}

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';

export const useNotebookStore = create<NotebookStore>((set, get) => ({
  notebooks: [],
  activeNotebookId: localStorage.getItem(ACTIVE_NB_KEY),
  activationStatus: localStorage.getItem(ACTIVE_NB_KEY) ? 'pending' : 'idle',
  loading: false,

  loadNotebooks: async () => {
    set({ loading: true });
    try {
      const notebooks = await api.listNotebooks();
      set({ notebooks, loading: false });
    } catch (e) {
      console.warn('Failed to load notebooks:', e);
      set({ loading: false });
    }
  },

  createNotebook: async (name) => {
    try {
      const id = await api.createNotebook(name);
      await get().loadNotebooks();
      await get().setActive(id);
      return id;
    } catch (e) {
      console.warn('Failed to create notebook:', e);
      throw e;
    }
  },

  renameNotebook: async (id, name) => {
    await api.renameNotebook(id, name);
    await get().loadNotebooks();
  },

  deleteNotebook: async (id) => {
    const { activeNotebookId } = get();
    // Clear active notebook BEFORE deletion so the backend stops summary jobs
    // and the UI resets immediately (prevents race with summary loop)
    if (activeNotebookId === id) {
      await get().setActive(null);
    }
    await api.deleteNotebook(id);
    await get().loadNotebooks();
  },

  setActive: async (id) => {
    if (get().activeNotebookId === id && get().activationStatus === 'confirmed') {
      return;
    }

    set({ activationStatus: 'pending' });
    if (id) {
      const targetId = id;
      try {
        await api.setActiveNotebook(targetId);
        useChatStore.getState().resetForNotebookSwitch();
        useNoteStore.getState().resetForNotebookSwitch();
        useSourceStore.getState().resetForNotebookSwitch();
        localStorage.setItem(ACTIVE_NB_KEY, targetId);
        set({ activeNotebookId: targetId, activationStatus: 'confirmed' });
        await get().loadNotebooks();
      } catch (e) {
        console.warn('Notebook activation failed:', e);
        set({ activationStatus: 'error' });
        throw e;
      }
    } else {
      try {
        await api.setActiveNotebook(null);
        useChatStore.getState().resetForNotebookSwitch();
        useNoteStore.getState().resetForNotebookSwitch();
        useSourceStore.getState().resetForNotebookSwitch();
        localStorage.removeItem(ACTIVE_NB_KEY);
        set({ activeNotebookId: null, activationStatus: 'idle' });
      } catch (e) {
        console.warn('Failed to clear active notebook:', e);
        set({ activationStatus: 'error' });
        throw e;
      }
    }
  },
}));

registerNotebookListRefresher(() => useNotebookStore.getState().loadNotebooks());
