import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ChatPanel } from './chat/ChatPanel';
import { NotesPanel } from './notes/NotesPanel';
import { PromptPanel } from './inspector/PromptPanel';

// Real components and React server rendering, with explicit store fixtures.
// Virtual layout requires a browser, so only the virtualizer is replaced by a
// deterministic list renderer. This does not certify scrolling or interaction.
const fixture = vi.hoisted(() => ({
  chat: {} as Record<string, unknown>, settings: {} as Record<string, unknown>,
  sources: {} as Record<string, unknown>, notes: {} as Record<string, unknown>,
}));
vi.mock('../stores/chatStore', () => ({ useChatStore: (select: (state: unknown) => unknown) => select(fixture.chat) }));
vi.mock('../stores/settingsStore', () => ({ useSettingsStore: (select: (state: unknown) => unknown) => select(fixture.settings) }));
vi.mock('../stores/sourceStore', () => ({ useSourceStore: (select: (state: unknown) => unknown) => select(fixture.sources) }));
vi.mock('../stores/noteStore', () => ({ useNoteStore: (select?: (state: unknown) => unknown) => select ? select(fixture.notes) : fixture.notes }));
vi.mock('react-virtuoso', () => ({ Virtuoso: ({ data, itemContent }: { data: unknown[]; itemContent: (index: number, item: unknown) => unknown }) => data.map((item, index) => itemContent(index, item)) }));

beforeEach(() => {
  fixture.chat = { conversations: [], messages: [], isStreaming: false, suggestedQuestions: [], style: 'default', customGoal: '', responseLength: 'default' };
  fixture.settings = { activeModel: 'current-model', models: [], settings: { default_provider: 'ollama' }, selectionPending: false };
  fixture.sources = { sources: [], selectedSourceIds: new Set(), sourceScopeMode: 'none', sourceListStatus: 'ready' };
  fixture.notes = { notes: [], loading: false, loadError: null };
});

describe('rendered UI trust contracts', () => {
  it('keeps an old answer model label stable after changing current model selection', () => {
    fixture.chat.messages = [{ id: 'answer', role: 'assistant', content: 'Captured answer', model_used: 'historic-model' }];
    const before = renderToStaticMarkup(createElement(ChatPanel, { notebookId: 'nb' }));
    fixture.settings.activeModel = 'new-current-model';
    const after = renderToStaticMarkup(createElement(ChatPanel, { notebookId: 'nb' }));
    for (const rendered of [before, after]) {
      expect(rendered).toContain('<span class="gloss-mono">historic-model</span>');
      expect(rendered).not.toContain('<span class="gloss-mono">current-model</span>');
      expect(rendered).not.toContain('<span class="gloss-mono">new-current-model</span>');
    }
  });

  it('renders an accessible multiline composer and disabled send during model persistence', () => {
    fixture.settings.selectionPending = true;
    const markup = renderToStaticMarkup(createElement(ChatPanel, { notebookId: 'nb' }));
    expect(markup).toMatch(/<textarea[^>]*aria-label="Chat message"/);
    expect(markup).toContain('Shift+Enter for a new line');
    expect(markup.match(/<button[^>]*aria-label="Send message"[^>]*>/)?.[0]).toContain('disabled=""');
    expect(markup).toContain('Saving model selection');
  });

  it('keeps Stop available while a response is active', () => {
    fixture.chat.isStreaming = true;
    fixture.settings.selectionPending = true;
    const markup = renderToStaticMarkup(createElement(ChatPanel, { notebookId: 'nb' }));
    const stop = markup.match(/<button[^>]*aria-label="Stop generation"[^>]*>/)?.[0];
    expect(stop).toBeDefined();
    expect(stop).not.toContain('disabled=');
  });

  it('offers a labeled full-note reading action and named persistence controls', () => {
    fixture.notes.notes = [{ id: 'note', title: 'Research', content: 'A long note '.repeat(30), note_type: 'manual', pinned: false }];
    const markup = renderToStaticMarkup(createElement(NotesPanel, { notebookId: 'nb' }));
    for (const label of ['Read full note Research', 'Edit note Research', 'Pin note Research', 'Delete note Research']) {
      expect(markup).toContain(`aria-label="${label}"`);
    }
    expect(markup).not.toContain('Confirm deletion of note Research');
  });

  it('shows uncaptured system prompt text explicitly instead of reconstructing it', () => {
    fixture.chat.messages = [{ id: 'answer', role: 'assistant', citations: { evidence: {
      context_passage_count: 0, context_digest: '', source_context_digest: '',
    } } }];
    const markup = renderToStaticMarkup(createElement(PromptPanel));
    expect(markup).toContain('System prompt text was not captured for this response.');
  });
});
