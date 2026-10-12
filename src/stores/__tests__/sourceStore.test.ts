import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { useSourceStore } from '../sourceStore';
import { useNotebookStore } from '../notebookStore';
import { useToastStore } from '../toastStore';
import * as api from '../../lib/tauri';
import type { Source, IndexSourceReceipt, SemanticMemoryBackfillReceipt } from '../../lib/types';

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
  retryFailedImports: vi.fn().mockResolvedValue({
    failed_sources_before: 3,
    recovered_by_dense_rebuild: 2,
    retried_sources: 1,
    already_running_sources: 0,
    queued_sources: 1,
    completed_sources: 0,
    failed_sources: 0,
    errors: [],
  }),
  semanticMemoryReindexSource: vi.fn(),
  semanticMemoryBackfillNotebook: vi.fn(),
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
  afterEach(async () => {
    if (vi.isFakeTimers()) await vi.runOnlyPendingTimersAsync();
    vi.useRealTimers();
  });
  it.each(['toggleSource', 'toggleGroup'] as const)('does not widen explicit scope when %s sees stale selected identities', async (action) => {
    vi.useFakeTimers();
    const source = (id: string, title: string) => ({ id, title, selected: false } as Source);
    useNotebookStore.setState({ activeNotebookId: 'nb-1', activationStatus: 'confirmed' });
    useSourceStore.setState({ sources: [source('gone', 'old/file'), source('A', 'group/file'), source('C', 'other/file')], selectedSourceIds: new Set(), sourceScopeMode: 'none', sourceListStatus: 'ready' });
    useSourceStore.getState().toggleSource('gone');
    // Deletion/refresh can complete before the debounced selection write.
    vi.mocked(api.listSources).mockResolvedValueOnce([source('A', 'group/file'), source('C', 'other/file')]);
    await useSourceStore.getState().deleteSource('nb-1', 'gone');
    expect(useSourceStore.getState().selectedSourceIds.has('gone')).toBe(true);
    if (action === 'toggleSource') useSourceStore.getState().toggleSource('A');
    else useSourceStore.getState().toggleGroup('group');
    expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'explicit', ids: ['gone', 'A'] });
    await vi.runAllTimersAsync();
    vi.useRealTimers();
  });
  beforeEach(() => {
    vi.clearAllMocks();
    useToastStore.setState({ toasts: [] });
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

  it.each([
    ['idle', 1],
    ['loading', 1],
    ['partial', 1],
    ['error', 1],
    ['ready', 1],
    ['empty', 0],
  ] as const)('preserves declared all with an empty %s inventory and source count %i', (sourceListStatus, sourceCount) => {
    useSourceStore.setState({
      sources: [],
      selectedSourceIds: new Set(sourceCount > 0 ? ['known'] : []),
      sourceScopeMode: 'all',
      sourceListStatus,
      stats: {
        source_count: sourceCount,
        ready_count: 0,
        error_count: 0,
        missing_summaries: 0,
        chunk_count: 0,
        sources_with_chunks: 0,
        total_words: 0,
      },
    });

    expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'all' });
    expect(useSourceStore.getState().sourceScopeMode).toBe('all');
  });

  it.each(['idle', 'loading', 'partial', 'error', 'ready', 'empty'] as const)(
    'preserves none and exact explicit intent with an empty %s inventory', (sourceListStatus) => {
      useSourceStore.setState({
        sources: [],
        selectedSourceIds: new Set(['missing-source']),
        sourceScopeMode: 'none',
        sourceListStatus,
      });
      expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'none' });

      useSourceStore.setState({ sourceScopeMode: 'explicit' });
      expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'explicit', ids: ['missing-source'] });

      useSourceStore.setState({ selectedSourceIds: new Set() });
      expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'explicit', ids: [] });
    },
  );

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

  it('retries failed imports through one bounded backend operation', async () => {
    await useSourceStore.getState().retryFailedSources('nb-1');

    expect(api.retryFailedImports).toHaveBeenCalledTimes(1);
    expect(api.retryFailedImports).toHaveBeenCalledWith('nb-1');
    expect(api.retrySourceIngestion).not.toHaveBeenCalled();
    expect(api.listSources).toHaveBeenCalledTimes(1);
    expect(api.getNotebookStats).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['synced', null, 'success', 'Reindex Complete'],
    ['skipped_no_chunks', null, 'info', 'Reindex Skipped'],
    ['partial', 'one chunk failed', 'warning', 'Reindex Incomplete'],
    ['failed', 'model unavailable', 'error', 'Reindex Failed'],
    ['synced', 'receipt reports an error', 'error', 'Reindex Failed'],
  ] as const)('shows the %s source receipt outcome', async (syncStatus, error, type, title) => {
    const receipt: IndexSourceReceipt = {
      backend_id: 'semantic-memory-preview', notebook_id: 'nb-1', source_id: 'src-1',
      receipt_id: 'receipt-fixture', indexed_chunks: syncStatus === 'skipped_no_chunks' ? 0 : 2, sync_status: syncStatus, error,
    };
    vi.mocked(api.semanticMemoryReindexSource).mockResolvedValueOnce(receipt);
    await useSourceStore.getState().reindexSource('nb-1', 'src-1');
    const toasts = useToastStore.getState().toasts;
    expect(toasts[toasts.length - 1]).toMatchObject({ type, title });
    if (error) expect(toasts[toasts.length - 1]?.message).toContain(error);
  });

  it.each(['failed', 'partial', 'stale', 'healthy'])('reports %s notebook backfill truth', async (outcome) => {
    const failed = outcome === 'failed';
    const partial = outcome === 'partial';
    const stale = outcome === 'stale';
    const receipt = {
      notebook_id: 'nb-1', receipt_id: 'receipt-fixture', total_sources: 2,
      chunk_bearing_sources: 2, projected_sources: failed || partial ? 1 : 2,
      skipped_no_chunks: 0, failed_sources: failed ? 1 : 0,
      stale_sources: stale ? 1 : 0, total_chunks: 4, projected_chunks: partial ? 3 : 4,
      errors: failed ? [{ source_id: 'src-2', title: 'Fixture', error: 'projection failed' }] : [],
      projection_summary: {
        notebook_id: 'nb-1', total_sources: 2, chunk_bearing_sources: 2, zero_chunk_sources: 0,
        projected_sources: failed || partial ? 1 : 2, failed_sources: failed ? 1 : 0,
        skipped_no_chunks: 0, stale_sources: stale ? 1 : 0,
        partial_sources: partial ? 1 : 0, projecting_sources: 0,
        healthy_links: partial ? 3 : 4, degraded_links: failed ? 1 : 0, missing_links: 0,
        total_chunks: 4, projected_chunks: partial ? 3 : 4, projection_required: outcome !== 'healthy',
      },
    } satisfies SemanticMemoryBackfillReceipt;
    vi.mocked(api.semanticMemoryBackfillNotebook).mockResolvedValueOnce(receipt);
    await useSourceStore.getState().reindexNotebook('nb-1');
    const toasts = useToastStore.getState().toasts;
    expect(toasts[toasts.length - 1]).toMatchObject({
      type: outcome === 'healthy' ? 'success' : 'warning',
      title: outcome === 'healthy' ? 'Projection Backfill Complete' : 'Projection Backfill Needs Attention',
    });
    expect(toasts[toasts.length - 1]?.message).toContain(`${receipt.projected_chunks} chunks projected`);
  });

  it('warns when fresh projection summary requires more work after the backfill snapshot', async () => {
    const receipt: SemanticMemoryBackfillReceipt = {
      notebook_id: 'nb-1', receipt_id: 'receipt-snapshot', total_sources: 1,
      chunk_bearing_sources: 1, projected_sources: 1, skipped_no_chunks: 0,
      failed_sources: 0, stale_sources: 0, total_chunks: 1, projected_chunks: 1, errors: [],
      projection_summary: {
        notebook_id: 'nb-1', total_sources: 2, chunk_bearing_sources: 2, zero_chunk_sources: 0,
        projected_sources: 1, failed_sources: 0, skipped_no_chunks: 0, stale_sources: 0,
        partial_sources: 0, projecting_sources: 0, healthy_links: 1, degraded_links: 0,
        missing_links: 1, total_chunks: 2, projected_chunks: 1, projection_required: true,
      },
    };
    vi.mocked(api.semanticMemoryBackfillNotebook).mockResolvedValueOnce(receipt);
    await useSourceStore.getState().reindexNotebook('nb-1');
    const toasts = useToastStore.getState().toasts;
    expect(toasts[toasts.length - 1]).toMatchObject({
      type: 'warning', title: 'Projection Backfill Needs Attention',
    });
  });
});
