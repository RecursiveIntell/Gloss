import { isValidElement, type ReactNode } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { SettingsDialog } from './index';

// Focus lifecycle has separate native/markup gates; this probe owns numeric controls.
vi.mock('../../../lib/useModalFocus', () => ({ useModalFocus: () => ({ current: null }) }));

const fixture = vi.hoisted(() => ({ state: [] as unknown[], index: 0 }));
vi.mock('react', async (importOriginal) => ({
  ...await importOriginal<typeof import('react')>(),
  useEffect: () => {},
  useMemo: (factory: () => unknown) => factory(),
  useState: (initial: unknown) => {
    const index = fixture.index++;
    if (!(index in fixture.state)) fixture.state[index] = initial;
    return [fixture.state[index], (value: unknown) => {
      fixture.state[index] = typeof value === 'function' ? value(fixture.state[index]) : value;
    }];
  },
}));
vi.mock('../../../stores/settingsStore', () => ({ useSettingsStore: () => ({
  models: [], settings: {}, featureFlags: [], providers: [], activeModel: '', externalTools: {},
}) }));
vi.mock('../../../stores/notebookStore', () => ({ useNotebookStore: (select: (state: unknown) => unknown) => select({ activeNotebookId: null }) }));

function render() {
  fixture.index = 0;
  return SettingsDialog({ open: true, onClose: vi.fn() });
}
function find(node: ReactNode, label: string): Record<string, unknown> | undefined {
  if (Array.isArray(node)) {
    for (const child of node) { const result = find(child, label); if (result) return result; }
  } else if (isValidElement<Record<string, unknown>>(node)) {
    if (node.props['aria-label'] === label) return node.props;
    return find(node.props.children as ReactNode, label);
  }
}
function input(label: string) {
  const result = find(render(), label); expect(result).toBeDefined(); return result!;
}
function enter(label: string, value: string) {
  (input(label).onChange as (event: { target: { value: string } }) => void)({ target: { value } });
}
beforeEach(() => { fixture.index = 0; fixture.state = []; });

it('300-second embedding input keeps the full five-second grace in the actual controls', () => {
  for (const seconds of [2, 60, 295, 296, 299, 300]) {
    enter('Embedding timeout seconds', String(seconds));
    const search = input('Search timeout milliseconds');
    expect(search.min).toBe(seconds * 1000 + 5000);
    expect(Number(search.max)).toBe(305000);
    expect(Number(search.value)).toBeGreaterThanOrEqual(seconds * 1000 + 5000);
  }
  expect(input('Search timeout milliseconds').value).toBe('305000');
});
it('native embedding controls do not inherit Ollama timeout coupling', () => {
  enter('Embedding backend', 'fastembed');
  expect(input('Search timeout milliseconds').min).toBe(100);
  expect(input('Embedding timeout seconds').disabled).toBe(true);
});
