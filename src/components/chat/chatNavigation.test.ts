import { isValidElement, type ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatPanel } from './ChatPanel';

const fixture = vi.hoisted(() => ({
  chat: {} as Record<string, any>, notebook: {} as Record<string, any>,
  slots: [] as any[], index: 0, effects: [] as (() => void)[],
}));
// Exercise the real component's navigation callbacks and context guards.
// This is a public Virtuoso-handle contract, not browser geometry proof.
vi.mock('react', async (original) => ({
  ...await original<typeof import('react')>(),
  useState: (initial: unknown) => {
    const index = fixture.index++;
    if (!(index in fixture.slots)) fixture.slots[index] = initial;
    return [fixture.slots[index], (value: any) => {
      fixture.slots[index] = typeof value === 'function' ? value(fixture.slots[index]) : value;
    }];
  },
  useRef: (initial: unknown) => {
    const index = fixture.index++;
    return fixture.slots[index] ??= { current: initial };
  },
  useEffect: (effect: () => void) => { fixture.effects.push(effect); },
  useMemo: (compute: () => unknown) => compute(),
}));
vi.mock('../../stores/chatStore', () => ({ useChatStore: Object.assign(
  (select: (state: any) => unknown) => select(fixture.chat), { getState: () => fixture.chat },
) }));
vi.mock('../../stores/notebookStore', () => ({ useNotebookStore: { getState: () => fixture.notebook } }));
vi.mock('../../stores/settingsStore', () => ({ useSettingsStore: (select: (state: any) => unknown) => select({
  activeModel: 'model', models: [], settings: {}, selectionPending: false,
}) }));
vi.mock('../../stores/sourceStore', () => ({ useSourceStore: (select: (state: any) => unknown) => select({
  sources: [], selectedSourceIds: new Set(), sourceScopeMode: 'none', sourceListStatus: 'ready',
  getSourceScope: () => ({ kind: 'none' }),
}) }));
vi.mock('../../stores/noteStore', () => ({ useNoteStore: () => vi.fn() }));

function find(node: ReactNode, predicate: (props: any) => boolean): any {
  if (Array.isArray(node)) return node.map((child) => find(child, predicate)).find(Boolean);
  if (isValidElement(node)) {
    const props = node.props as Record<string, any>;
    return predicate(props) ? node : find(props.children, predicate);
  }
}
const list = (tree: ReactNode) => find(tree, (props) => Array.isArray(props.data) && !!props.itemContent);
const button = (tree: ReactNode, label: string) => find(tree, (props) => props['aria-label'] === label);
const scroll = vi.fn();
function render(notebookId = 'nb') {
  fixture.index = 0;
  fixture.effects = [];
  const tree = ChatPanel({ notebookId });
  if (list(tree).props.ref) list(tree).props.ref.current = { scrollToIndex: scroll };
  return tree;
}
function flush() { fixture.effects.forEach((effect) => effect()); }
function appendUser(id = 'new-user') {
  fixture.chat.messages = [...fixture.chat.messages, { id, role: 'user', content: 'New question', conversation_id: fixture.chat.activeConversationId }];
}
async function send() {
  const tree = render();
  button(tree, 'Chat message').props.onChange({ target: { value: 'New question' } });
  return button(render(), 'Send message').props.onClick();
}
beforeEach(() => {
  fixture.slots = [];
  fixture.notebook = { activeNotebookId: 'nb', activationRequestId: 1 };
  fixture.chat = {
    activeConversationId: 'conv', conversations: [{ id: 'conv' }, { id: 'other' }],
    messages: [{ id: 'old-user', role: 'user', content: 'Old question', conversation_id: 'conv' },
               { id: 'old-answer', role: 'assistant', content: 'Old answer', conversation_id: 'conv' }],
    isStreaming: false, suggestedQuestions: [], style: 'default', customGoal: '', responseLength: 'default',
    replayChatEvents: vi.fn().mockResolvedValue(undefined), rehydrateConversation: vi.fn(),
    loadMessages: vi.fn(), setActiveConversation: (id: string) => { fixture.chat.activeConversationId = id; },
    sendMessage: vi.fn().mockImplementation(async () => {
      fixture.chat.streamingMessageId = 'owned-answer';
      fixture.chat.streamingNotebookId = 'nb';
      fixture.chat.isStreaming = true;
      appendUser();
    }),
  };
  scroll.mockClear();
});

describe('chat latest-message navigation', () => {
  it('uses a real Jump control and waits for the virtualizer acknowledgement', () => {
    let tree = render();
    expect(list(tree).props.atBottomStateChange).toBeTypeOf('function');
    list(tree).props.atBottomStateChange(false);
    tree = render();
    button(tree, 'Jump to latest').props.onClick();
    expect(scroll).toHaveBeenCalledExactlyOnceWith({ index: 'LAST', align: 'end', behavior: 'auto' });
    expect(find(render(), (props) => props['aria-label'] === 'Chat messages').props['data-chat-at-bottom']).toBe(false);
    list(render()).props.atBottomStateChange(true);
    expect(find(render(), (props) => props['aria-label'] === 'Chat messages').props['data-chat-at-bottom']).toBe(true);
    expect(button(render(), 'Jump to latest')).toBeUndefined();
  });

  it('jumps once after its own appended user and leaves later history reading alone', async () => {
    await send();
    render(); flush();
    expect(scroll).toHaveBeenCalledTimes(1);
    find(render(), (props) => props['aria-label'] === 'Chat messages').props.onWheelCapture();
    list(render()).props.atBottomStateChange(false);
    fixture.chat.messages = [...fixture.chat.messages, { id: 'owned-answer', role: 'assistant', content: 'New answer' }];
    const tree = render(); flush();
    expect(scroll).toHaveBeenCalledTimes(1);
    expect(list(tree).props.followOutput).toBe('auto');
    expect(button(tree, 'Jump to latest')).toBeDefined();
  });

  it('keeps explicit latest intent across measured streaming-footer growth and completion', async () => {
    await send();
    render(); flush();
    expect(scroll).toHaveBeenCalledTimes(1);
    fixture.chat.streamingContent = 'New streamed answer';
    let tree = render();
    list(tree).props.atBottomStateChange(false);
    list(tree).props.totalListHeightChanged(1200);
    expect(scroll).toHaveBeenCalledTimes(2);
    expect(find(render(), (props) => props['aria-label'] === 'Chat messages').props['data-chat-at-bottom']).toBe(false);
    fixture.chat.isStreaming = false;
    fixture.chat.messages = [...fixture.chat.messages, { id: 'owned-answer', role: 'assistant', content: 'New streamed answer', conversation_id: 'conv' }];
    tree = render();
    list(tree).props.totalListHeightChanged(1300);
    expect(scroll).toHaveBeenCalledTimes(3);
    find(tree, (props) => props['aria-label'] === 'Chat messages').props.onWheelCapture();
    list(tree).props.totalListHeightChanged(1400);
    expect(scroll).toHaveBeenCalledTimes(3);
  });

  it('does not carry measured-height following into a different notebook activation', async () => {
    await send();
    render(); flush();
    const oldList = list(render());
    fixture.notebook.activationRequestId += 2;
    oldList.props.totalListHeightChanged(1200);
    expect(scroll).toHaveBeenCalledTimes(1);
  });

  it.each(['failure', 'conversation round trip'])('cancels measured-height following after %s', async (change) => {
    await send();
    render(); flush();
    if (change === 'failure') fixture.chat.streamingError = 'Rejected';
    else {
      button(render(), 'Conversation').props.onChange({ target: { value: 'other' } });
      button(render(), 'Conversation').props.onChange({ target: { value: 'conv' } });
    }
    list(render()).props.totalListHeightChanged(1200);
    expect(scroll).toHaveBeenCalledTimes(1);
  });

  it('keeps the streaming footer component identity stable as tokens change', () => {
    fixture.chat.isStreaming = true;
    fixture.chat.streamingContent = 'First';
    const before = list(render()).props.components.Footer;
    fixture.chat.streamingContent = 'First second';
    const after = list(render()).props.components.Footer;
    expect(after).toBe(before);
  });

  it.each(['conversation', 'notebook', 'failure', 'history', 'pointer history'])('drops pending navigation after %s changes', async (change) => {
    fixture.chat.sendMessage = vi.fn().mockImplementation(async () => {
      fixture.chat.streamingMessageId = 'owned-answer'; fixture.chat.streamingNotebookId = 'nb';
    });
    await send();
    if (change === 'conversation') {
      button(render(), 'Conversation').props.onChange({ target: { value: 'other' } });
      button(render(), 'Conversation').props.onChange({ target: { value: 'conv' } });
    } else if (change === 'notebook') fixture.notebook.activationRequestId += 2;
    else if (change === 'failure') fixture.chat.streamingError = 'Failed before new row';
    else if (change === 'history') find(render(), (props) => props['aria-label'] === 'Chat messages').props.onWheelCapture();
    else find(render(), (props) => props['aria-label'] === 'Chat messages').props.onPointerDownCapture();
    appendUser();
    render(); flush();
    expect(scroll).not.toHaveBeenCalled();
  });

  it('follows the first send after its own conversation is created', async () => {
    fixture.chat.activeConversationId = null;
    fixture.chat.messages = [];
    fixture.chat.sendMessage = vi.fn().mockImplementation(async () => {
      fixture.chat.streamingMessageId = 'owned-answer'; fixture.chat.streamingNotebookId = 'nb';
    });
    await send();
    fixture.chat.activeConversationId = 'created';
    appendUser();
    render(); flush();
    expect(scroll).toHaveBeenCalledTimes(1);
  });

  it('gives explicit regeneration the same one-shot navigation intent', async () => {
    const actions = find(render(), (props) => props.value?.onRegenerate).props.value;
    await actions.onRegenerate('old-answer');
    render(); flush();
    expect(scroll).toHaveBeenCalledTimes(1);
    expect(fixture.chat.sendMessage).toHaveBeenCalledWith('nb', 'Old question', { kind: 'none' }, 'model', 'old-user');
  });

  it('keys virtual rows and the list by canonical message/conversation identity', () => {
    const first = list(render());
    expect(first.props.computeItemKey(8, { id: 'saved-id' })).toBe('saved-id');
    fixture.chat.activeConversationId = 'other';
    expect(list(render()).key).not.toBe(first.key);
  });

  it('does not inherit an old notebook bottom acknowledgement for equal conversation IDs', () => {
    list(render()).props.atBottomStateChange(true);
    fixture.notebook.activeNotebookId = 'other-nb';
    expect(find(render('other-nb'), (props) => props['aria-label'] === 'Chat messages').props['data-chat-at-bottom']).toBe(false);
  });

  it('projects the actual latest user identity while saved assistant hydration is pending', () => {
    appendUser('pending-user');
    const region = find(render(), (props) => props['aria-label'] === 'Chat messages');
    expect(region.props['data-chat-latest-message-id']).toBe('pending-user');
  });
});
