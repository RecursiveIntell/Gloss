import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { invoke } from '@tauri-apps/api/core';
import { useChatStore } from '../stores/chatStore';
import { useNotebookStore } from '../stores/notebookStore';
import { PromptPanel } from '../components/inspector/PromptPanel';
import { ReceiptPanel } from '../components/inspector/ReceiptPanel';
import { capturedModelLabel } from '../components/chat/ChatPanel';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('../stores/chatStore', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../stores/chatStore')>();
  // Server rendering normally reads Zustand's initial snapshot. Read the real
  // hydrated store here while retaining its actual actions and state methods.
  return { ...actual, useChatStore: Object.assign(
    (select: (state: ReturnType<typeof actual.useChatStore.getState>) => unknown) => select(actual.useChatStore.getState()),
    actual.useChatStore,
  ) };
});

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  useNotebookStore.setState({ activeNotebookId: 'notebook', activationStatus: 'confirmed' });
  useChatStore.setState({ messages: [], activeConversationId: null, isStreaming: false, streamingContent: '' });
});

async function restore(citations: string | null, model_used?: string) {
  vi.mocked(invoke).mockResolvedValueOnce([{
    id: 'saved-answer', conversation_id: 'conversation', role: 'assistant',
    content: 'Persisted answer', citations, model_used, created_at: '2026-09-05T00:00:00Z',
  }]);
  await useChatStore.getState().loadMessages('notebook', 'conversation');
  expect(invoke).toHaveBeenCalledWith('load_messages', { notebookId: 'notebook', conversationId: 'conversation' });
  return useChatStore.getState().messages[0];
}

describe('persisted message evidence through the real IPC wrapper and store', () => {
  it('restores the captured full prompt, receipts and model identity to every inspector', async () => {
    // Rust Message.citations is Option<String>, not the frontend envelope type.
    // Exercise that actual wire shape; object-only fixtures missed this defect.
    const payload = { citations: [], evidence: {
      receipt_id: 'saved-evidence', context_passage_count: 0, context_digest: 'context', source_context_digest: 'source',
      prompt_receipt: {
        schema: 'PromptReceiptV1', receipt_id: 'saved-prompt', capture_state: 'captured_system_prompt',
        redaction_state: 'system_prompt_stored_other_content_digest_only',
        system_prompt_text: 'The exact historical system prompt.', system_prompt_digest: 'system-digest',
        user_turn_digest: 'user-digest', context_payload_digest: 'context-digest', source_passage_count: 0, prompt_digest: 'prompt-digest',
      },
      generation_receipt: {
        schema: 'GenerationReceiptV1', receipt_id: 'saved-generation', status: 'completed',
        provider: 'ollama', model: 'historical-model', prompt_receipt_id: 'saved-prompt',
        decoding_settings_receipt_id: 'saved-decoding', provider_request_digest: 'request-digest',
        partial_persisted: true, done_frame_seen: true, eof_seen: false, chunks_seen: 1,
      },
    } };
    const message = await restore(JSON.stringify(payload), 'historical-model');
    expect(message.citations).toEqual(payload);
    expect(capturedModelLabel(message)).toBe('historical-model · ollama');
    const prompt = renderToStaticMarkup(createElement(PromptPanel));
    const receipt = renderToStaticMarkup(createElement(ReceiptPanel));
    expect(prompt).toContain('The exact historical system prompt.');
    expect(prompt).toContain('Copy full prompt');
    for (const markup of [prompt, receipt]) {
      expect(markup).toContain('historical-model');
      expect(markup).toContain('ollama');
      expect(markup).toContain('saved-prompt');
      expect(markup).toContain('completed');
      expect(markup).not.toContain('No prompt data available');
    }
  });

  it('preserves legacy citation references without inventing captured receipts', async () => {
    const citations = [{ chunk_id: 'chunk', source_id: 'source', source_title: 'Historical source' }];
    const message = await restore(JSON.stringify(citations));
    expect(message.citations?.citations).toEqual(citations);
    expect(message.citations?.evidence.backend_used).toBe('unknown');
    expect(message.citations?.evidence.source_scope_preserved).toBe(false);
    expect(message.citations?.evidence.generation_receipt).toBeNull();
    expect(renderToStaticMarkup(createElement(PromptPanel))).toContain('System prompt text was not captured');
    expect(capturedModelLabel(message)).toBe('Model not captured');
  });

  it.each([null, '{broken json', '42'])('keeps the saved answer usable with uncaptured evidence %s', async (raw) => {
    const message = await restore(raw);
    expect(message.content).toBe('Persisted answer');
    expect(message.citations?.evidence.generation_receipt).toBeFalsy();
    expect(capturedModelLabel(message)).toBe('Model not captured');
    const prompt = renderToStaticMarkup(createElement(PromptPanel));
    const receipt = renderToStaticMarkup(createElement(ReceiptPanel));
    expect(prompt).not.toContain('Copy full prompt');
    expect(receipt).not.toContain('text-success');
  });
});
