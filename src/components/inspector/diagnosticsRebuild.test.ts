import { isValidElement, type ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DiagnosticsPanel } from './DiagnosticsPanel';

const fixture = vi.hoisted(() => ({
  notebook: 'nb', rebuild: vi.fn(), sources: vi.fn(), stats: vi.fn(), poll: vi.fn(),
}));
vi.mock('react', async (original) => ({
  ...await original<typeof import('react')>(),
  useState: (value: unknown) => [value, vi.fn()],
}));
vi.mock('../../lib/tauri', () => ({ nativeDenseRebuild: (...args: unknown[]) => fixture.rebuild(...args) }));
vi.mock('../../stores/notebookStore', () => ({
  useNotebookStore: { getState: () => ({ activeNotebookId: fixture.notebook }) },
}));
vi.mock('../../stores/sourceStore', () => ({
  useSourceStore: Object.assign((select: (state: any) => unknown) => select({ stats: null }), {
    getState: () => ({ loadSources: fixture.sources, loadStats: fixture.stats }),
  }),
}));
vi.mock('../../stores/settingsStore', () => ({
  findSelectedModel: () => null,
  useSettingsStore: (select: (state: any) => unknown) => select({ models: [], settings: {} }),
}));
vi.mock('../../stores/healthStore', () => ({
  useHealthStore: (select: (state: any) => unknown) => select({ poll: fixture.poll }),
}));

function rebuildAction(node: ReactNode): (() => Promise<void>) | undefined {
  if (Array.isArray(node)) return node.map(rebuildAction).find(Boolean);
  if (isValidElement(node)) {
    const props = node.props as Record<string, any>;
    if (node.type === 'button' && props.children === 'Rebuild dense index') return props.onClick;
    return rebuildAction(props.children);
  }
}
beforeEach(() => {
  vi.resetAllMocks();
  fixture.notebook = 'nb';
  fixture.rebuild.mockResolvedValue({ status: 'ready' });
});

describe('native rebuild observation refresh', () => {
  it.each(['success', 'failure'])('refreshes canonical rows, stats and health after %s', async (outcome) => {
    if (outcome === 'failure') fixture.rebuild.mockRejectedValue(new Error('rebuild failed'));
    await rebuildAction(DiagnosticsPanel({ notebookId: 'nb' }))!();
    expect(fixture.rebuild).toHaveBeenCalledExactlyOnceWith('nb');
    expect(fixture.sources).toHaveBeenCalledExactlyOnceWith('nb');
    expect(fixture.stats).toHaveBeenCalledExactlyOnceWith('nb');
    expect(fixture.poll).toHaveBeenCalledTimes(1);
  });
  it('does not refresh a different active notebook after a late completion', async () => {
    fixture.rebuild.mockImplementation(async () => { fixture.notebook = 'other'; return {}; });
    await rebuildAction(DiagnosticsPanel({ notebookId: 'nb' }))!();
    expect(fixture.sources).not.toHaveBeenCalled();
    expect(fixture.stats).not.toHaveBeenCalled();
    expect(fixture.poll).not.toHaveBeenCalled();
  });
});
