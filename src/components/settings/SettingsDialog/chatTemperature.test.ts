import { isValidElement, type ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatTemperatureControl } from './ChatTemperatureControl';
import { useSettingsStore } from '../../../stores/settingsStore';
import * as api from '../../../lib/tauri';

const fixture = vi.hoisted(() => ({ state: [] as unknown[], index: 0 }));

// Run the actual control callbacks and settings acknowledgement queue without
// requiring a browser. Static rendering and the native workflow cover the UI.
vi.mock('react', async (importOriginal) => ({
  ...await importOriginal<typeof import('react')>(),
  useState: (initial: unknown) => {
    const index = fixture.index++;
    if (!(index in fixture.state)) fixture.state[index] = initial;
    return [fixture.state[index], (value: unknown) => {
      fixture.state[index] = typeof value === 'function' ? value(fixture.state[index]) : value;
    }];
  },
}));
vi.mock('../../../stores/settingsStore', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../../stores/settingsStore')>();
  return {
    ...original,
    useSettingsStore: Object.assign(() => original.useSettingsStore.getState(), original.useSettingsStore),
  };
});
vi.mock('../../../lib/tauri', () => ({ updateSetting: vi.fn() }));
vi.mock('../../../stores/toastStore', () => ({ useToastStore: { getState: () => ({ addToast: vi.fn() }) } }));

function renderControl(providerId = 'ollama') {
  fixture.index = 0;
  return ChatTemperatureControl({ providerId });
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

function input(node: ReactNode) {
  const result = findProps(node, (props) => props['aria-label'] === 'Chat temperature');
  expect(result).toBeDefined();
  return result!;
}

function button(node: ReactNode, label = 'Apply chat temperature') {
  const result = findProps(node, (props) => props.children === label);
  expect(result).toBeDefined();
  return result!;
}

function enter(value: string, providerId = 'ollama') {
  (input(renderControl(providerId)).onChange as (event: { target: { value: string } }) => void)({ target: { value } });
}

function apply(providerId = 'ollama') {
  return (button(renderControl(providerId)).onClick as () => Promise<void>)();
}

beforeEach(() => {
  fixture.state = [];
  fixture.index = 0;
  vi.clearAllMocks();
  vi.mocked(api.updateSetting).mockResolvedValue(undefined);
  useSettingsStore.setState({ settings: { generation_temperature: '0.7' } });
});

describe('chat temperature settings acknowledgement', () => {
  it('persists zero through the real queue and shows Apply disabled only after acknowledgement', async () => {
    let acknowledge!: () => void;
    vi.mocked(api.updateSetting).mockImplementationOnce(() => new Promise<void>((resolve) => { acknowledge = resolve; }));
    enter('0');
    expect(button(renderControl()).disabled).toBe(false);
    const saving = apply();
    await Promise.resolve();
    expect(api.updateSetting).toHaveBeenCalledExactlyOnceWith('generation_temperature', '0');
    expect(useSettingsStore.getState().settings.generation_temperature).toBe('0.7');
    const pending = renderControl();
    expect(input(pending).disabled).toBe(true);
    expect(input(pending).value).toBe('0');
    expect(button(pending, 'Saving…').disabled).toBe(true);

    acknowledge();
    await saving;
    expect(useSettingsStore.getState().settings.generation_temperature).toBe('0');
    const saved = renderControl();
    expect(input(saved).value).toBe('0');
    expect(input(saved).disabled).toBe(false);
    expect(button(saved).disabled).toBe(true);
    // Reopening mounts fresh local draft state while reading the saved setting.
    fixture.state = [];
    expect(input(renderControl()).value).toBe('0');
  });

  it('preserves an unsaved draft across unrelated settings reloads and provider changes', () => {
    enter('0.25');
    useSettingsStore.setState({ settings: { generation_temperature: '0.7', summary_model: 'reloaded-model' } });
    expect(input(renderControl()).value).toBe('0.25');
    expect(button(renderControl()).disabled).toBe(false);
    expect(input(renderControl('anthropic')).value).toBe('0.25');
    expect(input(renderControl('anthropic')).disabled).toBe(true);
    expect(input(renderControl('openai')).value).toBe('0.25');
    expect(api.updateSetting).not.toHaveBeenCalled();
  });

  it('keeps failed input and a visible error through reload so the same value can be retried', async () => {
    vi.mocked(api.updateSetting).mockRejectedValueOnce(new Error('disk full'));
    enter('0.1');
    await apply();
    expect(useSettingsStore.getState().settings.generation_temperature).toBe('0.7');
    useSettingsStore.setState({ settings: { generation_temperature: '0.7', default_model: 'reloaded-model' } });
    const failed = renderControl();
    expect(input(failed).value).toBe('0.1');
    expect(button(failed).disabled).toBe(false);
    expect(findProps(failed, (props) => props.role === 'alert')?.children).toContain('disk full');
    await apply();
    expect(vi.mocked(api.updateSetting).mock.calls).toEqual([
      ['generation_temperature', '0.1'], ['generation_temperature', '0.1'],
    ]);
    expect(useSettingsStore.getState().settings.generation_temperature).toBe('0.1');
    expect(button(renderControl()).disabled).toBe(true);
    expect(findProps(renderControl(), (props) => props.role === 'alert')).toBeUndefined();
  });

  it.each(['', ' ', 'NaN', 'Infinity', '-0.1', '2.1', 'not a number'])('rejects invalid finite-range input %j without persistence', async (value) => {
    enter(value);
    const invalid = renderControl();
    expect(input(invalid)['aria-invalid']).toBe(true);
    expect(button(invalid).disabled).toBe(true);
    expect(findProps(invalid, (props) => props.role === 'alert')?.children).toBe('Enter a number from 0 to 2.');
    await apply();
    expect(api.updateSetting).not.toHaveBeenCalled();
  });

  it.each(['0', '2'])('accepts the inclusive boundary %s', async (value) => {
    enter(value);
    await apply();
    expect(api.updateSetting).toHaveBeenCalledExactlyOnceWith('generation_temperature', value);
  });

  it.each(['anthropic', 'unknown'])('does not save a previously valid draft through unsupported provider %s', async (providerId) => {
    enter('0');
    const unsupported = renderControl(providerId);
    expect(input(unsupported).disabled).toBe(true);
    expect(button(unsupported).disabled).toBe(true);
    await apply(providerId);
    expect(api.updateSetting).not.toHaveBeenCalled();
    expect(input(renderControl()).value).toBe('0');
  });

  it('follows acknowledged settings updates when there is no unsaved draft', () => {
    expect(input(renderControl()).value).toBe('0.7');
    useSettingsStore.setState({ settings: { generation_temperature: '0.2' } });
    expect(input(renderControl()).value).toBe('0.2');
    expect(button(renderControl()).disabled).toBe(true);
  });
});
