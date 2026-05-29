import { useChatStore } from "../../stores/chatStore";
import type { ChatEvidenceDisclosure, CitationAnchorV1, CitationFilterReasonV1 } from "../../lib/types";
import { AlertTriangle, CheckCircle, ShieldAlert } from "lucide-react";

/**
 * Evidence Inspector panel — shows the evidence disclosure for the most recent
 * assistant message in the active conversation. Reads from chatStore.messages
 * where citations (ChatEvidencePayload) are attached after each chat response.
 */
export function EvidencePanel() {
  const messages = useChatStore((s) => s.messages);
  const isStreaming = useChatStore((s) => s.isStreaming);
  const streamingContent = useChatStore((s) => s.streamingContent);

  // Find the latest assistant message with evidence
  const lastAssistantWithEvidence = [...messages]
    .reverse()
    .find((m) => m.role === "assistant" && m.citations?.evidence);

  const evidence: ChatEvidenceDisclosure | undefined =
    lastAssistantWithEvidence?.citations?.evidence;
  const citations = lastAssistantWithEvidence?.citations?.citations ?? [];
  const messageId = lastAssistantWithEvidence?.id ?? null;

  // While streaming, show placeholder
  if (isStreaming && streamingContent && !evidence) {
    return (
      <div className="flex h-full items-center justify-center p-4 text-center text-xs text-text-muted">
        Generating — evidence arrives after response completes
      </div>
    );
  }

  if (!evidence) {
    return (
      <div className="p-3 space-y-3 text-xs">
        <div className="text-text-muted text-center py-8">
          No evidence available yet. Send a chat message to generate evidence.
        </div>
      </div>
    );
  }

  return (
    <div className="p-3 space-y-3 text-xs overflow-y-auto h-full">
      {/* Retrieval Backend */}
      <Section title="Retrieval Backend">
        <KVRow label="Requested" value={evidence.backend_requested} />
        <KVRow label="Used" value={evidence.backend_used} />
        <KVRow label="Mode" value={evidence.retrieval_mode} />
        {evidence.fallback_used && (
          <div className="flex items-start gap-1 mt-1 rounded bg-warning/10 px-2 py-1 text-warning">
            <AlertTriangle className="w-3 h-3 shrink-0 mt-0.5" />
            <span>Fallback: {evidence.fallback_reason ?? "unknown"}</span>
          </div>
        )}
        {evidence.degradation_markers.length > 0 && (
          <KVRow label="Degradation" value={evidence.degradation_markers.join(", ")} warn />
        )}
      </Section>

      {/* Source Scope Integrity */}
      <Section title="Source Scope">
        <KVRow label="Mode" value={evidence.source_scope_mode} />
        <KVRow label="Requested" value={`${evidence.requested_source_ids.length} sources`} />
        <KVRow label="Effective" value={`${evidence.effective_source_count} sources`} />
        {evidence.invalid_source_count > 0 && (
          <KVRow label="Invalid" value={`${evidence.invalid_source_count}`} warn />
        )}
        {evidence.excluded_source_count > 0 && (
          <KVRow label="Excluded" value={`${evidence.excluded_source_count}`} warn />
        )}
        <KVRow
          label="Scope preserved"
          value={evidence.source_scope_preserved ? "Yes" : "NO"}
          warn={!evidence.source_scope_preserved}
        />
      </Section>

      {/* Citations */}
      <Section title={`Citations (${evidence.citation_valid_count} valid, ${evidence.citation_invalid_count} invalid)`}>
        {citations.map((c, i) => (
          <div key={i} className="rounded border border-border bg-bg-secondary/60 px-2 py-1.5 space-y-0.5">
            <div className="flex items-center gap-1">
              <CheckCircle className="w-3 h-3 text-success shrink-0" />
              <span className="text-text font-medium truncate">{c.source_title || c.source_id}</span>
            </div>
            {c.quote && <div className="text-text-muted pl-4 line-clamp-2">"{c.quote}"</div>}
            <div className="text-text-muted pl-4">
              {c.chunk_id.slice(0, 12)}{c.page ? ` · p${c.page}` : ""}{c.section ? ` · ${c.section}` : ""}
            </div>
          </div>
        ))}
        {evidence.citation_anchors.length > 0 && (
          <CitationAnchors anchors={evidence.citation_anchors} />
        )}
        {evidence.citation_filter_reasons.length > 0 && (
          <CitationFilters filters={evidence.citation_filter_reasons} />
        )}
        {evidence.omitted_candidate_count > 0 && (
          <KVRow label="Omitted candidates" value={`${evidence.omitted_candidate_count}`} warn />
        )}
      </Section>

      {/* Index / Link status */}
      <Section title="Index Status">
        <KVRow label="Index" value={evidence.index_status} />
        <KVRow label="Link" value={evidence.link_status} />
        <KVRow label="Passages" value={`${evidence.context_passage_count}`} />
      </Section>

      {/* Semantic Memory / TurboQuant Truth */}
      <Section title="Semantic Memory">
        <KVRow
          label="Requested"
          value={evidence.semantic_memory_runtime_truth?.decision?.requested_backend ?? "—"}
        />
        <KVRow
          label="Effective"
          value={evidence.semantic_memory_runtime_truth?.decision?.effective_backend ?? "—"}
        />
        <KVRow
          label="Build feature"
          value={evidence.semantic_memory_runtime_truth?.decision?.build_feature_available ? "available" : "not compiled"}
          warn={!evidence.semantic_memory_runtime_truth?.decision?.build_feature_available}
        />
        <KVRow
          label="Runtime enabled"
          value={evidence.semantic_memory_runtime_truth?.decision?.runtime_enabled ? "yes" : "no"}
        />
        <KVRow
          label="Projection ready"
          value={evidence.semantic_memory_runtime_truth?.decision?.projection_ready ? "yes" : "no"}
        />
        <KVRow
          label="Dense ready"
          value={evidence.semantic_memory_runtime_truth?.decision?.dense_ready ? "yes" : "no"}
        />
        {evidence.turbo_quant_generation_id && (
          <KVRow label="TQ generation" value={evidence.turbo_quant_generation_id.slice(0, 12)} />
        )}
        {evidence.exact_rerank != null && (
          <KVRow
            label="TQ exact rerank"
            value={evidence.exact_rerank ? "proven" : "not proven"}
            warn={!evidence.exact_rerank}
          />
        )}
        {evidence.semantic_memory_fallback_reason && (
          <div className="flex items-start gap-1 rounded bg-warning/10 px-2 py-1 text-warning">
            <AlertTriangle className="w-3 h-3 shrink-0 mt-0.5" />
            <span>SM fallback: {evidence.semantic_memory_fallback_reason}</span>
          </div>
        )}
      </Section>

      {/* Receipt IDs */}
      <Section title="Receipts">
        <KVRow label="Receipt" value={evidence.receipt_id.slice(0, 16)} />
        <KVRow label="Context digest" value={evidence.context_digest.slice(0, 16)} />
        {evidence.semantic_memory_receipt_id && (
          <KVRow label="SM receipt" value={evidence.semantic_memory_receipt_id.slice(0, 16)} />
        )}
      </Section>

      {messageId && (
        <div className="text-text-muted border-t border-border pt-2">
          Message: {messageId.slice(0, 12)}
        </div>
      )}
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1.5">
      <div className="font-medium text-text">{title}</div>
      <div className="space-y-1 pl-1">{children}</div>
    </div>
  );
}

function KVRow({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <div className="grid grid-cols-[5.5rem_1fr] gap-1">
      <span className="text-text-muted truncate">{label}</span>
      <span className={`break-words ${warn ? "text-warning" : "text-text-secondary"}`}>{value}</span>
    </div>
  );
}

function CitationAnchors({ anchors }: { anchors: CitationAnchorV1[] }) {
  if (anchors.length === 0) return null;
  return (
    <div className="space-y-0.5 mt-1">
      <div className="text-text-muted">Anchors ({anchors.length}):</div>
      {anchors.map((a) => (
        <div key={a.ref_number} className="pl-2 text-text-secondary">
          [{a.ref_number}] {a.source_id.slice(0, 12)} · {a.evidence_class} · chunk {a.chunk_id.slice(0, 8)}
        </div>
      ))}
    </div>
  );
}

function CitationFilters({ filters }: { filters: CitationFilterReasonV1[] }) {
  if (filters.length === 0) return null;
  return (
    <div className="space-y-0.5 mt-1">
      <div className="flex items-center gap-1 text-warning">
        <ShieldAlert className="w-3 h-3" />
        <span>Filtered ({filters.length}):</span>
      </div>
      {filters.map((f) => (
        <div key={f.ref_number} className="pl-2 text-text-secondary">
          [{f.ref_number}] {f.reason_code}: {f.detail}
        </div>
      ))}
    </div>
  );
}