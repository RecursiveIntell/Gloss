import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useChatStore } from '../chatStore';
import { useNotebookStore } from '../notebookStore';
import * as api from '../../lib/tauri';
import type { Message } from '../../lib/types';

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

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise; });
  return { promise, resolve };
}

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

  it('older same-conversation hydration cannot erase a newer completed assistant', async () => {
    const roles = ['user', 'assistant', 'user', 'user', 'assistant', 'user', 'assistant', 'user'] as const;
    const beforeCompletion: Message[] = roles.map((role, index) => ({
      id: `saved-row-${index + 1}`,
      conversation_id: 'conv-1',
      role,
      content: `Saved content ${index + 1}`,
      created_at: `2026-09-06T00:00:0${index + 1}Z`,
    }));
    const completed: Message = {
      id: 'newly-completed-assistant', conversation_id: 'conv-1', role: 'assistant',
      content: 'The Atlas launch code is GLACIER-ORBIT-417.', created_at: '2026-09-06T00:00:09Z',
    };
    const afterCompletion = [...beforeCompletion, completed];
    const older = deferred<Message[]>();
    const newer = deferred<Message[]>();
    vi.mocked(api.loadMessages)
      .mockImplementationOnce(() => older.promise)
      .mockImplementationOnce(() => newer.promise);
    useChatStore.setState({
      activeConversationId: 'conv-1', messages: beforeCompletion,
      isStreaming: true, streamingNotebookId: 'nb-1',
      streamingMessageId: completed.id, pendingMessageIds: { [completed.id]: true },
      streamingContent: completed.content,
    });

    const olderLoad = useChatStore.getState().rehydrateConversation('nb-1', 'conv-1');
    const completion = useChatStore.getState().finalizeMessage('nb-1', 'conv-1', completed.id);
    newer.resolve(afterCompletion);
    await completion;
    expect(useChatStore.getState().messages).toEqual(afterCompletion);
    expect(useChatStore.getState().isStreaming).toBe(false);

    // The earlier database snapshot reaches the same active conversation last.
    older.resolve(beforeCompletion);
    await olderLoad;
    const afterOlderLoad = useChatStore.getState();
    expect(afterOlderLoad.messages).toEqual(afterCompletion);
    expect(afterOlderLoad.messages[afterOlderLoad.messages.length - 1]?.id).toBe(completed.id);
  });

  it('hydration started before a new send cannot erase its optimistic user or stream', async () => {
    const saved: Message[] = [{
      id: 'prior-user', conversation_id: 'conv-1', role: 'user',
      content: 'Previous question', created_at: '2026-09-06T00:00:01Z',
    }, {
      id: 'prior-assistant', conversation_id: 'conv-1', role: 'assistant',
      content: 'Previous answer', created_at: '2026-09-06T00:00:02Z',
    }];
    const oldSnapshot = deferred<Message[]>();
    const acknowledgement = deferred<string>();
    vi.mocked(api.loadMessages).mockImplementationOnce(() => oldSnapshot.promise);
    vi.mocked(api.sendMessage).mockImplementationOnce(() => acknowledgement.promise);
    useChatStore.setState({
      activeConversationId: 'conv-1', messages: saved,
      isStreaming: false, streamingNotebookId: null, streamingMessageId: null,
      preparingMessageId: null, pendingMessageIds: {}, streamingContent: '',
    });

    const hydration = useChatStore.getState().rehydrateConversation('nb-1', 'conv-1');
    const send = useChatStore.getState().sendMessage('nb-1', 'Current question', { kind: 'none' }, 'model-1');
    const current = useChatStore.getState();
    const assistantId = current.streamingMessageId!;
    const optimisticUser = current.messages[current.messages.length - 1]!;
    expect(assistantId).toBeTruthy();
    expect(optimisticUser.content).toBe('Current question');
    useChatStore.getState().appendToken('nb-1', 'conv-1', assistantId, 'Current response token');
    acknowledgement.resolve(assistantId);
    await send;

    oldSnapshot.resolve(saved);
    await hydration;
    const after = useChatStore.getState();
    expect(after.isStreaming).toBe(true);
    expect(after.streamingMessageId).toBe(assistantId);
    expect(after.pendingMessageIds[assistantId]).toBe(true);
    expect(after.streamingContent).toBe('Current response token');
    expect(after.messages).toEqual([...saved, optimisticUser]);
  });

  it('conversation A to B to A rejects the old A load and accepts a fresh A hydration', async () => {
    const currentA: Message[] = [{
      id: 'current-a-user', conversation_id: 'conv-a', role: 'user',
      content: 'Current A question', created_at: '2026-09-06T00:00:01Z',
    }, {
      id: 'current-a-assistant', conversation_id: 'conv-a', role: 'assistant',
      content: 'Current A answer', created_at: '2026-09-06T00:00:02Z',
    }];
    const staleA = currentA.slice(0, 1);
    const freshA: Message[] = [...currentA, {
      id: 'new-a-user', conversation_id: 'conv-a', role: 'user',
      content: 'New canonical A question', created_at: '2026-09-06T00:00:03Z',
    }];
    const older = deferred<Message[]>();
    const fresh = deferred<Message[]>();
    vi.mocked(api.loadMessages)
      .mockImplementationOnce(() => older.promise)
      .mockImplementationOnce(() => fresh.promise);
    useChatStore.setState({
      activeConversationId: 'conv-a', messages: currentA,
      isStreaming: false, streamingNotebookId: null, streamingMessageId: null,
      preparingMessageId: null, pendingMessageIds: {}, streamingContent: '',
    });

    const oldLoad = useChatStore.getState().rehydrateConversation('nb-1', 'conv-a');
    useChatStore.getState().setActiveConversation('conv-b');
    useChatStore.getState().setActiveConversation('conv-a');
    older.resolve(staleA);
    await oldLoad;
    expect(useChatStore.getState().activeConversationId).toBe('conv-a');
    expect(useChatStore.getState().messages).toEqual(currentA);

    const freshLoad = useChatStore.getState().rehydrateConversation('nb-1', 'conv-a');
    fresh.resolve(freshA);
    await freshLoad;
    expect(useChatStore.getState().messages).toEqual(freshA);
  });

  it('hydration started after send reservation preserves the optimistic user before acknowledgement', async () => {
    const saved: Message[] = [{
      id: 'prior-user', conversation_id: 'conv-1', role: 'user',
      content: 'Previous question', created_at: '2026-09-06T00:00:01Z',
    }];
    const snapshot = deferred<Message[]>();
    const acknowledgement = deferred<string>();
    vi.mocked(api.loadMessages).mockImplementationOnce(() => snapshot.promise);
    vi.mocked(api.sendMessage).mockImplementationOnce(() => acknowledgement.promise);
    useChatStore.setState({
      activeConversationId: 'conv-1', messages: saved,
      isStreaming: false, streamingNotebookId: null, streamingMessageId: null,
      preparingMessageId: null, pendingMessageIds: {}, streamingContent: '',
    });

    const send = useChatStore.getState().sendMessage('nb-1', 'Current question', { kind: 'none' }, 'model-1');
    const current = useChatStore.getState();
    const assistantId = current.streamingMessageId!;
    const optimisticUser = current.messages[current.messages.length - 1]!;
    useChatStore.getState().appendToken('nb-1', 'conv-1', assistantId, 'Current response token');
    const hydration = useChatStore.getState().rehydrateConversation('nb-1', 'conv-1');
    snapshot.resolve(saved);
    await hydration;
    const beforeAcknowledgement = useChatStore.getState();
    acknowledgement.resolve(assistantId);
    await send;

    expect(beforeAcknowledgement.messages).toEqual([...saved, optimisticUser]);
    expect(beforeAcknowledgement.isStreaming).toBe(true);
    expect(beforeAcknowledgement.streamingMessageId).toBe(assistantId);
    expect(beforeAcknowledgement.pendingMessageIds[assistantId]).toBe(true);
    expect(beforeAcknowledgement.streamingContent).toBe('Current response token');
  });

  it('final focus hydration recovers with a fresh canonical read after an owned send rejection', async () => {
    const saved: Message[] = [{
      id: 'known-user', conversation_id: 'conv-1', role: 'user',
      content: 'Known question', created_at: '2026-09-06T00:00:01Z',
    }];
    const recovered: Message[] = [...saved, {
      id: 'canonical-answer', conversation_id: 'conv-1', role: 'assistant',
      content: 'Saved answer discovered on focus', created_at: '2026-09-06T00:00:02Z',
    }];
    const focusSnapshot = deferred<Message[]>();
    const recoverySnapshot = deferred<Message[]>();
    let reads = 0;
    vi.mocked(api.loadMessages).mockClear().mockImplementation(() => (
      ++reads === 1 ? focusSnapshot.promise : recoverySnapshot.promise
    ));
    vi.mocked(api.sendMessage).mockRejectedValueOnce(new Error('IPC rejected before attempt'));
    useChatStore.setState({
      activeConversationId: 'conv-1', messages: saved,
      isStreaming: false, streamingNotebookId: null, streamingMessageId: null,
      preparingMessageId: null, pendingMessageIds: {}, streamingContent: '',
    });

    try {
      // App's final focus hydration has no later hydration continuation.
      await useChatStore.getState().replayChatEvents('nb-1', 'conv-1');
      const focusHydration = useChatStore.getState().rehydrateConversation('nb-1', 'conv-1');
      await useChatStore.getState().sendMessage('nb-1', 'Next question', { kind: 'none' }, 'model-1');
      expect(api.loadMessages).toHaveBeenCalledTimes(2);
      expect(useChatStore.getState().isStreaming).toBe(false);
      expect(useChatStore.getState().streamingError).toBe('IPC rejected before attempt');
      expect(useChatStore.getState().messages).toEqual(saved);

      focusSnapshot.resolve(recovered);
      await focusHydration;
      // The invalidated focus result cannot stand in for the fresh recovery read.
      expect(useChatStore.getState().messages).toEqual(saved);
      recoverySnapshot.resolve(recovered);
      await recoverySnapshot.promise;
      await Promise.resolve();
      expect(useChatStore.getState().messages).toEqual(recovered);
      expect(useChatStore.getState().streamingError).toBe('IPC rejected before attempt');
      expect(api.loadMessages).toHaveBeenCalledTimes(2);
    } finally {
      focusSnapshot.resolve(saved);
      recoverySnapshot.resolve(recovered);
      await Promise.resolve();
      vi.mocked(api.loadMessages).mockReset().mockResolvedValue([]);
    }
  });

  it.each(['newer send', 'conversation round trip'] as const)(
    'send rejection recovery cannot commit after a %s', async (invalidation) => {
      const saved: Message[] = [{
        id: 'known-user', conversation_id: 'conv-1', role: 'user',
        content: 'Known question', created_at: '2026-09-06T00:00:01Z',
      }];
      const recovered: Message[] = [...saved, {
        id: 'older-canonical-answer', conversation_id: 'conv-1', role: 'assistant',
        content: 'Answer from the invalidated recovery', created_at: '2026-09-06T00:00:02Z',
      }];
      const focusSnapshot = deferred<Message[]>();
      const recoverySnapshot = deferred<Message[]>();
      const acknowledgement = deferred<string>();
      let reads = 0;
      vi.mocked(api.loadMessages).mockClear().mockImplementation(() => (
        ++reads === 1 ? focusSnapshot.promise : recoverySnapshot.promise
      ));
      vi.mocked(api.sendMessage).mockRejectedValueOnce(new Error('IPC rejected before attempt'));
      useChatStore.setState({
        activeConversationId: 'conv-1', messages: saved,
        isStreaming: false, streamingNotebookId: null, streamingMessageId: null,
        preparingMessageId: null, pendingMessageIds: {}, streamingContent: '',
      });
      let newerSend: Promise<void> | undefined;
      let assistantId: string | null = null;

      try {
        const focusHydration = useChatStore.getState().rehydrateConversation('nb-1', 'conv-1');
        await useChatStore.getState().sendMessage('nb-1', 'Rejected question', { kind: 'none' }, 'model-1');
        expect(api.loadMessages).toHaveBeenCalledTimes(2);
        if (invalidation === 'newer send') {
          vi.mocked(api.sendMessage).mockImplementationOnce(() => acknowledgement.promise);
          newerSend = useChatStore.getState().sendMessage('nb-1', 'Current question', { kind: 'none' }, 'model-1');
          assistantId = useChatStore.getState().streamingMessageId;
          useChatStore.getState().appendToken('nb-1', 'conv-1', assistantId!, 'Current response token');
        } else {
          useChatStore.getState().setActiveConversation('conv-2');
          useChatStore.getState().setActiveConversation('conv-1');
        }
        const current = useChatStore.getState();

        focusSnapshot.resolve(recovered);
        await focusHydration;
        recoverySnapshot.resolve(recovered);
        await recoverySnapshot.promise;
        await Promise.resolve();
        const after = useChatStore.getState();
        expect(after.messages).toEqual(current.messages);
        expect(after.activeConversationId).toBe('conv-1');
        expect(after.isStreaming).toBe(current.isStreaming);
        expect(after.streamingMessageId).toBe(current.streamingMessageId);
        expect(after.streamingContent).toBe(current.streamingContent);
        expect(after.streamingError).toBe(current.streamingError);
        expect(api.loadMessages).toHaveBeenCalledTimes(2);
      } finally {
        focusSnapshot.resolve(saved);
        recoverySnapshot.resolve(recovered);
        acknowledgement.resolve(assistantId ?? 'unused-acknowledgement');
        await newerSend;
        await Promise.resolve();
        vi.mocked(api.loadMessages).mockReset().mockResolvedValue([]);
      }
    }
  );

  it('loading conversation B is not blocked by an active stream and displayed users from conversation A', async () => {
    const conversationA: Message[] = [{
      id: 'a-user', conversation_id: 'conv-a', role: 'user',
      content: 'Question in A', created_at: '2026-09-06T00:00:01Z',
    }];
    const conversationB: Message[] = [{
      id: 'b-user', conversation_id: 'conv-b', role: 'user',
      content: 'Question in B', created_at: '2026-09-06T00:00:02Z',
    }];
    const snapshot = deferred<Message[]>();
    vi.mocked(api.loadMessages).mockImplementationOnce(() => snapshot.promise);
    useChatStore.setState({
      activeConversationId: 'conv-a', messages: conversationA,
      isStreaming: true, streamingNotebookId: 'nb-1', streamingMessageId: 'a-assistant',
      preparingMessageId: null, pendingMessageIds: { 'a-assistant': true }, streamingContent: 'A response',
    });

    const loadB = useChatStore.getState().loadMessages('nb-1', 'conv-b');
    snapshot.resolve(conversationB);
    await loadB;
    expect(useChatStore.getState().activeConversationId).toBe('conv-b');
    expect(useChatStore.getState().messages).toEqual(conversationB);
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
