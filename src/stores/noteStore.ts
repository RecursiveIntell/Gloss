import { create } from 'zustand';
import type { Note } from '../lib/types';
import * as api from '../lib/tauri';

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';

interface NoteStore {
  notes: Note[];
  loading: boolean;
  loadNotes: (notebookId: string) => Promise<void>;
  createNote: (notebookId: string, title: string, content: string) => Promise<void>;
  saveResponse: (notebookId: string, messageId: string) => Promise<void>;
  updateNote: (notebookId: string, noteId: string, title?: string, content?: string) => Promise<void>;
  togglePin: (notebookId: string, noteId: string) => Promise<void>;
  deleteNote: (notebookId: string, noteId: string) => Promise<void>;
  resetForNotebookSwitch: () => void;
}

export const useNoteStore = create<NoteStore>((set, get) => ({
  notes: [],
  loading: false,

  loadNotes: async (notebookId) => {
    set({ loading: true });
    try {
      const notes = await api.listNotes(notebookId);
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      set({ notes, loading: false });
    } catch (e) {
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      console.error('Failed to load notes:', e);
      set({ loading: false });
    }
  },

  createNote: async (notebookId, title, content) => {
    await api.createNote(notebookId, title, content);
    await get().loadNotes(notebookId);
  },

  saveResponse: async (notebookId, messageId) => {
    await api.saveResponseAsNote(notebookId, messageId);
    await get().loadNotes(notebookId);
  },

  updateNote: async (notebookId, noteId, title, content) => {
    await api.updateNote(notebookId, noteId, title, content);
    await get().loadNotes(notebookId);
  },

  togglePin: async (notebookId, noteId) => {
    await api.togglePin(notebookId, noteId);
    await get().loadNotes(notebookId);
  },

  deleteNote: async (notebookId, noteId) => {
    await api.deleteNote(notebookId, noteId);
    await get().loadNotes(notebookId);
  },

  resetForNotebookSwitch: () => {
    set({
      notes: [],
      loading: false,
    });
  },
}));
