import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { SettingsDialog } from './index';

const fixture = vi.hoisted(() => ({ settings: {} as Record<string, unknown> }));
vi.mock('../../../stores/settingsStore', () => ({ useSettingsStore: () => fixture.settings }));
vi.mock('../../../stores/notebookStore', () => ({ useNotebookStore: (select: (state: unknown) => unknown) => select({ activeNotebookId: null }) }));

beforeEach(() => {
  fixture.settings = { models: [], settings: {}, featureFlags: [], providers: [], activeModel: '', externalTools: {} };
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
});
