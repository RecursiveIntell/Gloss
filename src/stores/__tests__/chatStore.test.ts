import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useChatStore } from '../chatStore';
import { useNotebookStore } from '../notebookStore';
import * as api from '../../lib/tauri';

// Mock the Tauri API layer so we never need a running backend
vi.mock('../../lib/tauri', () => ({
  createConversation: vi.fn().mockResolvedValue('conv-1'),
  listConversations: vi.fn().mockResolvedValue([]),
  loadMessages: vi.fn().mockResolvedValue([]),
  getChatEventsSince: vi.fn().mockResolvedValue([]),
  sendMessage: vi.fn().mockResolvedValue('msg-1'),
  stopChat: vi.fn().mockResolvedValue({ cancellation_requested: true, attempts: [] }),
  getSuggestedQuestions: vi.fn().mockResolvedValue([]),
  deleteConversation: vi.fn().mockResolvedValue(undefined),
}));

// Mock localStorage so the notebook-switch guard doesn't short-circuit
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

describe('chatStore', () => {
  beforeEach(() => {
    // Reset store to a clean baseline
    useChatStore.setState({
      messages: [],
      streamingContent: '',
      isStreaming: false,
      streamingNotebookId: null,
      streamingMessageId: null,
      streamingError: null,
      streamingStatus: null,
      pendingEvidence: {},
      conversations: [],
      activeConversationId: null,
      suggestedQuestions: [],
    });
    useNotebookStore.setState({
      activeNotebookId: 'nb-1',
      activationStatus: 'confirmed',
    });
    // Default: active notebook matches so guards pass
    localStorageMock.clear();
    localStorageMock.setItem('gloss:activeNotebookId', 'nb-1');
  });

  it('terminal event without tokens loads persisted assistant', async () => {
    const store = useChatStore.getState();
    const msgId = 'msg-assistant-1';
    vi.mocked(api.loadMessages).mockResolvedValueOnce([
      {
        id: 'user-1',
        conversation_id: 'conv-1',
        role: 'user' as const,
        content: 'question',
        created_at: '2026-06-12T00:00:00Z',
      },
      {
        id: msgId,
        conversation_id: 'conv-1',
        role: 'assistant' as const,
        content: 'persisted answer',
        created_at: '2026-06-12T00:00:01Z',
      },
    ]);

    // Put the store mid-stream with empty content
    useChatStore.setState({
      isStreaming: true,
      streamingContent: '',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
      activeConversationId: 'conv-1',
    });

    await store.finalizeMessage('nb-1', 'conv-1', msgId);

    const after = useChatStore.getState();
    expect(after.messages.map((message) => message.id)).toEqual(['user-1', msgId]);
    expect(after.messages[1].content).toBe('persisted answer');
    expect(after.isStreaming).toBe(false);
    expect(after.streamingError).toBeNull();
    expect(after.streamingContent).toBe('');
  });

  it('duplicate terminal does not duplicate persisted assistant message', async () => {
    const msgId = 'msg-assistant-dupe';
    const persisted = [
      {
        id: 'user-dupe',
        conversation_id: 'conv-1',
        role: 'user' as const,
        content: 'question',
        created_at: '2026-06-12T00:00:00Z',
      },
      {
        id: msgId,
        conversation_id: 'conv-1',
        role: 'assistant' as const,
        content: 'persisted once',
        created_at: '2026-06-12T00:00:01Z',
      },
    ];
    vi.mocked(api.loadMessages)
      .mockResolvedValueOnce(persisted)
      .mockResolvedValueOnce(persisted);

    useChatStore.setState({
      isStreaming: true,
      streamingContent: 'persisted once',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
      activeConversationId: 'conv-1',
    });

    await useChatStore.getState().finalizeMessage('nb-1', 'conv-1', msgId);
    await useChatStore.getState().finalizeMessage('nb-1', 'conv-1', msgId);

    const assistantMessages = useChatStore
      .getState()
      .messages
      .filter((message) => message.role === 'assistant' && message.id === msgId);
    expect(assistantMessages).toHaveLength(1);
  });

  it('rehydrateConversation replaces optimistic streamed state with DB truth idempotently', async () => {
    const persisted = [
      {
        id: 'user-rehydrate',
        conversation_id: 'conv-1',
        role: 'user' as const,
        content: 'question',
        created_at: '2026-06-12T00:00:00Z',
      },
      {
        id: 'assistant-rehydrate',
        conversation_id: 'conv-1',
        role: 'assistant' as const,
        content: 'authoritative persisted answer',
        created_at: '2026-06-12T00:00:01Z',
      },
    ];
    vi.mocked(api.loadMessages)
      .mockResolvedValueOnce(persisted)
      .mockResolvedValueOnce(persisted);

    useChatStore.setState({
      activeConversationId: 'conv-1',
      messages: [
        persisted[0],
        {
          id: 'assistant-rehydrate',
          conversation_id: 'conv-1',
          role: 'assistant',
          content: 'optimistic stale content',
          created_at: '2026-06-12T00:00:01Z',
        },
      ],
      isStreaming: true,
      streamingContent: 'optimistic stale content',
      streamingNotebookId: 'nb-1',
      streamingMessageId: 'assistant-rehydrate',
      pendingMessageIds: { 'assistant-rehydrate': true },
    });

    await useChatStore.getState().rehydrateConversation('nb-1', 'conv-1');
    await useChatStore.getState().rehydrateConversation('nb-1', 'conv-1');

    const after = useChatStore.getState();
    expect(after.messages).toEqual(persisted);
    expect(after.messages.filter((message) => message.id === 'assistant-rehydrate')).toHaveLength(1);
  });

  it('replayed done after listener loss finalizes from DB once', async () => {
    const msgId = 'assistant-replay-done';
    const persisted = [
      {
        id: 'user-replay',
        conversation_id: 'conv-1',
        role: 'user' as const,
        content: 'question',
        created_at: '2026-06-12T00:00:00Z',
      },
      {
        id: msgId,
        conversation_id: 'conv-1',
        role: 'assistant' as const,
        content: 'answer after remount',
        created_at: '2026-06-12T00:00:01Z',
      },
    ];
    vi.mocked(api.getChatEventsSince).mockResolvedValueOnce([
      {
        seq: 9,
        attempt_id: msgId,
        kind: 'done',
        notebook_id: 'nb-1',
        conversation_id: 'conv-1',
        message_id: msgId,
        recorded_at: '2026-06-12T00:00:02Z',
        payload: {
          notebook_id: 'nb-1',
          conversation_id: 'conv-1',
          message_id: msgId,
        },
      },
    ]);
    vi.mocked(api.loadMessages).mockResolvedValueOnce(persisted);

    useChatStore.setState({
      isStreaming: true,
      streamingContent: '',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
      pendingMessageIds: { [msgId]: true },
      activeConversationId: 'conv-1',
    });

    await useChatStore.getState().replayChatEvents('nb-1', 'conv-1');

    const after = useChatStore.getState();
    expect(after.lastChatEventSeq).toBe(9);
    expect(after.isStreaming).toBe(false);
    expect(after.messages).toEqual(persisted);
  });

  it('stopStreaming keeps the stream pending until chat:cancelled is observed', async () => {
    const msgId = 'msg-assistant-2';

    useChatStore.setState({
      isStreaming: true,
      streamingContent: 'partial...',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
      pendingMessageIds: { [msgId]: true },
      activeConversationId: 'conv-1',
    });

    await useChatStore.getState().stopStreaming('nb-1');

    const afterAcknowledgement = useChatStore.getState();
    expect(afterAcknowledgement.isStreaming).toBe(true);
    expect(afterAcknowledgement.streamingError).toBeNull();
    expect(afterAcknowledgement.streamingNotebookId).toBe('nb-1');
    expect(afterAcknowledgement.streamingMessageId).toBe(msgId);
    expect(afterAcknowledgement.streamingStatus?.phase).toBe('cancelling');

    useChatStore.getState().handleChatCancelled('nb-1', 'conv-1', msgId, 'Cancelled by user');

    const afterTerminalEvent = useChatStore.getState();
    expect(afterTerminalEvent.isStreaming).toBe(false);
    expect(afterTerminalEvent.streamingError).toBe('Cancelled by user');
    expect(afterTerminalEvent.streamingNotebookId).toBeNull();
    expect(afterTerminalEvent.streamingMessageId).toBeNull();
  });

  it('stopStreaming reports a cancellation request failure as an error terminal state', async () => {
    const msgId = 'msg-cancel-request-failure';
    vi.mocked(api.stopChat).mockRejectedValueOnce(new Error('backend unavailable'));
    useChatStore.setState({
      isStreaming: true,
      streamingContent: 'partial...',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
      pendingMessageIds: { [msgId]: true },
      activeConversationId: 'conv-1',
    });

    await useChatStore.getState().stopStreaming('nb-1');

    const after = useChatStore.getState();
    expect(after.isStreaming).toBe(false);
    expect(after.streamingError).toBe('Cancellation request failed: backend unavailable');
    expect(after.streamingNotebookId).toBeNull();
    expect(after.streamingMessageId).toBeNull();
  });

  it('appendToken (onChatToken) accumulates streaming content', () => {
    const msgId = 'msg-assistant-3';

    useChatStore.setState({
      isStreaming: true,
      streamingContent: '',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
    });

    const store = useChatStore.getState();
    store.appendToken('nb-1', 'conv-1', msgId, 'Hello ');
    store.appendToken('nb-1', 'conv-1', msgId, 'world!');

    const after = useChatStore.getState();
    expect(after.streamingContent).toBe('Hello world!');
  });

  it('setStreamingError sets streamingError', () => {
    const msgId = 'msg-assistant-4';

    useChatStore.setState({
      isStreaming: true,
      streamingContent: '',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
    });

    useChatStore.getState().setStreamingError('nb-1', 'conv-1', msgId, 'Provider timed out');

    const after = useChatStore.getState();
    expect(after.streamingError).toBe('Provider timed out');
    expect(after.isStreaming).toBe(false);
  });

  it('replays typed slow retrieval status for visible diagnostics', async () => {
    const msgId = 'msg-status-typed';
    vi.mocked(api.getChatEventsSince).mockResolvedValueOnce([
      {
        seq: 7,
        attempt_id: msgId,
        kind: 'status',
        notebook_id: 'nb-1',
        conversation_id: 'conv-1',
        message_id: msgId,
        recorded_at: '2026-06-12T00:00:00Z',
        payload: {
          notebook_id: 'nb-1',
          conversation_id: 'conv-1',
          message_id: msgId,
          phase: 'semantic_memory_search_timeout',
          message: 'semantic-memory preview timed out',
          provider: 'ollama',
          model: 'llama3',
          owner: 'semantic-memory',
          owner_detail: 'bge-m3',
          reason_code: 'semantic_memory_timeout',
          elapsed_ms: 8000,
          timeout_ms: 8000,
          truncated: false,
          error: 'search-timeout',
        },
      },
    ]);

    useChatStore.setState({
      isStreaming: true,
      streamingContent: '',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
      pendingMessageIds: { [msgId]: true },
      activeConversationId: 'conv-1',
    });

    await useChatStore.getState().replayChatEvents('nb-1', 'conv-1');

    expect(useChatStore.getState().streamingStatus?.reason_code).toBe('semantic_memory_timeout');
    expect(useChatStore.getState().streamingStatus?.timeout_ms).toBe(8000);
  });

  it('does not apply a late replay response from conversation A to conversation B', async () => {
    let resolveA!: (events: never[]) => void;
    vi.mocked(api.getChatEventsSince).mockImplementationOnce(() => new Promise((resolve) => { resolveA = resolve; }));
    useChatStore.setState({ activeConversationId: 'conv-a', isStreaming: true, streamingNotebookId: 'nb-1', streamingMessageId: 'msg-a', pendingMessageIds: { 'msg-a': true } });
    const replayA = useChatStore.getState().replayChatEvents('nb-1', 'conv-a');
    useChatStore.setState({ activeConversationId: 'conv-b', messages: [], isStreaming: true, streamingNotebookId: 'nb-1', streamingMessageId: 'msg-b', pendingMessageIds: { 'msg-b': true } });
    resolveA([{ seq: 99, attempt_id: 'msg-a', kind: 'token', notebook_id: 'nb-1', conversation_id: 'conv-a', message_id: 'msg-a', recorded_at: 'now', payload: { token: 'wrong conversation' } }] as never[]);
    await replayA;
    expect(useChatStore.getState().messages).toEqual([]);
    expect(useChatStore.getState().replayCursors['nb-1:conv-b']).toBeUndefined();
  });

  it('createConversation updates store state', async () => {
    await useChatStore.getState().createConversation('nb-1');

    const after = useChatStore.getState();
    expect(after.activeConversationId).toBe('conv-1');
    expect(after.messages).toEqual([]);
  });

  it('sendMessage reports first-conversation creation failure without throwing', async () => {
    vi.mocked(api.createConversation).mockRejectedValueOnce(new Error('conversation db unavailable'));

    await expect(
      useChatStore.getState().sendMessage('nb-1', 'preserve this prompt', { kind: 'none' }, 'model-1')
    ).resolves.toBeUndefined();

    const after = useChatStore.getState();
    expect(after.streamingError).toContain('conversation db unavailable');
    expect(after.isStreaming).toBe(false);
    expect(after.messages).toEqual([]);
  });

  it('does not append a completed stream from a previous notebook into the active notebook', async () => {
    const msgId = 'old-stream-msg';
    useChatStore.setState({
      isStreaming: true,
      streamingContent: 'old notebook answer',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
      activeConversationId: 'conv-1',
    });

    useChatStore.getState().resetForNotebookSwitch();
    localStorageMock.setItem('gloss:activeNotebookId', 'nb-2');
    await useChatStore.getState().finalizeMessage('nb-1', 'conv-1', msgId);

    const after = useChatStore.getState();
    expect(after.isStreaming).toBe(false);
    expect(after.messages).toEqual([]);
    expect(after.streamingError).toBeNull();
  });
});
