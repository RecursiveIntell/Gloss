import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useHealthStore } from '../healthStore';
import * as api from '../../lib/tauri';
import type { QueueStatus } from '../../lib/types';

vi.mock('../../lib/tauri', () => ({
  getQueueStatus: vi.fn(), memoryBackendStatus: vi.fn(),
  getSemanticMemoryProfileStatus: vi.fn(), testProvider: vi.fn(),
}));

const queue = (provider: string | null) => ({
  summary_backend: { provider_id: provider, ready: Boolean(provider) },
} as QueueStatus);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(res => { resolve = res; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  useHealthStore.getState().stopPolling();
  useHealthStore.setState({ notebookId: 'nb', providerId: 'chat', chatConnected: null, backgroundConnected: null });
  vi.mocked(api.getQueueStatus).mockResolvedValue(queue('summary'));
  vi.mocked(api.memoryBackendStatus).mockResolvedValue(null as never);
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

  it('does not starve a slow health response by superseding it every timer tick', async () => {
    const slow = deferred<boolean>();
    vi.mocked(api.testProvider).mockReturnValue(slow.promise);
    vi.mocked(api.getQueueStatus).mockResolvedValue(queue('chat'));
    useHealthStore.getState().startPolling('nb', 'chat');
    await vi.advanceTimersByTimeAsync(30000);
    expect(api.getQueueStatus).toHaveBeenCalledTimes(1);
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
