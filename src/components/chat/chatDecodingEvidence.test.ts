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

describe('captured dense availability and stored inventory', () => {
  function renderCoverage(available?: boolean): string {
    const evidence = evidenceWith();
    evidence.retrieval_outcome = {
      mode: 'bm25_only', results: [], degraded: true,
      engines: available === undefined ? [] : [{
        engine: 'native_dense_hnsw', attempted: available, available,
        contributed: false, candidate_count: 0, elapsed_ms: 0,
        reason_code: available ? undefined : 'embedding_index_metadata_stale',
      }],
      coverage: {
        selected_sources: 1, total_chunks: 1, fts_indexed_chunks: 1,
        embedded_chunks: 1, missing_embeddings: 0, dense_coverage_ratio: 1,
        semantic_links_total: 0, semantic_links_healthy: 0, semantic_links_degraded: 0,
      },
      fallback_chain: ['embedding_index_metadata_stale'],
      user_visible_summary: 'BM25 with stale dense metadata', trace_ref: 'captured-trace',
    };
    return renderToStaticMarkup(createElement(EvidenceDrawer, { id: 'dense-evidence', evidence }));
  }

  function value(html: string, label: string): string | undefined {
    return html.match(new RegExp(`${label}: </span><span[^>]*>([^<]*)</span>`))?.[1];
  }

  it('does not present stored stale embeddings as usable dense coverage', () => {
    const html = renderCoverage(false);
    expect(value(html, 'Dense coverage')).toBe('Unavailable');
    expect(value(html, 'Stored embeddings')).toBe('1/1 chunks');
    expect(html).toContain('embedding_index_metadata_stale');
  });

  it('shows captured coverage when the dense engine is available', () => {
    const html = renderCoverage(true);
    expect(value(html, 'Dense coverage')).toBe('100% (1/1)');
    expect(value(html, 'Stored embeddings')).toBe('1/1 chunks');
  });

  it('does not invent readiness for historical receipts without the engine', () => {
    const html = renderCoverage();
    expect(value(html, 'Dense coverage')).toBe('Not recorded');
    expect(value(html, 'Stored embeddings')).toBe('1/1 chunks');
  });
});
