import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useHealthStore } from '../healthStore';
import * as api from '../../lib/tauri';
import type { MemoryBackendStatus, QueueStatus } from '../../lib/types';

vi.mock('../../lib/tauri', () => ({
  getQueueStatus: vi.fn(), memoryBackendStatus: vi.fn(),
  getSemanticMemoryProfileStatus: vi.fn(), testProvider: vi.fn(),
}));

const queue = (provider: string | null) => ({
  summary_backend: { provider_id: provider, ready: Boolean(provider) },
} as QueueStatus);

const memory = (overrides: Partial<MemoryBackendStatus> = {}): MemoryBackendStatus => ({
  backend_id: 'semantic-memory-preview', default_backend: 'semantic-memory-preview',
  active_backend: 'semantic-memory-preview', backend_used: 'semantic-memory-preview', available: true,
  semantic_memory_feature_enabled: true, semantic_memory_available: true,
  index_sync_status: 'synced', sync_status: 'synced', degradation_markers: [],
  embedding_index_metadata: [], degraded: false, ...overrides,
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  useHealthStore.getState().stopPolling();
  useHealthStore.setState({ notebookId: 'nb', providerId: 'chat', chatConnected: null, backgroundConnected: null,
    queueStatus: null, profileStatus: null, memoryStatus: null, memoryState: 'checking', memoryError: null });
  vi.mocked(api.getQueueStatus).mockResolvedValue(queue('summary'));
  vi.mocked(api.memoryBackendStatus).mockResolvedValue(memory());
  vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValue(null as never);
  vi.mocked(api.testProvider).mockResolvedValue(true);
});

afterEach(() => { useHealthStore.getState().stopPolling(); vi.useRealTimers(); });

describe('health polling owns exact provider identity', () => {
  it('checks the configured foreground and canonical queue provider separately', async () => {
    await useHealthStore.getState().poll();
    expect(api.testProvider).toHaveBeenCalledWith('chat');
    expect(api.testProvider).toHaveBeenCalledWith('summary');
    expect(useHealthStore.getState().chatConnected).toBe(true);
    expect(useHealthStore.getState().backgroundConnected).toBe(true);
    expect(useHealthStore.getState().backgroundProviderId).toBe('summary');
  });

  it('keeps background failure separate from a healthy chat provider', async () => {
    vi.mocked(api.testProvider).mockImplementation(async id => {
      if (id === 'summary') throw new Error('unreachable');
      return true;
    });
    await useHealthStore.getState().poll();
    expect(useHealthStore.getState().chatConnected).toBe(true);
    expect(useHealthStore.getState().backgroundConnected).toBe(false);
  });

  it('reuses a check only when both provider IDs match exactly', async () => {
    vi.mocked(api.getQueueStatus).mockResolvedValue(queue('chat'));
    await useHealthStore.getState().poll();
    expect(api.testProvider).toHaveBeenCalledExactlyOnceWith('chat');
    expect(useHealthStore.getState().backgroundConnected).toBe(true);
  });

  it('does not invent background connectivity when queue capture fails', async () => {
    vi.mocked(api.getQueueStatus).mockRejectedValue(new Error('queue unavailable'));
    await useHealthStore.getState().poll();
    expect(api.testProvider).toHaveBeenCalledExactlyOnceWith('chat');
    expect(useHealthStore.getState().backgroundProviderId).toBeNull();
    expect(useHealthStore.getState().backgroundConnected).toBeNull();
  });

  it('invalidates old health immediately and rejects a late prior-provider response', async () => {
    const old = deferred<boolean>();
    vi.mocked(api.testProvider).mockImplementation(id => id === 'old' ? old.promise : Promise.resolve(false));
    useHealthStore.setState({ providerId: 'old', chatConnected: true, backgroundConnected: true });
    const prior = useHealthStore.getState().poll();
    useHealthStore.getState().startPolling('nb-new', 'new');
    expect(useHealthStore.getState().chatConnected).toBeNull();
    expect(useHealthStore.getState().backgroundConnected).toBeNull();
    await vi.advanceTimersByTimeAsync(0);
    old.resolve(true);
    await prior;
    expect(useHealthStore.getState().providerId).toBe('new');
    expect(useHealthStore.getState().notebookId).toBe('nb-new');
    expect(useHealthStore.getState().chatConnected).toBe(false);
  });

  it('does not let a late background check overwrite newer notebook health', async () => {
    const old = deferred<boolean>();
    vi.mocked(api.getQueueStatus).mockResolvedValueOnce(queue('old-summary')).mockResolvedValue(queue('new-summary'));
    vi.mocked(api.testProvider).mockImplementation(id => id === 'old-summary' ? old.promise : Promise.resolve(false));
    const prior = useHealthStore.getState().poll();
    await vi.advanceTimersByTimeAsync(0);
    useHealthStore.getState().startPolling('nb-new', 'new');
    await vi.advanceTimersByTimeAsync(0);
    old.resolve(true);
    await prior;
    expect(useHealthStore.getState().backgroundProviderId).toBe('new-summary');
    expect(useHealthStore.getState().backgroundConnected).toBe(false);
  });

  it('keeps one timer for repeated subscriptions to the same identity and stops it', async () => {
    useHealthStore.getState().startPolling('nb', 'chat');
    useHealthStore.getState().startPolling('nb', 'chat');
    await vi.advanceTimersByTimeAsync(0);
    expect(api.getQueueStatus).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(1);
    await vi.advanceTimersByTimeAsync(10000);
    expect(api.getQueueStatus).toHaveBeenCalledTimes(2);
    useHealthStore.getState().stopPolling();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('rejects a late background result after an independent queue refresh changes its provider', async () => {
    const old = deferred<boolean>();
    vi.mocked(api.getQueueStatus).mockResolvedValueOnce(queue('old-summary')).mockResolvedValue(queue('new-summary'));
    vi.mocked(api.testProvider).mockImplementation(id => id === 'old-summary' ? old.promise : Promise.resolve(false));
    const prior = useHealthStore.getState().poll();
    await vi.advanceTimersByTimeAsync(0);
    expect(useHealthStore.getState().backgroundProviderId).toBe('old-summary');
    await useHealthStore.getState().poll();
    old.resolve(true);
    await prior;
    expect(useHealthStore.getState()).toMatchObject({ backgroundProviderId: 'new-summary', backgroundConnected: false });
  });

  it('coalesces a slow provider while refreshing independent local evidence each tick', async () => {
    const slow = deferred<boolean>();
    vi.mocked(api.testProvider).mockReturnValue(slow.promise);
    vi.mocked(api.getQueueStatus).mockResolvedValue(queue('chat'));
    useHealthStore.getState().startPolling('nb', 'chat');
    await vi.advanceTimersByTimeAsync(30000);
    expect(api.getQueueStatus).toHaveBeenCalledTimes(4);
    expect(api.memoryBackendStatus).toHaveBeenCalledTimes(4);
    expect(api.getSemanticMemoryProfileStatus).toHaveBeenCalledTimes(4);
    expect(api.testProvider).toHaveBeenCalledExactlyOnceWith('chat');
    expect(useHealthStore.getState().memoryState).toBe('ready');
    slow.resolve(true);
    await vi.advanceTimersByTimeAsync(0);
    expect(useHealthStore.getState().chatConnected).toBe(true);
    expect(useHealthStore.getState().backgroundConnected).toBe(true);
  });

  it('rejects an old endpoint result after settings restart health for the same provider', async () => {
    const oldEndpoint = deferred<boolean>();
    vi.mocked(api.getQueueStatus).mockResolvedValue(queue('chat'));
    vi.mocked(api.testProvider).mockReturnValueOnce(oldEndpoint.promise).mockResolvedValue(true);
    useHealthStore.getState().startPolling('nb', 'chat');
    const prior = useHealthStore.getState().poll();
    useHealthStore.getState().stopPolling();
    useHealthStore.getState().startPolling('nb', 'chat');
    expect(useHealthStore.getState().chatConnected).toBeNull();
    await vi.advanceTimersByTimeAsync(0);
    expect(useHealthStore.getState().chatConnected).toBe(true);
    oldEndpoint.resolve(false);
    await prior;
    expect(useHealthStore.getState().chatConnected).toBe(true);
    expect(vi.getTimerCount()).toBe(1);
  });
});

describe('memory health is an independent, truthful probe', () => {
  it('publishes memory, queue and profile before either provider resolves', async () => {
    const chat = deferred<boolean>();
    const summary = deferred<boolean>();
    vi.mocked(api.testProvider).mockImplementation(id => id === 'chat' ? chat.promise : summary.promise);
    const profile = { compiled_semantic_memory: true } as Awaited<ReturnType<typeof api.getSemanticMemoryProfileStatus>>;
    vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValue(profile);
    const poll = useHealthStore.getState().poll();
    await vi.advanceTimersByTimeAsync(0);
    expect(useHealthStore.getState()).toMatchObject({
      memoryState: 'ready', memoryStatus: memory(), memoryError: null,
      queueStatus: queue('summary'), profileStatus: profile, backgroundProviderId: 'summary',
      chatConnected: null, backgroundConnected: null,
    });
    chat.resolve(true); summary.resolve(false);
    await poll;
    expect(useHealthStore.getState().backgroundConnected).toBe(false);
  });

  it('publishes memory while unrelated queue and profile reads remain pending', async () => {
    const slowQueue = deferred<QueueStatus>();
    const slowProfile = deferred<Awaited<ReturnType<typeof api.getSemanticMemoryProfileStatus>>>();
    vi.mocked(api.getQueueStatus).mockReturnValue(slowQueue.promise);
    vi.mocked(api.getSemanticMemoryProfileStatus).mockReturnValue(slowProfile.promise);
    const poll = useHealthStore.getState().poll();
    await vi.advanceTimersByTimeAsync(0);
    expect(useHealthStore.getState().memoryStatus).toEqual(memory());
    expect(useHealthStore.getState().memoryState).toBe('ready');
    slowQueue.resolve(queue('summary')); slowProfile.resolve(null as never);
    await poll;
  });

  it('coalesces a pending memory read and shows checking until that read completes', async () => {
    const pending = deferred<MemoryBackendStatus>();
    vi.mocked(api.memoryBackendStatus).mockReturnValue(pending.promise);
    useHealthStore.getState().startPolling('nb', 'chat');
    const poll = useHealthStore.getState().poll();
    await vi.advanceTimersByTimeAsync(30000);
    expect(api.memoryBackendStatus).toHaveBeenCalledExactlyOnceWith('nb');
    expect(api.getQueueStatus).toHaveBeenCalledTimes(4);
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'checking', memoryStatus: null, memoryError: null });
    pending.resolve(memory());
    await poll;
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'ready', memoryStatus: memory() });
  });

  it('retains a bounded useful read error, then replaces it on success', async () => {
    const message = 'Memory IPC unavailable: ' + 'x'.repeat(800);
    vi.mocked(api.memoryBackendStatus).mockRejectedValueOnce(new Error(message));
    await useHealthStore.getState().poll();
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'unavailable', memoryStatus: null, memoryError: message.slice(0, 500) });
    const next = useHealthStore.getState().poll();
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'checking', memoryError: null });
    await next;
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'ready', memoryStatus: memory(), memoryError: null });
  });

  it('retains a native string error without claiming a fallback', async () => {
    vi.mocked(api.memoryBackendStatus).mockRejectedValue('notebook locked');
    await useHealthStore.getState().poll();
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'unavailable', memoryError: 'notebook locked', memoryStatus: null });
  });

  it('preserves the returned requested backend, actual backend and fallback reason', async () => {
    const fallback = memory({ backend_used: 'gloss-local', degraded: true, fallback_reason: 'preview index not ready', degradation_markers: ['preview-not-ready'] });
    vi.mocked(api.memoryBackendStatus).mockResolvedValue(fallback);
    await useHealthStore.getState().poll();
    expect(useHealthStore.getState().memoryStatus).toBe(fallback);
    expect(useHealthStore.getState().memoryState).toBe('ready');
  });

  it.each(['success', 'error'] as const)('rejects late memory %s after settings restart for the same identity', async outcome => {
    const old = deferred<MemoryBackendStatus>();
    vi.mocked(api.memoryBackendStatus).mockReturnValueOnce(old.promise).mockResolvedValue(memory());
    useHealthStore.getState().startPolling('nb', 'chat');
    const prior = useHealthStore.getState().poll();
    useHealthStore.getState().stopPolling();
    useHealthStore.getState().startPolling('nb', 'chat');
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'checking', memoryStatus: null });
    await vi.advanceTimersByTimeAsync(0);
    if (outcome === 'error') old.reject(new Error('old settings'));
    else old.resolve(memory({ backend_used: 'gloss-local', degraded: true }));
    await prior;
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'ready', memoryStatus: memory(), memoryError: null });
  });

  it('does not publish a pending memory result after polling stops', async () => {
    const pending = deferred<MemoryBackendStatus>();
    vi.mocked(api.memoryBackendStatus).mockReturnValue(pending.promise);
    const poll = useHealthStore.getState().poll();
    useHealthStore.getState().stopPolling();
    pending.resolve(memory());
    await poll;
    expect(useHealthStore.getState()).toMatchObject({ memoryState: 'checking', memoryStatus: null });
  });
});
