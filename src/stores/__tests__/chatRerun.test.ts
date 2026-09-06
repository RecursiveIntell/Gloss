import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useChatStore } from '../chatStore';
import { useNotebookStore } from '../notebookStore';
import { sendMessage } from '../../lib/tauri';
import type { Message } from '../../lib/types';

// Keep the real store and Tauri wrapper; observe the submitted IPC contract.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const priorUser: Message = {
  id: '11111111-1111-4111-8111-111111111111', conversation_id: 'conv-1',
  role: 'user', content: 'Earlier question', created_at: '2026-09-05T00:00:00Z',
};
const priorAnswer: Message = {
  id: '22222222-2222-4222-8222-222222222222', conversation_id: 'conv-1',
  role: 'assistant', content: 'Saved answer', created_at: '2026-09-05T00:00:01Z',
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(invoke).mockImplementation(async (_command, args) =>
    (args as { messageId?: string } | undefined)?.messageId);
  useNotebookStore.setState({ activeNotebookId: 'nb-1', activationStatus: 'confirmed' });
  useChatStore.setState({
    activeConversationId: 'conv-1', messages: [priorUser, priorAnswer],
    isStreaming: false, streamingContent: '', streamingNotebookId: null,
    streamingMessageId: null, preparingMessageId: null, streamingError: null,
    streamingStatus: null, pendingMessageIds: {}, pendingEvidence: {},
    style: 'default', customGoal: '', responseLength: 'default',
  });
});

describe('rerun request identity', () => {
  it('submits the exact visible user ID and can target it after cancellation without reloading', async () => {
    await useChatStore.getState().sendMessage('nb-1', 'Long question', { kind: 'none' }, 'model');
    const first = useChatStore.getState();
    const cancelledUser = first.messages[2];
    const firstAssistantId = first.streamingMessageId!;
    expect(invoke).toHaveBeenLastCalledWith('send_message', expect.objectContaining({
      messageId: firstAssistantId, userMessageId: cancelledUser.id,
      historyBeforeUserMessageId: undefined,
    }));
    expect(cancelledUser.id).not.toBe(firstAssistantId);
    expect(first.isStreaming).toBe(true);

    first.handleChatCancelled('nb-1', 'conv-1', firstAssistantId, 'Chat cancelled by user');
    expect(useChatStore.getState().messages).toEqual([priorUser, priorAnswer, cancelledUser]);

    await useChatStore.getState().sendMessage('nb-1', 'Replacement question', { kind: 'none' }, 'model', cancelledUser.id);
    const rerun = useChatStore.getState();
    const replacementUser = rerun.messages[3];
    expect(invoke).toHaveBeenLastCalledWith('send_message', {
      notebookId: 'nb-1', conversationId: 'conv-1', query: 'Replacement question',
      sourceScope: { kind: 'none' }, model: 'model', messageId: rerun.streamingMessageId,
      style: undefined, customGoal: undefined, responseLength: undefined,
      historyBeforeUserMessageId: cancelledUser.id, userMessageId: replacementUser.id,
    });
    expect(replacementUser.id).not.toBe(cancelledUser.id);
    expect(rerun.messages).toEqual([priorUser, priorAnswer, cancelledUser, replacementUser]);
    expect(rerun.isStreaming).toBe(true);
    expect(rerun.streamingError).toBeNull();
    expect(rerun.pendingMessageIds).toEqual({ [rerun.streamingMessageId!]: true });
  });

  it('leaves ordinary sends unanchored while preserving source and generation settings', async () => {
    useChatStore.setState({ style: 'custom', customGoal: 'Be concise', responseLength: 'short' });
    await useChatStore.getState().sendMessage('nb-1', 'New question', { kind: 'explicit', ids: ['source-1'] }, 'chosen-model');
    const state = useChatStore.getState();
    expect(invoke).toHaveBeenLastCalledWith('send_message', {
      notebookId: 'nb-1', conversationId: 'conv-1', query: 'New question',
      sourceScope: { kind: 'explicit', ids: ['source-1'] }, model: 'chosen-model',
      messageId: state.streamingMessageId, style: 'custom', customGoal: 'Be concise',
      responseLength: 'short', historyBeforeUserMessageId: undefined,
      userMessageId: state.messages[2].id,
    });
    expect(state.messages.slice(0, 2)).toEqual([priorUser, priorAnswer]);
    expect(state.isStreaming).toBe(true);
    expect(state.streamingError).toBeNull();
  });

  it('keeps both new wrapper arguments optional for ordinary API callers', async () => {
    await sendMessage('nb-1', 'conv-1', 'Question', { kind: 'none' }, 'model');
    expect(invoke).toHaveBeenLastCalledWith('send_message', expect.objectContaining({
      historyBeforeUserMessageId: undefined, userMessageId: undefined,
    }));
  });
});
