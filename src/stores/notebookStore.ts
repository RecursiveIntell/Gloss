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
  activationRequestId: number;
  activationTargetId: string | null;
  activationError: string | null;
  loading: boolean;
  loadNotebooks: () => Promise<void>;
  createNotebook: (name: string) => Promise<string>;
  renameNotebook: (id: string, name: string) => Promise<void>;
  deleteNotebook: (id: string) => Promise<void>;
  setActive: (id: string | null) => Promise<void>;
}

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';
// Serialize real IPC effects, not just their UI responses. Every caller still
// receives its own operation result, including create/import activation.
let activationQueue: Promise<void> = Promise.resolve();

function readActiveNotebookId(): string | null {
  return typeof globalThis.localStorage === 'undefined'
    ? null
    : globalThis.localStorage.getItem(ACTIVE_NB_KEY);
}

export const useNotebookStore = create<NotebookStore>((set, get) => ({
  notebooks: [],
  activeNotebookId: readActiveNotebookId(),
  activationStatus: readActiveNotebookId() ? 'pending' : 'idle',
  activationRequestId: 0,
  activationTargetId: readActiveNotebookId(),
  activationError: null,
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

    const requestId = get().activationRequestId + 1;
    set({ activationStatus: 'pending', activationRequestId: requestId, activationTargetId: id, activationError: null });
    const operation = activationQueue.then(async () => {
      try {
        await api.setActiveNotebook(id);
        useChatStore.getState().resetForNotebookSwitch();
        useNoteStore.getState().resetForNotebookSwitch();
        useSourceStore.getState().resetForNotebookSwitch();
        // This cache remembers a preference only. Its failure cannot undo the
        // backend acknowledgment or leave the frontend pointing elsewhere.
        try {
          if (id) localStorage.setItem(ACTIVE_NB_KEY, id);
          else localStorage.removeItem(ACTIVE_NB_KEY);
        } catch (error) {
          console.warn('Could not remember the active notebook locally:', error);
        }
        const latest = get().activationRequestId === requestId;
        set({ activeNotebookId: id, activationStatus: latest ? (id ? 'confirmed' : 'idle') : 'pending' });
        if (id) await get().loadNotebooks();
      } catch (e) {
        console.warn('Notebook activation failed:', e);
        if (get().activationRequestId === requestId) {
          set({ activationStatus: 'error', activationError: e instanceof Error ? e.message : String(e) });
        }
        throw e;
      }
    });
    // An explicit later request may proceed after a failure. This continuation
    // never retries the failed operation and does not swallow its caller result.
    activationQueue = operation.then(() => undefined, () => undefined);
    return operation;
  },
}));

registerNotebookListRefresher(() => useNotebookStore.getState().loadNotebooks());
