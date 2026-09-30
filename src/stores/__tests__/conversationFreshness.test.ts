import { beforeEach, expect, it, vi } from 'vitest';
import { useChatStore } from '../chatStore';
import { useNotebookStore } from '../notebookStore';
import { useToastStore } from '../toastStore';
import * as api from '../../lib/tauri';
import type { Conversation } from '../../lib/types';

vi.mock('../../lib/tauri', () => ({ listConversations: vi.fn() }));
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
const row = (id: string) => ({ id } as Conversation);
beforeEach(() => {
  vi.resetAllMocks();
  useNotebookStore.setState({ activeNotebookId: 'A', activationRequestId: 1 });
  useChatStore.getState().resetForNotebookSwitch();
  useToastStore.setState({ toasts: [] });
});
it('keeps the latest list when an older same-notebook read finishes last', async () => {
  const old = deferred<Conversation[]>();
  vi.mocked(api.listConversations).mockReturnValueOnce(old.promise).mockResolvedValueOnce([row('current')]);
  const first = useChatStore.getState().loadConversations('A');
  await useChatStore.getState().loadConversations('A');
  old.resolve([row('deleted')]);
  await first;
  expect(useChatStore.getState().conversations.map(c => c.id)).toEqual(['current']);
});
it('rejects an old A read after A to B to A, even before the next A read', async () => {
  const old = deferred<Conversation[]>();
  vi.mocked(api.listConversations).mockReturnValueOnce(old.promise);
  const first = useChatStore.getState().loadConversations('A');
  useNotebookStore.setState({ activeNotebookId: 'B', activationRequestId: 2 });
  useChatStore.getState().resetForNotebookSwitch();
  useNotebookStore.setState({ activeNotebookId: 'A', activationRequestId: 3 });
  useChatStore.getState().resetForNotebookSwitch();
  old.resolve([row('old-A')]);
  await first;
  expect(useChatStore.getState().conversations).toEqual([]);
});
it('does not show an obsolete list failure in the new notebook', async () => {
  const old = deferred<Conversation[]>();
  vi.mocked(api.listConversations).mockReturnValueOnce(old.promise);
  const first = useChatStore.getState().loadConversations('A');
  useNotebookStore.setState({ activeNotebookId: 'B', activationRequestId: 2 });
  useChatStore.getState().resetForNotebookSwitch();
  old.reject(new Error('obsolete A read'));
  await first;
  expect(useToastStore.getState().toasts).toEqual([]);
});
it('does not let a background refresh supersede the active notebook request', async () => {
  const current = deferred<Conversation[]>();
  vi.mocked(api.listConversations).mockReturnValueOnce(current.promise);
  const first = useChatStore.getState().loadConversations('A');
  await useChatStore.getState().loadConversations('B');
  current.resolve([row('current')]);
  await first;
  expect(api.listConversations).toHaveBeenCalledTimes(1);
  expect(useChatStore.getState().conversations.map(c => c.id)).toEqual(['current']);
});
