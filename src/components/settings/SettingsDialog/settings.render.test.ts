import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createElement, isValidElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { SettingsDialog } from './index';
import { StatusBar } from '../../layout/StatusBar';
import * as api from '../../../lib/tauri';
import type {
  MemoryBackendStatus, SemanticMemoryBackfillReceipt, SemanticMemoryLinkStatus,
  SemanticMemoryProfileStatus, SemanticMemoryProjectionSummary,
} from '../../../lib/types';

const fixture = vi.hoisted(() => ({
  settings: {} as Record<string, any>, notebookId: null as string | null,
  health: {} as Record<string, any>, embeddingEvent: null as null | ((payload: { state: string; message: string }) => void),
  addToast: vi.fn(),
}));
// Match the existing layout hook harness, enabling it only for effect-driven
// diagnostic cases. The original server-rendered settings checks keep real hooks.
const hooks = vi.hoisted(() => ({
  enabled: false, slots: [] as any[], index: 0,
  effects: [] as { run: () => void | (() => void); deps?: readonly unknown[] }[],
  layouts: [] as (() => void)[],
}));
vi.mock('react', async original => {
  const actual = await original<typeof import('react')>();
  const sameDeps = (left?: readonly unknown[], right?: readonly unknown[]) => Boolean(
    left && right && left.length === right.length && left.every((value, index) => Object.is(value, right[index])),
  );
  return { ...actual,
    useState: (initial: any) => {
      if (!hooks.enabled) return actual.useState(initial);
      const index = hooks.index++;
      if (!(index in hooks.slots)) hooks.slots[index] = typeof initial === 'function' ? initial() : initial;
      return [hooks.slots[index], (value: any) => {
        hooks.slots[index] = typeof value === 'function' ? value(hooks.slots[index]) : value;
      }];
    },
    useRef: (initial: any) => {
      if (!hooks.enabled) return actual.useRef(initial);
      const index = hooks.index++;
      return hooks.slots[index] ??= { current: initial };
    },
    useCallback: (callback: any, deps: readonly unknown[]) => hooks.enabled ? callback : actual.useCallback(callback, deps),
    useMemo: (compute: any, deps: readonly unknown[]) => {
      if (!hooks.enabled) return actual.useMemo(compute, deps);
      const index = hooks.index++;
      const previous = hooks.slots[index];
      if (!previous || !sameDeps(previous.deps, deps)) hooks.slots[index] = { deps, value: compute() };
      return hooks.slots[index].value;
    },
    useLayoutEffect: (run: () => void | (() => void), deps?: readonly unknown[]) => {
      if (!hooks.enabled) return actual.useLayoutEffect(run, deps);
      const index = hooks.index++;
      const previous = hooks.slots[index];
      if (!previous || !sameDeps(previous.deps, deps)) hooks.layouts.push(() => {
        previous?.cleanup?.();
        hooks.slots[index] = { deps, cleanup: run() };
      });
    },
    useEffect: (run: () => void | (() => void), deps?: readonly unknown[]) => {
      if (hooks.enabled) hooks.effects.push({ run, deps });
      else actual.useEffect(run, deps);
    },
  };
});
vi.mock('../../../lib/useModalFocus', () => ({ useModalFocus: () => ({ current: null }) }));
vi.mock('../../../stores/settingsStore', () => ({
  useSettingsStore: (select?: (state: any) => unknown) => select ? select(fixture.settings) : fixture.settings,
  findSelectedModel: (models: any[], provider: string | null, id: string) => models.find(model => model.provider_id === provider && model.id === id),
}));
vi.mock('../../../stores/notebookStore', () => ({ useNotebookStore: (select: (state: any) => unknown) => select({ activeNotebookId: fixture.notebookId }) }));
vi.mock('../../../stores/sourceStore', () => ({ useSourceStore: (select: (state: any) => unknown) => select({ stats: null }) }));
vi.mock('../../../stores/toastStore', () => ({ useToastStore: { getState: () => ({ addToast: fixture.addToast }) } }));
vi.mock('../../../stores/healthStore', async original => ({
  ...await original<typeof import('../../../stores/healthStore')>(),
  useHealthStore: Object.assign((select: (state: any) => unknown) => select(fixture.health), { setState: vi.fn() }),
}));
vi.mock('../../../lib/events', () => ({ onEmbeddingModelStatus: (callback: typeof fixture.embeddingEvent) => {
  fixture.embeddingEvent = callback;
  return Promise.resolve(vi.fn());
} }));
vi.mock('../../../lib/tauri', () => ({
  memoryBackendStatus: vi.fn(), semanticMemoryLinkStatus: vi.fn(), getSemanticMemoryProfileStatus: vi.fn(),
  semanticMemoryBackfillNotebook: vi.fn(),
}));

const memory = (overrides: Partial<MemoryBackendStatus> = {}): MemoryBackendStatus => ({
  backend_id: 'semantic-memory-preview', default_backend: 'semantic-memory-preview',
  active_backend: 'semantic-memory-preview', backend_used: 'semantic-memory-preview', available: true,
  semantic_memory_feature_enabled: true, semantic_memory_available: true,
  index_sync_status: 'synced', sync_status: 'synced', degradation_markers: [],
  embedding_index_metadata: [], degraded: false, ...overrides,
});
const links = (notebookId: string, synced = 7): SemanticMemoryLinkStatus => ({
  notebook_id: notebookId, total_links: 9, synced_links: synced, stale_links: 0,
  failed_links: 0, missing_document_links: 0, degraded_links: 0, reason_codes: [],
});
const summary = (notebookId: string, overrides: Partial<SemanticMemoryProjectionSummary> = {}): SemanticMemoryProjectionSummary => ({
  notebook_id: notebookId, total_sources: 23, chunk_bearing_sources: 23, zero_chunk_sources: 0,
  projected_sources: 23, failed_sources: 0, skipped_no_chunks: 0, stale_sources: 0,
  partial_sources: 0, projecting_sources: 0, healthy_links: 23, degraded_links: 0,
  missing_links: 0, total_chunks: 29, projected_chunks: 29, projection_required: false,
  ...overrides,
});
const profile = (notebookId: string, digest = 'current-proof'): SemanticMemoryProfileStatus => ({
  compiled_semantic_memory: true, compiled_turbo_quant: true, experimental_enabled: true,
  semantic_memory_flag_enabled: true, turbo_quant_flag_enabled: true,
  selected_backend: 'semantic-memory-preview', effective_backend: 'semantic-memory-preview',
  fallback_allowed: false, strict_testing: true, projection_summary: summary(notebookId),
  turbo_quant_status: { compiled_turbo_quant: true, runtime_turbo_quant_enabled: true,
    vector_artifact_manifest_digest: digest, vector_artifact_missing_count: 0,
    vector_artifact_stale_count: 0, exact_rerank: true, exact_rerank_count: 8 },
  next_actions: [], blocking_reasons: [],
});
const backfillReceipt = (overrides: Partial<SemanticMemoryBackfillReceipt> = {}): SemanticMemoryBackfillReceipt => ({
  notebook_id: 'A', receipt_id: 'backfill-A', total_sources: 23, chunk_bearing_sources: 23,
  projected_sources: 23, skipped_no_chunks: 0, failed_sources: 0, stale_sources: 0,
  total_chunks: 29, projected_chunks: 29, errors: [], projection_summary: summary('A'), ...overrides,
});
function enableBackfill() {
  fixture.settings.featureFlags = [{
    id: 'feature_semantic_memory_preview_enabled', label: 'Semantic memory preview', section: 'Memory & Retrieval',
    description: '', enabled: true, active: true, available: true, stable: true, default_enabled: true, requires_experimental: false,
  }];
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
async function flush() { for (let i = 0; i < 5; i++) await Promise.resolve(); }
function renderSettings(open = true) {
  hooks.enabled = true; hooks.index = 0; hooks.effects = []; hooks.layouts = [];
  let tree;
  try { tree = SettingsDialog({ open, onClose: vi.fn() }); }
  finally { hooks.enabled = false; }
  hooks.layouts.forEach(commit => commit());
  return { tree, html: renderToStaticMarkup(tree) };
}
function startSettingsRead() {
  return hooks.effects.find(effect => effect.deps?.includes(fixture.settings.loadSettings))!.run();
}
function memoryCard(html: string) { return html.match(/<p[^>]*>Memory<\/p>(.*?)<\/div>/s)?.[1] ?? ''; }
function renderStatus() {
  hooks.enabled = true; hooks.index = 0; hooks.effects = [];
  let tree;
  try { tree = StatusBar(); }
  finally { hooks.enabled = false; }
  return { tree, html: renderToStaticMarkup(tree) };
}
function find(node: ReactNode, predicate: (props: any) => boolean): any {
  if (Array.isArray(node)) return node.map(child => find(child, predicate)).find(Boolean);
  if (isValidElement(node)) {
    const props = node.props as any;
    return predicate(props) ? node : find(props.children, predicate);
  }
}
function button(tree: ReactNode, label: string) {
  const found = find(tree, props => props.onClick && Array.isArray(props.children)
    && props.children.some((child: unknown) => typeof child === 'string' && child.trim() === label));
  expect(found).toBeTruthy();
  return found;
}

beforeEach(() => {
  vi.resetAllMocks();
  hooks.enabled = false; hooks.slots = []; hooks.index = 0; hooks.effects = []; hooks.layouts = [];
  fixture.notebookId = null; fixture.embeddingEvent = null;
  fixture.settings = { models: [], settings: {}, featureFlags: [], providers: [], activeModel: '', externalTools: {},
    loadSettings: vi.fn(), loadProviders: vi.fn(), loadModels: vi.fn(), loadFeatureFlags: vi.fn(), loadExternalTools: vi.fn(),
  };
  fixture.health = { memoryStatus: null, memoryState: 'checking', memoryError: null,
    queueStatus: null, profileStatus: null, chatConnected: null, backgroundConnected: null,
    startPolling: vi.fn(), stopPolling: vi.fn(),
  };
  vi.mocked(api.memoryBackendStatus).mockResolvedValue(memory());
  vi.mocked(api.semanticMemoryLinkStatus).mockResolvedValue(null as never);
  vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValue(null as never);
});

describe('settings memory diagnostic reads', () => {
  it('shows the saved request and unknown effective backend while a read is pending', async () => {
    fixture.settings.settings = { memory_backend: 'semantic-memory-preview' };
    const pending = deferred<MemoryBackendStatus>();
    vi.mocked(api.memoryBackendStatus).mockReturnValue(pending.promise);
    renderSettings(); startSettingsRead();
    const card = memoryCard(renderSettings().html);
    expect(card).toContain('checking');
    expect(card).toContain('Requested semantic-memory-preview; used unknown');
    expect(card).not.toContain('gloss-local');
    pending.resolve(memory()); await flush();
    expect(memoryCard(renderSettings().html)).toContain('used semantic-memory-preview');
  });

  it('shows a bounded read failure without inventing local health, and clears it after retry', async () => {
    fixture.settings.settings = { memory_backend: 'semantic-memory-preview' };
    const error = 'memory read locked: ' + 'x'.repeat(800);
    vi.mocked(api.memoryBackendStatus).mockRejectedValueOnce(new Error(error));
    renderSettings(); startSettingsRead(); await flush();
    const failed = renderSettings();
    expect(memoryCard(failed.html)).toContain('unavailable');
    expect(memoryCard(failed.html)).toContain(error.slice(0, 500));
    expect(memoryCard(failed.html)).not.toContain('gloss-local');
    const retry = find(failed.tree, props => props.onClick && Array.isArray(props.children)
      && props.children.some((child: unknown) => typeof child === 'string' && child.trim() === 'Refresh evidence'));
    expect(retry).toBeTruthy();
    await retry.props.onClick();
    const recovered = memoryCard(renderSettings().html);
    expect(recovered).toContain('used semantic-memory-preview');
    expect(recovered).not.toContain('memory read locked');
  });

  it('renders actual fallback and reason separately from the requested/default backend', async () => {
    const fallback = memory({ backend_used: 'gloss-local', degraded: true, fallback_reason: 'preview index unavailable' });
    vi.mocked(api.memoryBackendStatus).mockResolvedValue(fallback);
    renderSettings(); startSettingsRead(); await flush();
    const { html } = renderSettings();
    expect(memoryCard(html)).toContain('Requested semantic-memory-preview; used gloss-local; fallback disclosed; default semantic-memory-preview');
    expect(html).toContain('preview index unavailable');
  });

  it('labels returned backend unavailability without changing its identity', async () => {
    vi.mocked(api.memoryBackendStatus).mockResolvedValue(memory({ available: false }));
    renderSettings(); startSettingsRead(); await flush();
    expect(memoryCard(renderSettings().html)).toContain('semantic-memory-preview (unavailable)');
  });

  it.each(['success', 'error'] as const)('rejects a late %s from the previous notebook', async outcome => {
    const old = deferred<MemoryBackendStatus>();
    fixture.notebookId = 'old';
    vi.mocked(api.memoryBackendStatus).mockReturnValueOnce(old.promise).mockResolvedValue(memory());
    renderSettings(); const cleanup = startSettingsRead();
    if (typeof cleanup === 'function') cleanup();
    fixture.notebookId = 'new';
    renderSettings(); startSettingsRead(); await flush();
    if (outcome === 'error') old.reject(new Error('old notebook failure'));
    else old.resolve(memory({ backend_used: 'gloss-local', degraded: true }));
    await flush();
    const card = memoryCard(renderSettings().html);
    expect(card).toContain('used semantic-memory-preview');
    expect(card).not.toContain('gloss-local');
    expect(card).not.toContain('old notebook failure');
  });

  describe.each(['links', 'profile'] as const)('retired %s evidence', read => {
    it.each([
      ['notebook switch', 'success'], ['notebook switch', 'error'],
      ['close/reopen', 'success'], ['close/reopen', 'error'],
      ['close/switch/reopen', 'success'], ['close/switch/reopen', 'error'],
      ['overlapping refresh', 'success'], ['overlapping refresh', 'error'],
    ] as const)('preserves current counts/proof after %s and late %s', async (transition, outcome) => {
      fixture.notebookId = 'A';
      const old = deferred<SemanticMemoryLinkStatus | SemanticMemoryProfileStatus>();
      vi.mocked(api.semanticMemoryLinkStatus).mockResolvedValue(links('A'));
      vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValue(profile('A'));
      if (read === 'links') {
        vi.mocked(api.semanticMemoryLinkStatus).mockReturnValueOnce(old.promise as Promise<SemanticMemoryLinkStatus>);
      } else {
        vi.mocked(api.getSemanticMemoryProfileStatus).mockReturnValueOnce(old.promise as Promise<SemanticMemoryProfileStatus>);
      }
      renderSettings(); const cleanup = startSettingsRead(); await flush();
      expect(api.getSemanticMemoryProfileStatus).toHaveBeenCalledTimes(read === 'links' ? 0 : 1);

      const currentNotebook = transition.includes('switch') ? 'B' : 'A';
      vi.mocked(api.semanticMemoryLinkStatus).mockResolvedValue(links(currentNotebook));
      vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValue(profile(currentNotebook));
      if (transition === 'overlapping refresh') {
        await button(renderSettings().tree, 'Refresh evidence').props.onClick();
      } else {
        if (typeof cleanup === 'function') cleanup();
        if (transition.startsWith('close')) { renderSettings(false); startSettingsRead(); }
        fixture.notebookId = currentNotebook;
        renderSettings(); startSettingsRead(); await flush();
      }
      const before = renderSettings().html;
      expect(before).toContain('7/9 semantic links synced');
      expect(before).toContain('Sources 23');
      expect(before).toContain('current-proof');
      const profileReads = vi.mocked(api.getSemanticMemoryProfileStatus).mock.calls.length;
      if (outcome === 'error') old.reject(new Error(`retired ${read} failure`));
      else old.resolve(read === 'links' ? links('A', 1) : profile('A', 'retired-proof'));
      await flush();

      const after = renderSettings().html;
      expect(after).toContain('7/9 semantic links synced');
      expect(after).toContain('Sources 23');
      expect(after).toContain('current-proof');
      expect(after).not.toContain('1/9 semantic links synced');
      expect(after).not.toContain('retired-proof');
      expect(after).not.toContain(`retired ${read} failure`);
      // The retired link read must not launch its sequential profile continuation.
      expect(api.getSemanticMemoryProfileStatus).toHaveBeenCalledTimes(profileReads);
      expect(api.semanticMemoryLinkStatus).toHaveBeenLastCalledWith(currentNotebook);
      expect(api.getSemanticMemoryProfileStatus).toHaveBeenLastCalledWith(currentNotebook, { kind: 'all' });
    });
  });

  it.each(['switch', 'close', 'no notebook'] as const)('clears completed notebook evidence on %s before the next read settles', async transition => {
    fixture.notebookId = 'A';
    vi.mocked(api.semanticMemoryLinkStatus).mockResolvedValue(links('A', 1));
    vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValue(profile('A', 'retired-proof'));
    renderSettings(); const cleanup = startSettingsRead(); await flush();
    expect(renderSettings().html).toContain('retired-proof');
    if (typeof cleanup === 'function') cleanup();
    fixture.notebookId = transition === 'no notebook' ? null : transition === 'switch' ? 'B' : 'A';
    const next = deferred<MemoryBackendStatus>();
    vi.mocked(api.memoryBackendStatus).mockReturnValue(next.promise);
    renderSettings(transition !== 'close'); startSettingsRead();
    // The hook harness commits layout clearing; this render reads the committed state.
    const html = renderSettings(transition !== 'close').html;
    expect(html).not.toContain('1/9 semantic links synced');
    expect(html).not.toContain('retired-proof');
    expect(html).not.toContain('Sources 23');
    next.resolve(memory()); await flush();
  });

  it.each([
    ['success', 'pending'], ['error', 'pending'],
    ['success', 'completed'], ['error', 'completed'],
  ] as const)('rejects old action %s after close/switch/reopen while the new read is %s', async (outcome, readState) => {
    fixture.notebookId = 'A';
    fixture.settings.settings = { memory_backend: 'semantic-memory-preview' };
    fixture.settings.featureFlags = [{
      id: 'feature_semantic_memory_preview_enabled', label: 'Semantic memory preview', section: 'Memory & Retrieval',
      description: '', enabled: true, active: true, available: true, stable: true, default_enabled: true, requires_experimental: false,
    }];
    const oldAction = deferred<Awaited<ReturnType<typeof api.semanticMemoryBackfillNotebook>>>();
    const newRead = deferred<MemoryBackendStatus>();
    vi.mocked(api.semanticMemoryBackfillNotebook).mockReturnValue(oldAction.promise);
    vi.mocked(api.memoryBackendStatus).mockResolvedValueOnce(memory()).mockReturnValueOnce(newRead.promise)
      .mockRejectedValue(new Error('stale A memory read'));
    renderSettings(); const cleanupA = startSettingsRead(); await flush();
    const oldDialog = renderSettings();
    const backfill = find(oldDialog.tree, props => props.onClick && Array.isArray(props.children)
      && props.children.some((child: unknown) => typeof child === 'string' && child.trim() === 'Run projection backfill'));
    expect(backfill.props.disabled).toBe(false);
    const continuation = backfill.props.onClick();
    expect(api.semanticMemoryBackfillNotebook).toHaveBeenCalledExactlyOnceWith('A');

    // Keep the component mounted through both the closed and switched renders.
    if (typeof cleanupA === 'function') cleanupA();
    renderSettings(false); startSettingsRead();
    fixture.notebookId = 'B'; renderSettings(false); startSettingsRead();
    renderSettings(); startSettingsRead();
    if (readState === 'completed') { newRead.resolve(memory()); await flush(); }
    const before = memoryCard(renderSettings().html);
    expect(before).toContain(readState === 'pending' ? 'checking' : 'used semantic-memory-preview');
    const linkReads = vi.mocked(api.semanticMemoryLinkStatus).mock.calls.length;
    const profileReads = vi.mocked(api.getSemanticMemoryProfileStatus).mock.calls.length;

    if (outcome === 'error') oldAction.reject(new Error('A backfill failed'));
    else oldAction.resolve({ projected_sources: 1, skipped_no_chunks: 0, failed_sources: 0 } as Awaited<ReturnType<typeof api.semanticMemoryBackfillNotebook>>);
    await continuation;
    expect(api.memoryBackendStatus).toHaveBeenCalledTimes(2);
    expect(api.memoryBackendStatus).toHaveBeenNthCalledWith(1, 'A');
    expect(api.memoryBackendStatus).toHaveBeenNthCalledWith(2, 'B');
    expect(memoryCard(renderSettings().html)).toBe(before);
    // A rejected helper entry also stops that stale evidence-refresh chain.
    expect(api.semanticMemoryLinkStatus).toHaveBeenCalledTimes(linkReads);
    expect(api.getSemanticMemoryProfileStatus).toHaveBeenCalledTimes(profileReads);
    expect(fixture.addToast).not.toHaveBeenCalled();
    if (readState === 'pending') { newRead.resolve(memory()); await flush(); }
    const final = memoryCard(renderSettings().html);
    expect(final).toContain('used semantic-memory-preview');
    expect(final).not.toContain('stale A memory read');
  });
});

describe('settings projection backfill outcome', () => {
  it.each([
    ['failed', { failed_sources: 1 }],
    ['stale', { stale_sources: 1 }],
    ['partial', { projection_summary: summary('A', { partial_sources: 1 }) }],
    ['projecting', { projection_summary: summary('A', { projecting_sources: 1 }) }],
    ['projection required', { projection_summary: summary('A', { projection_required: true }) }],
    ['source error', { errors: [{ source_id: 'source-7', title: 'Broken source', error: 'embedding failed' }] }],
    ['healthy', {}],
  ] as [string, Partial<SemanticMemoryBackfillReceipt>][])('uses the actual %s receipt for its toast', async (state, overrides) => {
    fixture.notebookId = 'A'; enableBackfill();
    vi.mocked(api.semanticMemoryBackfillNotebook).mockResolvedValue(backfillReceipt(overrides));
    renderSettings(); startSettingsRead(); await flush();
    const backfill = button(renderSettings().tree, 'Run projection backfill');
    expect(backfill.props.disabled).toBe(false);
    await backfill.props.onClick();
    expect(fixture.addToast).toHaveBeenCalledTimes(1);
    const toast = fixture.addToast.mock.calls[0][0];
    expect(toast.type).toBe(state === 'healthy' ? 'success' : 'warning');
    expect(toast.title).toBe(state === 'healthy' ? 'Projection backfill complete' : 'Projection backfill needs attention');
    expect(toast.message).toContain('23 fully projected');
    expect(toast.message).toContain('29 chunks projected');
    if (state === 'source error') expect(toast.message).toContain('Broken source (source-7): embedding failed');
  });

  it('reports an invocation failure as an error', async () => {
    fixture.notebookId = 'A'; enableBackfill();
    vi.mocked(api.semanticMemoryBackfillNotebook).mockRejectedValue(new Error('backfill unavailable'));
    renderSettings(); startSettingsRead(); await flush();
    await button(renderSettings().tree, 'Run projection backfill').props.onClick();
    expect(fixture.addToast).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
      type: 'error', title: 'semantic-memory reindex failed', message: 'backfill unavailable',
    }));
  });

  it('does not announce a backfill whose evidence refresh was replaced', async () => {
    fixture.notebookId = 'A'; enableBackfill();
    const oldLinks = deferred<SemanticMemoryLinkStatus>();
    vi.mocked(api.semanticMemoryBackfillNotebook).mockResolvedValue(backfillReceipt());
    vi.mocked(api.semanticMemoryLinkStatus).mockResolvedValueOnce(links('A'))
      .mockReturnValueOnce(oldLinks.promise).mockResolvedValue(links('A'));
    vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValue(profile('A'));
    renderSettings(); startSettingsRead(); await flush();
    const completion = button(renderSettings().tree, 'Run projection backfill').props.onClick();
    await flush();
    await button(renderSettings().tree, 'Refresh evidence').props.onClick();
    const profileReads = vi.mocked(api.getSemanticMemoryProfileStatus).mock.calls.length;
    oldLinks.resolve(links('A', 1)); await completion;
    expect(fixture.addToast).not.toHaveBeenCalled();
    expect(api.getSemanticMemoryProfileStatus).toHaveBeenCalledTimes(profileReads);
    expect(renderSettings().html).toContain('7/9 semantic links synced');
    expect(renderSettings().html).toContain('current-proof');
  });

  it('does not announce a backfill when the dialog closes after its evidence completes', async () => {
    fixture.notebookId = 'A'; enableBackfill();
    const pendingProfile = deferred<SemanticMemoryProfileStatus>();
    vi.mocked(api.semanticMemoryBackfillNotebook).mockResolvedValue(backfillReceipt());
    vi.mocked(api.semanticMemoryLinkStatus).mockResolvedValue(links('A'));
    vi.mocked(api.getSemanticMemoryProfileStatus).mockResolvedValueOnce(profile('A'))
      .mockReturnValueOnce(pendingProfile.promise);
    renderSettings(); startSettingsRead(); await flush();
    const completion = button(renderSettings().tree, 'Run projection backfill').props.onClick();
    await flush();
    expect(api.getSemanticMemoryProfileStatus).toHaveBeenCalledTimes(2);
    // Register after the refresh awaits this promise: the refresh publishes first,
    // then this close commits before the backfill handler resumes from its await.
    const close = pendingProfile.promise.then(() => { renderSettings(false); startSettingsRead(); });
    pendingProfile.resolve(profile('A'));
    await close; await completion;
    expect(fixture.addToast).not.toHaveBeenCalled();
  });
});

describe('status bar diagnostic projections', () => {
  it('keeps the requested preview visible during checking, with unknown effective/default health', () => {
    fixture.settings.settings = { memory_backend: 'semantic-memory-preview' };
    const first = renderStatus();
    expect(first.html).toContain('Memory: checking (requested semantic-memory-preview)');
    find(first.tree, props => props['aria-label'] === 'Health status').props.onClick();
    const html = renderStatus().html;
    expect(html).toMatch(/Backend used<\/span><span[^>]*>unknown/);
    expect(html).toMatch(/Default backend<\/span><span[^>]*>unknown/);
    expect(html).not.toContain('gloss-local');
  });

  it('surfaces read errors without claiming a fallback', () => {
    fixture.settings.settings = { memory_backend: 'semantic-memory-preview' };
    fixture.health.memoryState = 'unavailable'; fixture.health.memoryError = 'memory IPC unavailable';
    const first = renderStatus();
    expect(first.html).toContain('Memory: unavailable (requested semantic-memory-preview)');
    expect(first.html).toContain('title="memory IPC unavailable"');
    find(first.tree, props => props['aria-label'] === 'Health status').props.onClick();
    expect(renderStatus().html).toContain('memory IPC unavailable');
  });

  it('shows successful preview and actual local fallback from returned status', () => {
    fixture.health.memoryState = 'ready'; fixture.health.memoryStatus = memory();
    expect(renderStatus().html).toContain('Memory: semantic-memory-preview');
    fixture.health.memoryStatus = memory({ backend_used: 'gloss-local', degraded: true, fallback_reason: 'index stale' });
    const fallback = renderStatus();
    expect(fallback.html).toContain('Memory: gloss-local fallback');
    expect(fallback.html).toContain('title="index stale"');
  });

  it('retains emitted downloading/loading/error reasons and clears only on ready', () => {
    renderStatus(); hooks.effects.find(effect => effect.deps?.length === 0)!.run();
    // Raw event fixtures cover the canonical state union, including loading/error.
    // No native emitter is run by this projection test.
    for (const state of ['downloading', 'loading', 'error']) {
      fixture.embeddingEvent!({ state, message: `${state} reason` });
      expect(renderStatus().html).toContain(`Embedding ${state}: ${state} reason`);
    }
    fixture.embeddingEvent!({ state: 'error', message: 'x'.repeat(800) });
    const bounded = renderStatus().html;
    expect(bounded).toContain('x'.repeat(500));
    expect(bounded).not.toContain('x'.repeat(501));
    fixture.embeddingEvent!({ state: 'ready', message: 'ready' });
    expect(renderStatus().html).not.toContain('Embedding error:');
  });
});

describe('settings rendered controls', () => {
  it('offers only available Ollama background models and exposes incompatible chat default', () => {
    fixture.settings.settings = { default_provider: 'openai', summary_model: 'old-model' };
    fixture.settings.activeModel = 'cloud-model';
    fixture.settings.providers = [{ id: 'ollama', enabled: true }, { id: 'openai', enabled: true }];
    fixture.settings.models = [
      { id: 'local', provider_id: 'ollama', display_name: 'Local', available: true, stale: false },
      { id: 'remote', provider_id: 'openai', display_name: 'Remote', available: true, stale: false },
      { id: 'stale', provider_id: 'ollama', display_name: 'Stale', available: true, stale: true },
    ];
    const html = renderToStaticMarkup(createElement(SettingsDialog, { open: true, onClose: vi.fn() }));
    const summary = html.match(/<select[^>]*aria-label="summary model"[^>]*>(.*?)<\/select>/s)?.[1];
    expect(summary).toContain('value="local"');
    expect(summary).not.toContain('value="remote"');
    expect(summary).not.toContain('value="stale"');
    expect(summary).toContain('old-model — unavailable Ollama model');
    expect(summary).toContain('requires Ollama');
  });

  it('names explicit embedding Apply and removes the false automatic fallback promise', () => {
    const html = renderToStaticMarkup(createElement(SettingsDialog, { open: true, onClose: vi.fn() }));
    expect(html).toContain('Apply embedding and ingestion settings');
    expect(html).toContain('Gloss uses exactly the selected embedding backend');
    expect(html).not.toContain('Gloss automatically falls back');
  });

  it('renders the default Chat temperature and explicit Apply control in the dialog', () => {
    const html = renderToStaticMarkup(createElement(SettingsDialog, { open: true, onClose: vi.fn() }));
    const input = html.match(/<input[^>]*aria-label="Chat temperature"[^>]*>/)?.[0];
    expect(input).toContain('type="number"');
    expect(input).toContain('min="0"');
    expect(input).toContain('max="2"');
    expect(input).toContain('value="0.7"');
    expect(input).not.toContain('disabled=');
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Apply chat temperature<\/button>/);
    expect(html).toContain('Default temperature');
  });

  it('shows a persisted zero after reopening Settings', () => {
    fixture.settings.settings = { default_provider: 'ollama', generation_temperature: '0' };
    const html = renderToStaticMarkup(createElement(SettingsDialog, { open: true, onClose: vi.fn() }));
    expect(html.match(/<input[^>]*aria-label="Chat temperature"[^>]*>/)?.[0]).toContain('value="0"');
    expect(html).toContain('Saved temperature');
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Apply chat temperature<\/button>/);
  });

  it('disables Anthropic temperature without claiming the saved preference is applied', () => {
    fixture.settings.settings = { default_provider: 'anthropic', generation_temperature: '0' };
    const html = renderToStaticMarkup(createElement(SettingsDialog, { open: true, onClose: vi.fn() }));
    expect(html.match(/<input[^>]*aria-label="Chat temperature"[^>]*>/)?.[0]).toContain('disabled=""');
    expect(html).toContain('Anthropic uses provider-managed temperature. This setting is not applied to Anthropic replies.');
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Apply chat temperature<\/button>/);
  });
});
