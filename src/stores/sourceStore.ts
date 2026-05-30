import { create } from 'zustand';
import type { Source, NotebookStats, SourceScope } from '../lib/types';
import * as api from '../lib/tauri';
import { useChatStore } from './chatStore';
import { useToastStore } from './toastStore';
import { refreshNotebookList } from './notebookRefresh';

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';
const SELECTION_PERSIST_DEBOUNCE_MS = 350;
let persistSelectedSourcesTimer: ReturnType<typeof setTimeout> | null = null;
let persistSelectedSourcesInFlight = false;
let persistSelectedSourcesPending: { notebookId: string; ids: string[] } | null = null;
export type SourceListStatus = 'idle' | 'loading' | 'partial' | 'ready' | 'empty' | 'error';
export type SourceScopeMode = 'none' | 'all' | 'explicit';

function buildSourceScope(
  sources: Source[],
  selectedSourceIds: Set<string>,
  sourceScopeMode: SourceScopeMode,
  sourceListStatus: SourceListStatus,
  stats: NotebookStats | null
): SourceScope {
  // When user explicitly selects no-retrieval, honour it regardless of source
  // list status so chat always works without retrieval.
  if (sourceScopeMode === 'none') {
    return { kind: 'none' };
  }
  // When source list is degraded, do NOT silently downgrade an explicit
  // retrieval request — pass it through so the backend can decide how to
  // handle incomplete source data. Only fall back to 'none' when we have
  // no source information at all (idle).
  if (sourceListStatus === 'idle') {
    return { kind: 'none' };
  }
  if (sources.length === 0) {
    if (sourceListStatus === 'ready' && stats?.source_count && stats.source_count > 0) {
      return { kind: 'none' };
    }
    return { kind: 'none' };
  }
  if (sourceScopeMode === 'all') {
    if (stats?.source_count && stats.source_count !== sources.length) {
      return { kind: 'none' };
    }
    return { kind: 'all' };
  }
  const validIds = new Set(sources.map((source) => source.id));
  const ids = Array.from(selectedSourceIds).filter((id) => validIds.has(id));
  return ids.length > 0 ? { kind: 'explicit', ids } : { kind: 'none' };
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
    console.warn('Failed to persist selected sources:', e);
    useToastStore.getState().addToast({ type: 'error', title: 'Save Failed', message: 'Failed to persist selected sources', duration: 5000 });
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
  sourceScopeMode: SourceScopeMode;
  loading: boolean;
  sourceListStatus: SourceListStatus;
  sourceListError?: string | null;
  stats: NotebookStats | null;
  loadSources: (notebookId: string) => Promise<void>;
  addSourceFile: (notebookId: string, path: string) => Promise<void>;
  addSourceFiles: (notebookId: string, paths: string[]) => Promise<void>;
  addSourceFolder: (notebookId: string, path: string) => Promise<void>;
  addSourcePaste: (notebookId: string, title: string, text: string) => Promise<void>;
  addSourceUrl: (notebookId: string, url: string, networkConsent: boolean) => Promise<void>;
  addSourceYouTubeTranscript: (notebookId: string, url: string, language: string | null, networkConsent: boolean) => Promise<void>;
  deleteSource: (notebookId: string, sourceId: string) => Promise<void>;
  quarantineFailedImports: (notebookId: string) => Promise<void>;
  deleteFailedImports: (notebookId: string) => Promise<void>;
  retrySource: (notebookId: string, sourceId: string) => Promise<void>;
  reindexSource: (notebookId: string, sourceId: string) => Promise<void>;
  reindexNotebook: (notebookId: string) => Promise<void>;
  bulkDeleteSelected: (notebookId: string) => Promise<void>;
  toggleSource: (sourceId: string) => void;
  toggleGroup: (group: string) => void;
  selectAll: () => void;
  selectNone: () => void;
  getSourceScope: () => SourceScope;
  markSourceListPartial: (expectedTotal?: number) => void;
  updateSourceStatus: (sourceId: string, status: string) => void;
  updateSourceStatusBulk: (updates: Array<{ sourceId: string; status: string; errorMessage?: string }>) => void;
  loadStats: (notebookId: string) => Promise<void>;
  resetForNotebookSwitch: () => void;
}

export const useSourceStore = create<SourceStore>((set, get) => ({
  sources: [],
  selectedSourceIds: new Set<string>(),
  sourceScopeMode: 'none',
  loading: false,
  sourceListStatus: 'idle',
  sourceListError: null,
  stats: null,

  loadSources: async (notebookId) => {
    set({ loading: true, sourceListStatus: 'loading', sourceListError: null });
    try {
      const sources = await api.listSources(notebookId);
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      const selectedIds = new Set(sources.filter(s => s.selected).map(s => s.id));
      const sourceScopeMode: SourceScopeMode =
        sources.length === 0
          ? 'none'
          : selectedIds.size === sources.length
            ? 'all'
            : selectedIds.size > 0
              ? 'explicit'
              : 'none';
      set({
        sources,
        selectedSourceIds: selectedIds,
        sourceScopeMode,
        loading: false,
        sourceListStatus: sources.length === 0 ? 'empty' : 'ready',
        sourceListError: null,
      });
    } catch (e) {
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      console.warn('Failed to load sources:', e);
      set({
        loading: false,
        sourceListStatus: 'error',
        sourceListError: String(e),
      });
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

  addSourceUrl: async (notebookId, url, networkConsent) => {
    try {
      clearSuggestedQuestions();
      await api.addSourceUrl(notebookId, url, networkConsent);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'URL Import Started',
        message: 'Fetched URL text is queued for indexing.',
        duration: 3000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'URL Import Failed',
        message: String(e),
        duration: 6000,
      });
    }
  },

  addSourceYouTubeTranscript: async (notebookId, url, language, networkConsent) => {
    try {
      clearSuggestedQuestions();
      await api.addSourceYouTubeTranscript(notebookId, url, language, networkConsent);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'YouTube Transcript Import Started',
        message: 'Fetched transcript text with timestamps is queued for indexing.',
        duration: 3000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'YouTube Transcript Import Failed',
        message: String(e),
        duration: 6000,
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

  quarantineFailedImports: async (notebookId) => {
    try {
      clearSuggestedQuestions();
      const receipt = await api.quarantineFailedImports(notebookId);
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'Failed Imports Quarantined',
        message: `${receipt.quarantined_sources} failed sources quarantined; ${receipt.cancelled_queue_jobs} queued jobs cancelled.`,
        duration: 5000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Quarantine Failed',
        message: String(e),
        duration: 6000,
      });
    }
  },

  deleteFailedImports: async (notebookId) => {
    try {
      clearSuggestedQuestions();
      const receipt = await api.deleteFailedImports(notebookId);
      await refreshNotebookList();
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'Failed Imports Deleted',
        message: `${receipt.deleted_sources} failed sources deleted; ${receipt.cancelled_queue_jobs} queued jobs cancelled.`,
        duration: 5000,
      });
    } catch (e) {
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Delete Failed Imports Failed',
        message: String(e),
        duration: 6000,
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
      const receipt = await api.semanticMemoryBackfillNotebook(notebookId);
      await get().loadSources(notebookId);
      await get().loadStats(notebookId);
      useToastStore.getState().addToast({
        type: 'success',
        title: 'Projection backfill complete',
        message: `${receipt.projected_sources} projected, ${receipt.skipped_no_chunks} skipped, ${receipt.failed_sources} failed.`,
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
      const sourceScopeMode: SourceScopeMode =
        next.size === 0 ? 'none' : next.size === state.sources.length ? 'all' : 'explicit';
      persistSelectedSources(next);
      clearSuggestedQuestions();
      return { selectedSourceIds: next, sourceScopeMode };
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
      const sourceScopeMode: SourceScopeMode =
        next.size === 0 ? 'none' : next.size === state.sources.length ? 'all' : 'explicit';
      persistSelectedSources(next);
      clearSuggestedQuestions();
      return { selectedSourceIds: next, sourceScopeMode };
    });
  },

  selectAll: () => {
    set((state) => {
      const next = new Set(state.sources.map(s => s.id));
      persistSelectedSources(next);
      clearSuggestedQuestions();
      return { selectedSourceIds: next, sourceScopeMode: next.size > 0 ? 'all' : 'none' };
    });
  },

  selectNone: () => {
    const next = new Set<string>();
    persistSelectedSources(next);
    clearSuggestedQuestions();
    set({ selectedSourceIds: next, sourceScopeMode: 'none' });
  },

  getSourceScope: () => {
    const { sources, selectedSourceIds, sourceScopeMode, sourceListStatus, stats } = get();
    return buildSourceScope(sources, selectedSourceIds, sourceScopeMode, sourceListStatus, stats);
  },

  markSourceListPartial: (expectedTotal) => {
    set((state) => ({
      sourceListStatus: 'partial',
      sourceListError: null,
      stats: state.stats
        ? { ...state.stats, source_count: Math.max(state.stats.source_count, expectedTotal ?? 0) }
        : state.stats,
    }));
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
      set((state) => ({
        stats,
        sourceListStatus:
          state.sources.length > 0 && stats.source_count > state.sources.length
            ? 'partial'
            :
          state.sourceListStatus === 'empty' && stats.source_count > 0
            ? 'error'
            : state.sourceListStatus,
        sourceListError:
          state.sourceListStatus === 'empty' && stats.source_count > 0
            ? 'Source list returned empty while notebook stats report sources.'
            : state.sourceListError,
      }));
    } catch {
      // Stats are optional — don't crash on failure
    }
  },

  resetForNotebookSwitch: () => {
    set({
      sources: [],
      selectedSourceIds: new Set<string>(),
      sourceScopeMode: 'none',
      stats: null,
      loading: false,
      sourceListStatus: 'idle',
      sourceListError: null,
    });
  },
}));
