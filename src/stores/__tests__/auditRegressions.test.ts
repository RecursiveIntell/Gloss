import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useChatStore } from '../chatStore';
import { useNotebookStore } from '../notebookStore';
import { useNoteStore } from '../noteStore';
import { useSettingsStore } from '../settingsStore';
import { useSourceStore } from '../sourceStore';
import * as api from '../../lib/tauri';
import type { Note, Source } from '../../lib/types';

vi.mock('../../lib/tauri', () => ({
  createConversation: vi.fn(), listConversations: vi.fn().mockResolvedValue([]),
  sendMessage: vi.fn(), stopChat: vi.fn().mockResolvedValue(undefined), loadMessages: vi.fn().mockResolvedValue([]),
  listNotes: vi.fn(), listSources: vi.fn(), getAllModels: vi.fn(),
  selectChatModel: vi.fn(), getSettings: vi.fn(),
  setSelectedSources: vi.fn().mockResolvedValue(undefined),
  getChatEventsSince: vi.fn().mockResolvedValue([]),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('localStorage', { getItem: () => 'nb-1' });
  useNotebookStore.setState({ activeNotebookId: 'nb-1', activationStatus: 'confirmed' });
  useChatStore.setState({ activeConversationId: 'conv-1', messages: [], isStreaming: false,
    streamingMessageId: null, streamingNotebookId: null, streamingError: null,
    streamingContent: '', streamingStatus: null, pendingMessageIds: {}, pendingEvidence: {}, replayCursors: {} });
  useNoteStore.getState().resetForNotebookSwitch();
  useSettingsStore.setState({ modelsLoaded: false, selectionPending: false, selectionError: null, activeModel: 'old', settings: { default_provider: 'ollama', default_model: 'old' }, models: [
    { id: 'new', provider_id: 'openai', display_name: 'New', available: true, stale: false },
  ] });
  useSourceStore.getState().resetForNotebookSwitch();
});

describe('audit regressions: asynchronous ownership', () => {
  it('Stop during conversation creation cancels preparation without sending', async () => {
    const created = deferred<string>();
    useChatStore.setState({ activeConversationId: null });
    vi.mocked(api.createConversation).mockReturnValueOnce(created.promise);
    const sending = useChatStore.getState().sendMessage('nb-1', 'cancel me', { kind: 'none' }, 'model');
    await useChatStore.getState().stopStreaming('nb-1');
    expect(useChatStore.getState().isStreaming).toBe(false);
    created.resolve('cancelled-conversation');
    await sending;
    expect(api.sendMessage).not.toHaveBeenCalled();
    expect(api.stopChat).not.toHaveBeenCalled();
    expect(useChatStore.getState().activeConversationId).toBeNull();
    expect(useChatStore.getState().pendingMessageIds).toEqual({});
  });

  it('submitted Stop waits for the matching backend terminal event', async () => {
    vi.mocked(api.sendMessage).mockImplementationOnce(async (...args) => args[5]!);
    await useChatStore.getState().sendMessage('nb-1', 'sent', { kind: 'none' }, 'model');
    const id = useChatStore.getState().streamingMessageId!;
    await useChatStore.getState().stopStreaming('nb-1');
    expect(api.stopChat).toHaveBeenCalledExactlyOnceWith('nb-1');
    expect(useChatStore.getState().isStreaming).toBe(true);
    expect(useChatStore.getState().streamingMessageId).toBe(id);
    useChatStore.getState().handleChatCancelled('nb-1', 'conv-1', id, 'Stopped');
    expect(useChatStore.getState().isStreaming).toBe(false);
  });

  it('inactive source refresh cannot invalidate an active notebook load', async () => {
    const active = deferred<Source[]>();
    useNotebookStore.setState({ activeNotebookId: 'nb-2' });
    vi.mocked(api.listSources).mockReturnValueOnce(active.promise).mockResolvedValueOnce([]);
    const loading = useSourceStore.getState().loadSources('nb-2');
    const epoch = useSourceStore.getState().loadEpoch;
    await useSourceStore.getState().loadSources('nb-1');
    expect(api.listSources).toHaveBeenCalledExactlyOnceWith('nb-2');
    expect(useSourceStore.getState().loadEpoch).toBe(epoch);
    active.resolve([{ id: 'active', selected: true } as Source]);
    await loading;
    expect(useSourceStore.getState().sources.map(s => s.id)).toEqual(['active']);
    expect(useSourceStore.getState().loading).toBe(false);
    expect(useSourceStore.getState().loadedNotebookId).toBe('nb-2');
  });

  it('model selection commits one acknowledged provider/model pair', async () => {
    const committed = deferred<void>();
    vi.mocked(api.selectChatModel).mockReturnValueOnce(committed.promise);
    const selecting = useSettingsStore.getState().selectModel('openai', 'new');
    expect(useSettingsStore.getState().activeModel).toBe('old');
    expect(useSettingsStore.getState().settings.default_provider).toBe('ollama');
    expect(useSettingsStore.getState().selectionPending).toBe(true);
    committed.resolve();
    await selecting;
    expect(api.selectChatModel).toHaveBeenCalledExactlyOnceWith('openai', 'new');
    expect(useSettingsStore.getState().settings.default_provider).toBe('openai');
    expect(useSettingsStore.getState().activeModel).toBe('new');
    expect(useSettingsStore.getState().selectionPending).toBe(false);
  });

  it('model persistence failure preserves the prior pair', async () => {
    vi.mocked(api.selectChatModel).mockRejectedValueOnce(new Error('disk failure'));
    await expect(useSettingsStore.getState().selectModel('openai', 'new')).rejects.toThrow('disk failure');
    expect(useSettingsStore.getState().activeModel).toBe('old');
    expect(useSettingsStore.getState().settings.default_provider).toBe('ollama');
    expect(useSettingsStore.getState().selectionError).toContain('disk failure');
    expect(useSettingsStore.getState().selectionPending).toBe(false);
  });

  it('late settings load cannot revert a committed selection', async () => {
    const loaded = deferred<Record<string, string>>();
    vi.mocked(api.getSettings).mockReturnValueOnce(loaded.promise);
    vi.mocked(api.selectChatModel).mockResolvedValueOnce(undefined);
    const loading = useSettingsStore.getState().loadSettings();
    await useSettingsStore.getState().selectModel('openai', 'new');
    loaded.resolve({ default_provider: 'ollama', default_model: 'old' });
    await loading;
    expect(useSettingsStore.getState().activeModel).toBe('new');
    expect(useSettingsStore.getState().settings.default_provider).toBe('openai');
  });

  it('settings loaded after discovery retain exact-pair readiness failure', async () => {
    vi.mocked(api.getAllModels).mockResolvedValueOnce([{ id: 'shared', provider_id: 'openai', display_name: 'Shared', available: true, stale: false }]);
    await useSettingsStore.getState().loadModels();
    vi.mocked(api.getSettings).mockResolvedValueOnce({ default_provider: 'ollama', default_model: 'shared' });
    await useSettingsStore.getState().loadSettings();
    expect(useSettingsStore.getState().selectionError).toContain('unavailable');
  });
  for (const outcome of ['resolve', 'reject'] as const) {
    it(`late send ${outcome} cannot clear a newer stream`, async () => {
      const old = deferred<string>();
      vi.mocked(api.sendMessage).mockReturnValueOnce(old.promise);
      const sending = useChatStore.getState().sendMessage('nb-1', 'old', { kind: 'none' }, 'model');
      useNotebookStore.setState({ activeNotebookId: 'nb-2' });
      useChatStore.setState({ activeConversationId: 'conv-2', isStreaming: true,
        streamingNotebookId: 'nb-2', streamingMessageId: 'new-id',
        streamingContent: 'new text', pendingMessageIds: { 'new-id': true } });
      if (outcome === 'resolve') old.resolve('old-id');
      else old.reject(new Error('old send failed'));
      await sending;
      expect(useChatStore.getState().streamingMessageId).toBe('new-id');
      expect(useChatStore.getState().streamingContent).toBe('new text');
      expect(useChatStore.getState().isStreaming).toBe(true);
      expect(useChatStore.getState().streamingError).toBeNull();
    });
  }

  it('does not send after first-conversation creation crosses a notebook switch', async () => {
    const created = deferred<string>();
    useChatStore.setState({ activeConversationId: null });
    vi.mocked(api.createConversation).mockReturnValueOnce(created.promise);
    const sending = useChatStore.getState().sendMessage('nb-1', 'private draft', { kind: 'none' }, 'model');
    useNotebookStore.setState({ activeNotebookId: 'nb-2' });
    useChatStore.getState().resetForNotebookSwitch();
    created.resolve('old-conversation');
    await sending;
    expect(api.sendMessage).not.toHaveBeenCalled();
    expect(useChatStore.getState().messages).toEqual([]);
    expect(useChatStore.getState().isStreaming).toBe(false);
  });

  it('reserves a single send before conversation creation awaits', async () => {
    const created = deferred<string>();
    useChatStore.setState({ activeConversationId: null });
    vi.mocked(api.createConversation).mockReturnValue(created.promise);
    vi.mocked(api.sendMessage).mockImplementation(async (...args) => args[5]!);
    const first = useChatStore.getState().sendMessage('nb-1', 'one', { kind: 'none' }, 'model');
    const second = useChatStore.getState().sendMessage('nb-1', 'two', { kind: 'none' }, 'model');
    created.resolve('conv-new');
    await Promise.all([first, second]);
    expect(api.createConversation).toHaveBeenCalledTimes(1);
    expect(api.sendMessage).toHaveBeenCalledTimes(1);
  });

  it('send acknowledgement after done cannot resurrect accepted IDs', async () => {
    const ack = deferred<string>();
    vi.mocked(api.sendMessage).mockReturnValueOnce(ack.promise);
    const sending = useChatStore.getState().sendMessage('nb-1', 'one', { kind: 'none' }, 'model');
    const id = useChatStore.getState().streamingMessageId!;
    await useChatStore.getState().finalizeMessage('nb-1', 'conv-1', id);
    ack.resolve(id);
    await sending;
    expect(useChatStore.getState().pendingMessageIds).toEqual({});
  });

  it('notes use active notebook state, not a stale localStorage projection', async () => {
    const pending = deferred<Note[]>();
    vi.mocked(api.listNotes).mockReturnValueOnce(pending.promise);
    const load = useNoteStore.getState().loadNotes('nb-1');
    useNotebookStore.setState({ activeNotebookId: 'nb-2' });
    useNoteStore.getState().resetForNotebookSwitch();
    pending.resolve([{ id: 'old' } as Note]);
    await load;
    expect(useNoteStore.getState().notes).toEqual([]);
  });

  it('notes keep the newest same-notebook load', async () => {
    const older = deferred<Note[]>();
    vi.mocked(api.listNotes).mockReturnValueOnce(older.promise).mockResolvedValueOnce([{ id: 'new' } as Note]);
    const first = useNoteStore.getState().loadNotes('nb-1');
    await useNoteStore.getState().loadNotes('nb-1');
    older.resolve([{ id: 'old' } as Note]);
    await first;
    expect(useNoteStore.getState().notes.map(n => n.id)).toEqual(['new']);
  });

  it('inactive note refresh cannot leave a spinner behind', async () => {
    vi.mocked(api.listNotes).mockResolvedValueOnce([]);
    useNotebookStore.setState({ activeNotebookId: 'nb-2' });
    await useNoteStore.getState().loadNotes('nb-1');
    expect(useNoteStore.getState().loading).toBe(false);
    expect(api.listNotes).not.toHaveBeenCalled();
  });

  it('source reload preserves a selection made after it started', async () => {
    const source = { id: 'src', selected: true } as Source;
    const pending = deferred<Source[]>();
    useSourceStore.setState({ sources: [source], selectedSourceIds: new Set(['src']), sourceScopeMode: 'all', loadedNotebookId: 'nb-1' });
    vi.mocked(api.listSources).mockReturnValueOnce(pending.promise);
    const load = useSourceStore.getState().loadSources('nb-1');
    useSourceStore.getState().selectNone();
    pending.resolve([source]);
    await load;
    expect(useSourceStore.getState().getSourceScope()).toEqual({ kind: 'none' });
  });

  it('model readiness does not cross provider identity for duplicate model names', async () => {
    useSettingsStore.setState({ activeModel: 'shared', settings: { default_provider: 'ollama' } });
    vi.mocked(api.getAllModels).mockResolvedValueOnce([{ id: 'shared', provider_id: 'openai', display_name: 'Shared', available: true, stale: false }]);
    await useSettingsStore.getState().loadModels();
    expect(useSettingsStore.getState().selectionError).toContain('unavailable');
  });
});
