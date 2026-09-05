import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useNotebookStore } from '../notebookStore';
import * as api from '../../lib/tauri';

vi.mock('../../lib/tauri', () => ({
  setActiveNotebook: vi.fn(), listNotebooks: vi.fn(), createNotebook: vi.fn(),
  renameNotebook: vi.fn(), deleteNotebook: vi.fn(),
}));
vi.mock('../chatStore', () => ({ useChatStore: { getState: () => ({ resetForNotebookSwitch: vi.fn() }) } }));
vi.mock('../noteStore', () => ({ useNoteStore: { getState: () => ({ resetForNotebookSwitch: vi.fn() }) } }));
vi.mock('../sourceStore', () => ({ useSourceStore: { getState: () => ({ resetForNotebookSwitch: vi.fn() }) } }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: vi.fn(), removeItem: vi.fn() });
  vi.mocked(api.listNotebooks).mockResolvedValue([]);
  useNotebookStore.setState({ activeNotebookId: 'initial', activationStatus: 'confirmed',
    activationRequestId: 0, activationTargetId: 'initial', activationError: null });
});

describe('notebook activation serializes backend effects', () => {
  it('cannot let A finish after B and confirm a different notebook from the backend', async () => {
    const a = deferred<void>();
    const b = deferred<void>();
    let backendActive: string | null = 'initial';
    vi.mocked(api.setActiveNotebook).mockImplementation(id => {
      backendActive = id;
      return id === 'A' ? a.promise : b.promise;
    });
    const first = useNotebookStore.getState().setActive('A');
    const second = useNotebookStore.getState().setActive('B');
    await vi.waitFor(() => expect(api.setActiveNotebook).toHaveBeenCalledTimes(1));
    expect(api.setActiveNotebook).toHaveBeenCalledWith('A');
    expect(useNotebookStore.getState().activationTargetId).toBe('B');
    expect(useNotebookStore.getState().activeNotebookId).toBe('initial');
    // Even an already-resolved B response cannot run its effect before A.
    b.resolve();
    expect(backendActive).toBe('A');
    a.resolve();
    await Promise.all([first, second]);
    expect(vi.mocked(api.setActiveNotebook).mock.calls.map(([id]) => id)).toEqual(['A', 'B']);
    expect(backendActive).toBe('B');
    expect(useNotebookStore.getState().activeNotebookId).toBe(backendActive);
    expect(useNotebookStore.getState().activationStatus).toBe('confirmed');
  });

  it('keeps pending status for the latest intent after an earlier acknowledgment', async () => {
    const a = deferred<void>();
    const b = deferred<void>();
    vi.mocked(api.setActiveNotebook).mockImplementation(id => id === 'A' ? a.promise : b.promise);
    const first = useNotebookStore.getState().setActive('A');
    const second = useNotebookStore.getState().setActive('B');
    a.resolve();
    await first;
    expect(useNotebookStore.getState().activeNotebookId).toBe('A');
    expect(useNotebookStore.getState().activationStatus).toBe('pending');
    expect(useNotebookStore.getState().activationTargetId).toBe('B');
    b.resolve();
    await second;
    expect(useNotebookStore.getState().activeNotebookId).toBe('B');
  });

  it('retains the acknowledged notebook when the latest activation fails', async () => {
    vi.mocked(api.setActiveNotebook).mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error('notebook validation failed'));
    await useNotebookStore.getState().setActive('A');
    await expect(useNotebookStore.getState().setActive('missing')).rejects.toThrow('notebook validation failed');
    expect(useNotebookStore.getState().activeNotebookId).toBe('A');
    expect(useNotebookStore.getState().activationTargetId).toBe('missing');
    expect(useNotebookStore.getState().activationStatus).toBe('error');
    expect(useNotebookStore.getState().activationError).toBe('notebook validation failed');
  });

  it('executes a later explicit request after failure without retrying the failed target', async () => {
    vi.mocked(api.setActiveNotebook).mockRejectedValueOnce(new Error('A failed')).mockResolvedValueOnce(undefined);
    const first = useNotebookStore.getState().setActive('A');
    const rejected = expect(first).rejects.toThrow('A failed');
    const second = useNotebookStore.getState().setActive('B');
    await rejected;
    await second;
    expect(vi.mocked(api.setActiveNotebook).mock.calls.map(([id]) => id)).toEqual(['A', 'B']);
    expect(useNotebookStore.getState().activeNotebookId).toBe('B');
    expect(useNotebookStore.getState().activationError).toBeNull();
  });

  it('creation awaits its own queued backend activation', async () => {
    const firstAck = deferred<void>();
    const createdAck = deferred<void>();
    vi.mocked(api.setActiveNotebook).mockImplementation(id => id === 'A' ? firstAck.promise : createdAck.promise);
    vi.mocked(api.createNotebook).mockResolvedValue('created');
    const first = useNotebookStore.getState().setActive('A');
    let createdSettled = false;
    const created = useNotebookStore.getState().createNotebook('Notebook').then(id => { createdSettled = true; return id; });
    await vi.waitFor(() => expect(useNotebookStore.getState().activationTargetId).toBe('created'));
    expect(api.setActiveNotebook).toHaveBeenCalledTimes(1);
    expect(createdSettled).toBe(false);
    firstAck.resolve();
    await first;
    await vi.waitFor(() => expect(api.setActiveNotebook).toHaveBeenCalledWith('created'));
    expect(createdSettled).toBe(false);
    createdAck.resolve();
    expect(await created).toBe('created');
    expect(useNotebookStore.getState().activeNotebookId).toBe('created');
  });

  it('local preference storage failure cannot override an acknowledged activation', async () => {
    vi.mocked(api.setActiveNotebook).mockResolvedValue(undefined);
    vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => { throw new Error('storage denied'); }, removeItem: () => { throw new Error('storage denied'); } });
    await useNotebookStore.getState().setActive('A');
    expect(useNotebookStore.getState().activeNotebookId).toBe('A');
    expect(useNotebookStore.getState().activationStatus).toBe('confirmed');
    await useNotebookStore.getState().setActive(null);
    expect(useNotebookStore.getState().activeNotebookId).toBeNull();
    expect(useNotebookStore.getState().activationStatus).toBe('idle');
  });
});
