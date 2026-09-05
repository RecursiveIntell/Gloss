import { describe, expect, it } from 'vitest';
import { capturedModelLabel, shouldSubmitChat } from './ChatPanel';
import { findSelectedModel } from '../../stores/settingsStore';
import type { Message, ModelRecord } from '../../lib/types';

describe('captured answer identity', () => {
  it('uses the persisted model without consulting current settings', () => {
    expect(capturedModelLabel({ model_used: 'historic-model' } as Message)).toBe('historic-model');
    expect(capturedModelLabel({} as Message)).toBe('Model not captured');
  });
  it('uses the generation receipt provider only when it agrees with captured model identity', () => {
    const message = { model_used: 'historic', citations: { evidence: { generation_receipt: { model: 'historic', provider: 'ollama' } } } } as Message;
    expect(capturedModelLabel(message)).toBe('historic · ollama');
    message.citations!.evidence.generation_receipt!.model = 'different';
    expect(capturedModelLabel(message)).toBe('historic');
  });
  it('can render a receipt-only capture without inventing a current model', () => {
    expect(capturedModelLabel({ citations: { evidence: { generation_receipt: { model: 'recorded', provider: 'provider' } } } } as Message)).toBe('recorded · provider');
  });
});

describe('exact provider model selection', () => {
  const models = [{ id: 'shared', provider_id: 'remote', available: true } as ModelRecord];
  it('does not borrow a same-ID model from a different provider', () => {
    expect(findSelectedModel(models, 'local', 'shared')).toBeUndefined();
    expect(findSelectedModel(models, null, 'shared')).toBeUndefined();
    expect(findSelectedModel(models, 'remote', 'shared')).toBe(models[0]);
  });
});

describe('chat composer submit keys', () => {
  it('sends on ordinary Enter but preserves newlines and input method composition', () => {
    expect(shouldSubmitChat('Enter', false, false, 13)).toBe(true);
    expect(shouldSubmitChat('Enter', true, false, 13)).toBe(false);
    expect(shouldSubmitChat('Enter', false, true, 13)).toBe(false);
    expect(shouldSubmitChat('Enter', false, false, 229)).toBe(false);
    expect(shouldSubmitChat('a', false, false, 65)).toBe(false);
  });
});
