import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChatEvidenceDisclosure, DecodingSettingsReceiptV1 } from '../../lib/types';
import { useSettingsStore } from '../../stores/settingsStore';
import { EvidenceDrawer } from './ChatPanel';

vi.mock('../../stores/settingsStore', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../stores/settingsStore')>();
  // SSR normally reads Zustand's initial snapshot. Expose the current saved
  // setting so a regression that substitutes it for captured evidence is caught.
  return { ...actual, useSettingsStore: Object.assign(
    (select: (state: ReturnType<typeof actual.useSettingsStore.getState>) => unknown) => select(actual.useSettingsStore.getState()),
    actual.useSettingsStore,
  ) };
});

const initialSettings = useSettingsStore.getState().settings;
afterEach(() => useSettingsStore.setState({ settings: initialSettings }));

function evidenceWith(receipt?: DecodingSettingsReceiptV1 | null): ChatEvidenceDisclosure {
  const decision: ChatEvidenceDisclosure['retrieval_capability_decision'] = {
    requested_backend: 'gloss-local',
    effective_backend: 'gloss-local',
    build_feature_available: true,
    runtime_enabled: true,
    projection_ready: true,
    dense_ready: false,
    fallback_allowed: false,
    degraded: false,
  };
  return {
    backend_requested: 'gloss-local',
    backend_used: 'gloss-local',
    retrieval_mode: 'none',
    fallback_used: false,
    degradation_markers: [],
    source_scope_mode: 'none',
    requested_source_ids: [],
    selected_source_ids: [],
    effective_source_ids: [],
    invalid_source_ids: [],
    excluded_source_ids: [],
    invalid_source_count: 0,
    effective_source_count: 0,
    excluded_source_count: 0,
    context_passage_count: 0,
    citation_valid_count: 0,
    citation_invalid_count: 0,
    citation_anchors: [],
    citation_filter_reasons: [],
    omitted_candidate_count: 0,
    source_scope_preserved: true,
    index_status: 'not requested',
    link_status: 'not requested',
    receipt_id: 'saved-evidence',
    context_digest: 'context-digest',
    source_context_digest: 'source-digest',
    retrieval_capability_decision: decision,
    semantic_memory_runtime_truth: {
      schema: 'SemanticMemoryRuntimeTruthV1',
      receipt_id: 'saved-runtime-truth',
      build: {},
      settings: {},
      projection: {},
      decision,
    },
    decoding_settings_receipt: receipt,
  };
}

function decodingReceipt(supportsTemperature: boolean, effective: number): DecodingSettingsReceiptV1 {
  return {
    schema: 'DecodingSettingsReceiptV1',
    receipt_id: 'saved-decoding',
    provider: supportsTemperature ? 'ollama' : 'anthropic',
    model: 'historical-model',
    requested: { temperature: 0.9 },
    effective: { temperature: effective, max_tokens: 1024 },
    unsupported_fields: supportsTemperature ? [] : ['temperature'],
    provider_capability: { supports_temperature: supportsTemperature },
    recorded_at: '2026-09-05T00:00:00Z',
  };
}

function renderEvidence(receipt?: DecodingSettingsReceiptV1 | null): string {
  return renderToStaticMarkup(createElement(EvidenceDrawer, {
    id: 'saved-answer-evidence',
    evidence: evidenceWith(receipt),
  }));
}

function temperatureValue(html: string): string | undefined {
  return html.match(/Temperature: <\/span><span[^>]*>([^<]*)<\/span>/)?.[1];
}

describe('captured decoding evidence in the answer drawer', () => {
  it('shows provider default when the receipt says temperature is unsupported, even with a recorded numeric effective value', () => {
    const html = renderEvidence(decodingReceipt(false, 0.7));
    expect(temperatureValue(html)).toBe('Provider default');
    expect(html).not.toContain('>0.7<');
    expect(html).not.toContain('>0.9<');
  });

  it('shows the captured effective temperature instead of the requested or currently saved setting', () => {
    useSettingsStore.setState({ settings: { generation_temperature: '0.15' } });
    const html = renderEvidence(decodingReceipt(true, 0.35));
    expect(temperatureValue(html)).toBe('0.35');
    expect(html).not.toContain('Provider default');
    expect(html).not.toContain('>0.9<');
    expect(html).not.toContain('>0.15<');
  });

  it.each([undefined, null])('omits temperature when the decoding receipt is %s', (receipt) => {
    const html = renderEvidence(receipt);
    expect(html).toContain('Answer evidence');
    expect(html).not.toContain('Temperature:');
    expect(html).not.toContain('Provider default');
  });
});
