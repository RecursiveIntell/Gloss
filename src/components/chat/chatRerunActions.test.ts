import { isValidElement, type ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatPanel } from './ChatPanel';
import type { Message } from '../../lib/types';

const fixture = vi.hoisted(() => ({
  chat: {} as Record<string, unknown>, settings: {} as Record<string, unknown>,
  sources: {} as Record<string, unknown>, notebook: {} as Record<string, unknown>,
  state: [] as unknown[], stateIndex: 0,
}));

// Exercise the component's real callbacks with controlled hook state. Browser
// rendering and native interaction remain covered by the desktop workflow.
vi.mock('react', async (importOriginal) => ({
  ...await importOriginal<typeof import('react')>(),
  useState: (initial: unknown) => {
    const index = fixture.stateIndex++;
    if (!(index in fixture.state)) fixture.state[index] = initial;
    return [fixture.state[index], (value: unknown) => {
      fixture.state[index] = typeof value === 'function' ? value(fixture.state[index]) : value;
    }];
  },
  useEffect: () => {},
  useMemo: (compute: () => unknown) => compute(),
}));
vi.mock('../../stores/chatStore', () => ({
  useChatStore: Object.assign(
    (select: (state: unknown) => unknown) => select(fixture.chat),
    { getState: () => fixture.chat },
  ),
}));
vi.mock('../../stores/settingsStore', () => ({ useSettingsStore: (select: (state: unknown) => unknown) => select(fixture.settings) }));
vi.mock('../../stores/sourceStore', () => ({ useSourceStore: (select: (state: unknown) => unknown) => select(fixture.sources) }));
vi.mock('../../stores/noteStore', () => ({ useNoteStore: () => vi.fn() }));
vi.mock('../../stores/notebookStore', () => ({ useNotebookStore: { getState: () => fixture.notebook } }));

const originalUser: Message = {
  id: '11111111-1111-4111-8111-111111111111', conversation_id: 'conv-1',
  role: 'user', content: 'Original question', created_at: '2026-09-05T00:00:00Z',
};
const originalAnswer: Message = { ...originalUser, id: 'original-answer', role: 'assistant', content: 'Original answer' };
const laterUser: Message = { ...originalUser, id: 'later-user', content: 'Later question' };
const laterAnswer: Message = { ...originalAnswer, id: 'later-answer', content: 'Later answer' };

function renderPanel(notebookId = 'nb-1') {
  fixture.stateIndex = 0;
  return ChatPanel({ notebookId });
}

function findProps(node: ReactNode, matches: (props: Record<string, unknown>) => boolean): Record<string, unknown> | undefined {
  if (Array.isArray(node)) {
    for (const child of node) {
      const found = findProps(child, matches);
      if (found) return found;
    }
  } else if (isValidElement<Record<string, unknown>>(node)) {
    if (matches(node.props)) return node.props;
    return findProps(node.props.children as ReactNode, matches);
  }
  return undefined;
}

function rowActions(node: ReactNode) {
  const props = findProps(node, (props) => !!props.value && typeof props.value === 'object' && 'onEditUserMessage' in props.value);
  expect(props).toBeDefined();
  return props!.value as {
    onEditUserMessage: (message: Message) => void;
    onRegenerate: (messageId: string) => Promise<void>;
  };
}

function enterText(node: ReactNode, value: string) {
  const composer = findProps(node, (props) => props['aria-label'] === 'Chat message');
  expect(composer).toBeDefined();
  (composer!.onChange as (event: { target: { value: string } }) => void)({ target: { value } });
}

async function clickSend(node: ReactNode, label: string) {
  const button = findProps(node, (props) => props['aria-label'] === label);
  expect(button).toBeDefined();
  await (button!.onClick as () => Promise<void>)();
}

function deferredFailure() {
  let fail!: () => void;
  fixture.chat.sendMessage = vi.fn().mockImplementationOnce(() => new Promise<void>((resolve) => {
    fail = () => {
      fixture.chat.streamingError = 'Late provider failure';
      resolve();
    };
  })).mockResolvedValue(undefined);
  return () => fail();
}

function selectConversation(node: ReactNode, value: string) {
  const select = findProps(node, (props) => props['aria-label'] === 'Conversation');
  expect(select).toBeDefined();
  (select!.onChange as (event: { target: { value: string } }) => void)({ target: { value } });
}

beforeEach(() => {
  fixture.state = [];
  fixture.stateIndex = 0;
  fixture.chat = {
    conversations: [{ id: 'conv-1' }, { id: 'conv-2' }], activeConversationId: 'conv-1',
    messages: [originalUser, originalAnswer, laterUser, laterAnswer],
    isStreaming: false, suggestedQuestions: [], style: 'default', customGoal: '',
    responseLength: 'default', sendMessage: vi.fn().mockResolvedValue(undefined),
    setActiveConversation: (id: string) => { fixture.chat.activeConversationId = id; },
    loadMessages: vi.fn(),
  };
  fixture.settings = { activeModel: 'model', models: [], settings: {}, selectionPending: false };
  fixture.sources = {
    sources: [], selectedSourceIds: new Set(), sourceScopeMode: 'none', sourceListStatus: 'ready',
    getSourceScope: () => ({ kind: 'none' }),
  };
  fixture.notebook = { activeNotebookId: 'nb-1', activationRequestId: 0 };
});

describe('chat rerun action targets', () => {
  it('restores the edited query and its same target after a failed submission', async () => {
    fixture.chat.sendMessage = vi.fn().mockImplementation(async () => {
      fixture.chat.streamingError = 'Provider temporarily unavailable';
    });
    rowActions(renderPanel()).onEditUserMessage(originalUser);
    enterText(renderPanel(), 'Replacement question');
    await clickSend(renderPanel(), 'Rerun edited message');

    const restored = renderPanel();
    expect(findProps(restored, (props) => props['aria-label'] === 'Chat message')?.value).toBe('Replacement question');
    await clickSend(restored, 'Rerun edited message');
    expect(fixture.chat.sendMessage).toHaveBeenLastCalledWith(
      'nb-1', 'Replacement question', { kind: 'none' }, 'model', originalUser.id,
    );
  });

  it.each(['new draft', 'new edit'])('does not replace a %s when an older edited submission fails', async (newAction) => {
    const fail = deferredFailure();
    rowActions(renderPanel()).onEditUserMessage(originalUser);
    enterText(renderPanel(), 'Old replacement');
    const sending = clickSend(renderPanel(), 'Rerun edited message');
    if (newAction === 'new draft') enterText(renderPanel(), 'New draft');
    else rowActions(renderPanel()).onEditUserMessage(laterUser);
    fail();
    await sending;

    const current = renderPanel();
    const expectedText = newAction === 'new draft' ? 'New draft' : laterUser.content;
    expect(findProps(current, (props) => props['aria-label'] === 'Chat message')?.value).toBe(expectedText);
    await clickSend(current, newAction === 'new draft' ? 'Send message' : 'Rerun edited message');
    expect(fixture.chat.sendMessage).toHaveBeenLastCalledWith(
      'nb-1', expectedText, { kind: 'none' }, 'model', newAction === 'new draft' ? undefined : laterUser.id,
    );
  });

  it('does not restore a deliberately cleared newer draft after a late failure', async () => {
    const fail = deferredFailure();
    rowActions(renderPanel()).onEditUserMessage(originalUser);
    const sending = clickSend(renderPanel(), 'Rerun edited message');
    enterText(renderPanel(), 'New draft');
    enterText(renderPanel(), '');
    fail();
    await sending;
    const current = renderPanel();
    expect(findProps(current, (props) => props['aria-label'] === 'Chat message')?.value).toBe('');
    expect(findProps(current, (props) => props['aria-label'] === 'Rerun edited message')).toBeUndefined();
  });

  it('clears an existing edit when the conversation changes', () => {
    rowActions(renderPanel()).onEditUserMessage(originalUser);
    fixture.chat.activeConversationId = 'conv-2';
    const switched = renderPanel();
    expect(findProps(switched, (props) => props['aria-label'] === 'Chat message')?.value).toBe('');
    expect(findProps(switched, (props) => props['aria-label'] === 'Rerun edited message')).toBeUndefined();
  });

  it.each([false, true])('does not restore after conversation selection, including return=%s before rerender', async (returnToOriginal) => {
    const fail = deferredFailure();
    rowActions(renderPanel()).onEditUserMessage(originalUser);
    const sending = clickSend(renderPanel(), 'Rerun edited message');
    const pending = renderPanel();
    selectConversation(pending, 'conv-2');
    if (returnToOriginal) selectConversation(pending, 'conv-1');
    fail();
    await sending;
    const current = renderPanel();
    expect(findProps(current, (props) => props['aria-label'] === 'Chat message')?.value).toBe('');
    expect(findProps(current, (props) => props['aria-label'] === 'Rerun edited message')).toBeUndefined();
  });

  it.each([false, true])('does not restore after notebook selection, including return=%s before rerender', async (returnToOriginal) => {
    const fail = deferredFailure();
    rowActions(renderPanel()).onEditUserMessage(originalUser);
    const sending = clickSend(renderPanel(), 'Rerun edited message');
    fixture.notebook = { activeNotebookId: returnToOriginal ? 'nb-1' : 'nb-2', activationRequestId: returnToOriginal ? 2 : 1 };
    fail();
    await sending;
    const current = renderPanel(returnToOriginal ? 'nb-1' : 'nb-2');
    expect(findProps(current, (props) => props['aria-label'] === 'Chat message')?.value).toBe('');
    expect(findProps(current, (props) => props['aria-label'] === 'Rerun edited message')).toBeUndefined();
  });

  it.each([false, true])('preserves ordinary first-send recovery when conversation creation rerenders=%s', async (rerender) => {
    fixture.chat.activeConversationId = null;
    const fail = deferredFailure();
    enterText(renderPanel(), 'First question');
    const sending = clickSend(renderPanel(), 'Send message');
    fixture.chat.activeConversationId = 'created-conversation';
    if (rerender) renderPanel();
    fail();
    await sending;
    const restored = renderPanel();
    expect(findProps(restored, (props) => props['aria-label'] === 'Chat message')?.value).toBe('First question');
    await clickSend(restored, 'Send message');
    expect(fixture.chat.sendMessage).toHaveBeenLastCalledWith('nb-1', 'First question', { kind: 'none' }, 'model', undefined);
  });

  it('captures the edited question before clearing composer state and leaves the next send unanchored', async () => {
    rowActions(renderPanel()).onEditUserMessage(originalUser);
    const editing = renderPanel();
    expect(findProps(editing, (props) => props.children === 'Rerun uses the conversation before this question. All saved turns are retained.')).toBeDefined();
    enterText(editing, '  Replacement question  ');
    await clickSend(renderPanel(), 'Rerun edited message');
    expect(fixture.chat.sendMessage).toHaveBeenLastCalledWith(
      'nb-1', 'Replacement question', { kind: 'none' }, 'model', originalUser.id,
    );

    const cleared = renderPanel();
    expect(findProps(cleared, (props) => props['aria-label'] === 'Chat message')?.value).toBe('');
    enterText(cleared, 'New question');
    await clickSend(renderPanel(), 'Send message');
    expect(fixture.chat.sendMessage).toHaveBeenLastCalledWith(
      'nb-1', 'New question', { kind: 'none' }, 'model', undefined,
    );
  });

  it('regenerates from the user preceding the selected answer even when later turns exist', async () => {
    await rowActions(renderPanel()).onRegenerate(originalAnswer.id);
    expect(fixture.chat.sendMessage).toHaveBeenCalledExactlyOnceWith(
      'nb-1', originalUser.content, { kind: 'none' }, 'model', originalUser.id,
    );
    expect(fixture.chat.messages).toEqual([originalUser, originalAnswer, laterUser, laterAnswer]);
  });
});
