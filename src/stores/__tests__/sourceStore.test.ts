import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useSourceStore } from '../sourceStore';
import { useNotebookStore } from '../notebookStore';
import * as api from '../../lib/tauri';

// Mock the Tauri API layer
vi.mock('../../lib/tauri', () => ({
  listSources: vi.fn().mockResolvedValue([]),
  createConversation: vi.fn().mockResolvedValue('conv-1'),
  setSelectedSources: vi.fn().mockResolvedValue(undefined),
  addSourceFile: vi.fn().mockResolvedValue('src-1'),
  addSourceFiles: vi.fn().mockResolvedValue(['src-1']),
  addSourceFolder: vi.fn().mockResolvedValue(undefined),
  addSourcePaste: vi.fn().mockResolvedValue('src-1'),
  addSourceUrl: vi.fn().mockResolvedValue('src-1'),
  addSourceYouTubeTranscript: vi.fn().mockResolvedValue('src-1'),
  deleteSource: vi.fn().mockResolvedValue(undefined),
  deleteSources: vi.fn().mockResolvedValue(undefined),
  quarantineFailedImports: vi.fn().mockResolvedValue({ quarantined_sources: 0, cancelled_queue_jobs: 0 }),
  deleteFailedImports: vi.fn().mockResolvedValue({ deleted_sources: 0, cancelled_queue_jobs: 0 }),
  retrySourceIngestion: vi.fn().mockResolvedValue(undefined),
  semanticMemoryReindexSource: vi.fn().mockResolvedValue(undefined),
  semanticMemoryBackfillNotebook: vi.fn().mockResolvedValue({ projected_sources: 0, skipped_no_chunks: 0, failed_sources: 0 }),
  getNotebookStats: vi.fn().mockResolvedValue({ source_count: 0, note_count: 0, conversation_count: 0 }),
}));

// Mock the chatStore so clearSuggestedQuestions doesn't explode
vi.mock('../chatStore', () => ({
  useChatStore: {
    getState: vi.fn(() => ({ clearSuggestedQuestions: vi.fn() })),
  },
}));

// Mock notebookRefresh
vi.mock('../notebookRefresh', () => ({
  refreshNotebookList: vi.fn().mockResolvedValue(undefined),
  registerNotebookListRefresher: vi.fn(),
}));

// Mock localStorage
const localStorageMock = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: vi.fn((key: string) => store[key] ?? null),
    setItem: vi.fn((key: string, value: string) => { store[key] = value; }),
    removeItem: vi.fn((key: string) => { delete store[key]; }),
    clear: vi.fn(() => { store = {}; }),
    get length() { return Object.keys(store).length; },
    key: vi.fn((_: number) => null),
  };
})();
vi.stubGlobal('localStorage', localStorageMock);

describe('sourceStore', () => {
  beforeEach(() => {
    useSourceStore.setState({
      sources: [],
      selectedSourceIds: new Set<string>(),
      sourceScopeMode: 'none',
      loading: false,
      sourceListStatus: 'idle',
      sourceListError: null,
      stats: null,
    });
    localStorageMock.clear();
    localStorageMock.setItem('gloss:activeNotebookId', 'nb-1');
    useNotebookStore.setState({ activeNotebookId: 'nb-1', activationStatus: 'confirmed' });
  });

  it('initial state has empty source list', () => {
    const state = useSourceStore.getState();
    expect(state.sources).toEqual([]);
    expect(state.sourceListStatus).toBe('idle');
    expect(state.loading).toBe(false);
  });

  it('setting sourceListStatus to error is reflected in state', () => {
    useSourceStore.setState({ sourceListStatus: 'error', sourceListError: 'Something broke' });

    const state = useSourceStore.getState();
    expect(state.sourceListStatus).toBe('error');
    expect(state.sourceListError).toBe('Something broke');
  });

  it('preserves all-source retrieval when source list is partial but has source information', () => {
    useSourceStore.setState({
      sources: [
        {
          id: 'src-loaded',
          source_type: 'file',
          title: 'Loaded source',
          status: 'ready',
          selected: true,
          created_at: '2026-06-10T00:00:00Z',
          updated_at: '2026-06-10T00:00:00Z',
        },
      ],
      selectedSourceIds: new Set(['src-loaded']),
      sourceScopeMode: 'all',
      sourceListStatus: 'partial',
      stats: {
        source_count: 2,
        ready_count: 1,
        error_count: 0,
        missing_summaries: 0,
        chunk_count: 1,
        sources_with_chunks: 1,
        total_words: 3,
      },
    });

    expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'all' });
  });

  it('does not let a late notebook A load overwrite notebook B', async () => {
    let resolveA!: (sources: never[]) => void;
    let resolveB!: (sources: never[]) => void;
    vi.mocked(api.listSources)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveA = resolve; }))
      .mockImplementationOnce(() => new Promise((resolve) => { resolveB = resolve; }));
    useNotebookStore.setState({ activeNotebookId: 'nb-a', activationStatus: 'confirmed' });
    const a = useSourceStore.getState().loadSources('nb-a');
    useNotebookStore.setState({ activeNotebookId: 'nb-b', activationStatus: 'confirmed' });
    const b = useSourceStore.getState().loadSources('nb-b');
    resolveA([]);
    await a;
    resolveB([]);
    await b;
    expect(useSourceStore.getState().loadedNotebookId).toBe('nb-b');
  });

  it('preserves invalid explicit ids instead of silently changing scope to none', () => {
    useSourceStore.setState({ sourceScopeMode: 'explicit', selectedSourceIds: new Set(['missing-source']), sourceListStatus: 'ready' });
    expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'explicit', ids: ['missing-source'] });
  });
});
