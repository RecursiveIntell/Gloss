import { create } from 'zustand';
import type { Note } from '../lib/types';
import * as api from '../lib/tauri';
import { useToastStore } from './toastStore';
import { useNotebookStore } from './notebookStore';


function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  return 'Unknown error';
}

interface NoteStore {
  notes: Note[];
  loading: boolean;
  loadError: string | null;
  loadEpoch: number;
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
  loadError: null,
  loadEpoch: 0,

  loadNotes: async (notebookId) => {
    if (useNotebookStore.getState().activeNotebookId !== notebookId) return;
    const loadEpoch = get().loadEpoch + 1;
    set({ loading: true, loadError: null, loadEpoch });
    try {
      const notes = await api.listNotes(notebookId);
      if (useNotebookStore.getState().activeNotebookId !== notebookId || get().loadEpoch !== loadEpoch) {
        return;
      }
      set({ notes, loading: false });
    } catch (e) {
      if (useNotebookStore.getState().activeNotebookId !== notebookId || get().loadEpoch !== loadEpoch) {
        return;
      }
      console.warn('Failed to load notes:', e);
      set({ loading: false, loadError: errMsg(e) });
    }
  },

  createNote: async (notebookId, title, content) => {
    try {
      await api.createNote(notebookId, title, content);
      await get().loadNotes(notebookId);
    } catch (e) {
      console.warn('Failed to create note:', e);
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Create note failed',
        message: errMsg(e),
        duration: 0,
      });
      throw e;
    }
  },

  saveResponse: async (notebookId, messageId) => {
    try {
      await api.saveResponseAsNote(notebookId, messageId);
      await get().loadNotes(notebookId);
    } catch (e) {
      console.warn('Failed to save response as note:', e);
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Save as note failed',
        message: errMsg(e),
        duration: 0,
      });
      throw e;
    }
  },

  updateNote: async (notebookId, noteId, title, content) => {
    try {
      await api.updateNote(notebookId, noteId, title, content);
      await get().loadNotes(notebookId);
    } catch (e) {
      console.warn('Failed to update note:', e);
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Update note failed',
        message: errMsg(e),
        duration: 0,
      });
      throw e;
    }
  },

  togglePin: async (notebookId, noteId) => {
    try {
      await api.togglePin(notebookId, noteId);
      await get().loadNotes(notebookId);
    } catch (e) {
      console.warn('Failed to toggle pin:', e);
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Pin toggle failed',
        message: errMsg(e),
        duration: 0,
      });
      throw e;
    }
  },

  deleteNote: async (notebookId, noteId) => {
    try {
      await api.deleteNote(notebookId, noteId);
      await get().loadNotes(notebookId);
    } catch (e) {
      console.warn('Failed to delete note:', e);
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Delete note failed',
        message: errMsg(e),
        duration: 0,
      });
      throw e;
    }
  },

  resetForNotebookSwitch: () => {
    set({
      notes: [],
      loading: false,
      loadError: null,
      loadEpoch: get().loadEpoch + 1,
    });
  },
}));
