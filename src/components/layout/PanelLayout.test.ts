import { beforeEach, expect, it, vi } from 'vitest';
import { isValidElement, type ReactNode } from 'react';
import { PanelLayout } from './PanelLayout';

const hooks = vi.hoisted(() => ({ slots: [] as any[], index: 0, effects: [] as (() => unknown)[] }));
vi.mock('react', async (original) => ({ ...await original<typeof import('react')>(),
  useState: (initial: any) => {
    const index = hooks.index++;
    if (!(index in hooks.slots)) hooks.slots[index] = typeof initial === 'function' ? initial() : initial;
    return [hooks.slots[index], (value: any) => {
      hooks.slots[index] = typeof value === 'function' ? value(hooks.slots[index]) : value;
    }];
  },
  useRef: (initial: any) => { const index = hooks.index++; return hooks.slots[index] ??= { current: initial }; },
  useEffect: (effect: () => unknown) => { hooks.effects.push(effect); },
}));
vi.mock('zustand', async (original) => {
  const actual = await original<any>();
  return { ...actual, create: (init: any) => {
    const store = actual.create(init);
    return Object.assign((select: any = (state: any) => state) => select(store.getState()), store);
  } };
});
vi.mock('../../lib/tauri', () => ({}));
let resize: (entries: any[]) => void;
const focus = vi.fn();
function find(node: ReactNode, predicate: (props: any) => boolean): any {
  if (Array.isArray(node)) return node.map((child) => find(child, predicate)).find(Boolean);
  if (isValidElement(node)) {
    const props = node.props as any;
    return predicate(props) ? node : find(props.children, predicate);
  }
}
function render() { hooks.index = 0; hooks.effects = []; return PanelLayout({ notebookId: 'nb-1' }); }
function mount(width: number, leftOpen = true, rightOpen = false) {
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => key.endsWith('leftCollapsed') ? (leftOpen ? '0' : '1')
      : key.endsWith('rightCollapsed') ? (rightOpen ? '0' : '1') : null,
    setItem: vi.fn(),
  });
  const tree = render();
  (tree.props.ref as any).current = { getBoundingClientRect: () => ({ width }) };
  hooks.effects.forEach((effect) => effect());
  return tree;
}
beforeEach(() => {
  hooks.slots = []; hooks.index = 0; hooks.effects = []; focus.mockClear();
  vi.stubGlobal('document', { getElementById: () => ({ focus }) });
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: typeof resize) { resize = callback; }
    observe() {} disconnect() {}
  });
});
it('focus closes a restored compact drawer before delayed observer delivery', () => {
  const tree = mount(1150);
  expect(find(tree, (p) => p.title === 'Sources')).toBeTruthy();
  find(tree, (p) => p.label === 'Focus chat message').props.onClick();
  resize([{ contentRect: { width: 1150 } }]);
  const after = render();
  expect(find(after, (p) => p.title === 'Sources')).toBeUndefined();
  expect(find(after, (p) => p.label === 'Open inspector')).toBeTruthy();
  expect(focus).toHaveBeenCalledOnce();
});
it('opening inspector before measurement closes restored compact sources', () => {
  const tree = mount(1150);
  find(tree, (p) => p.label === 'Open inspector').props.onClick();
  resize([{ contentRect: { width: 1150 } }]);
  const after = render();
  expect(find(after, (p) => p.title === 'Sources')).toBeUndefined();
  expect(find(after, (p) => p.label === 'Close inspector')).toBeTruthy();
});
it('opening sources before measurement closes restored compact inspector', () => {
  const tree = mount(1150, false, true);
  find(tree, (p) => p.label === 'Open sources').props.onClick();
  resize([{ contentRect: { width: 1150 } }]);
  const after = render();
  expect(find(after, (p) => p.label === 'Close sources')).toBeTruthy();
  expect(find(after, (p) => p.label === 'Open inspector')).toBeTruthy();
});
it('focusing chat preserves non-overlay wide panels', () => {
  const tree = mount(1800);
  find(tree, (p) => p.label === 'Focus chat message').props.onClick();
  expect(find(render(), (p) => p.title === 'Sources')).toBeTruthy();
  expect(focus).toHaveBeenCalledOnce();
});
