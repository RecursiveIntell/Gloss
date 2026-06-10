import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useChatStore } from '../chatStore';
import * as api from '../../lib/tauri';

// Mock the Tauri API layer so we never need a running backend
vi.mock('../../lib/tauri', () => ({
  createConversation: vi.fn().mockResolvedValue('conv-1'),
  listConversations: vi.fn().mockResolvedValue([]),
  loadMessages: vi.fn().mockResolvedValue([]),
  sendMessage: vi.fn().mockResolvedValue('msg-1'),
  stopChat: vi.fn().mockResolvedValue(undefined),
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
    // Default: active notebook matches so guards pass
    localStorageMock.clear();
    localStorageMock.setItem('gloss:activeNotebookId', 'nb-1');
  });

  it('finalizeMessage bails on empty streamingContent', () => {
    const store = useChatStore.getState();
    const msgId = 'msg-assistant-1';

    // Put the store mid-stream with empty content
    useChatStore.setState({
      isStreaming: true,
      streamingContent: '',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
    });

    store.finalizeMessage('nb-1', 'conv-1', msgId);

    // No assistant message should be added
    const after = useChatStore.getState();
    expect(after.messages).toHaveLength(0);
    expect(after.isStreaming).toBe(false);
    expect(after.streamingError).toBe('Chat completed without response content.');
    expect(after.streamingContent).toBe('');
  });

  it('stopStreaming sets isStreaming=false and streamingError', async () => {
    const msgId = 'msg-assistant-2';

    useChatStore.setState({
      isStreaming: true,
      streamingContent: 'partial...',
      streamingNotebookId: 'nb-1',
      streamingMessageId: msgId,
    });

    await useChatStore.getState().stopStreaming('nb-1');

    const after = useChatStore.getState();
    expect(after.isStreaming).toBe(false);
    // Error should be set because partial output wasn't finalized as a message
    expect(after.streamingError).toBeTruthy();
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

  it('does not append a completed stream from a previous notebook into the active notebook', () => {
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
    useChatStore.getState().finalizeMessage('nb-1', 'conv-1', msgId);

    const after = useChatStore.getState();
    expect(after.isStreaming).toBe(false);
    expect(after.messages).toEqual([]);
    expect(after.streamingError).toBeNull();
  });
});
