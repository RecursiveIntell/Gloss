import { useState, useRef, useEffect } from "react";
import { useChatStore } from "../../stores/chatStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { useSourceStore } from "../../stores/sourceStore";
import { useNoteStore } from "../../stores/noteStore";
import { SourceViewerModal } from "../sources/SourceViewerModal";
import {
  Send,
  Plus,
  MessageSquare,
  Loader2,
  AlertCircle,
  BookMarked,
  BookmarkPlus,
  Trash2,
  StopCircle,
  RotateCcw,
  Copy,
  FileEdit,
  ShieldCheck,
  ShieldAlert,
  ChevronDown,
  ChevronRight,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import type { ChatEvidenceDisclosure, ChatEvidencePayload, Citation, Message } from "../../lib/types";

interface ChatPanelProps {
  notebookId: string;
}

export function ChatPanel({ notebookId }: ChatPanelProps) {
  const {
    conversations,
    activeConversationId,
    messages,
    isStreaming,
    streamingContent,
    streamingError,
    streamingStatus,
    sendMessage,
    stopStreaming,
    createConversation,
    deleteConversation,
    setActiveConversation,
    loadMessages,
    suggestedQuestions,
  } = useChatStore();
  const saveResponse = useNoteStore((s) => s.saveResponse);
  const { activeModel, models, settings } = useSettingsStore();
  const getSourceScope = useSourceStore((s) => s.getSourceScope);
  const sources = useSourceStore((s) => s.sources);
  const selectedSourceIds = useSourceStore((s) => s.selectedSourceIds);
  const setActiveModel = useSettingsStore((s) => s.setActiveModel);
  const updateSetting = useSettingsStore((s) => s.updateSetting);

  const [input, setInput] = useState("");
  const [savingMessageId, setSavingMessageId] = useState<string | null>(null);
  const [activeCitation, setActiveCitation] = useState<Citation | null>(null);
  const [expandedEvidence, setExpandedEvidence] = useState<Set<string>>(new Set());
  const [editingUserMessageId, setEditingUserMessageId] = useState<string | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, streamingContent]);

  const handleSend = async () => {
    if (!input.trim() || isStreaming) return;
    const query = input.trim();
    setInput("");
    setEditingUserMessageId(null);
    await sendMessage(notebookId, query, getSourceScope(), activeModel);
  };

  const handleSuggestionClick = (question: string) => {
    setInput(question);
  };

  const handleDeleteConversation = async () => {
    if (!activeConversationId) return;
    await deleteConversation(notebookId, activeConversationId);
  };

  const handleSaveResponse = async (messageId: string) => {
    if (savingMessageId) return;
    setSavingMessageId(messageId);
    try {
      await saveResponse(notebookId, messageId);
    } finally {
      setSavingMessageId(null);
    }
  };

  const handleStop = async () => {
    await stopStreaming(notebookId);
  };

  const handleCopy = async (content: string) => {
    await navigator.clipboard.writeText(content);
  };

  const handleRegenerate = async (messageIndex: number) => {
    if (isStreaming) return;
    const priorUser = [...messages]
      .slice(0, messageIndex)
      .reverse()
      .find((message) => message.role === "user");
    if (priorUser) {
      await sendMessage(notebookId, priorUser.content, getSourceScope(), activeModel);
    }
  };

  const handleEditUserMessage = (message: Message) => {
    setInput(message.content);
    setEditingUserMessageId(message.id);
  };

  const toggleEvidence = (messageId: string) => {
    setExpandedEvidence((previous) => {
      const next = new Set(previous);
      if (next.has(messageId)) next.delete(messageId);
      else next.add(messageId);
      return next;
    });
  };

  const selectedSources = sources.filter((source) => selectedSourceIds.has(source.id));
  const invalidSelectedCount = Array.from(selectedSourceIds).filter(
    (id) => !sources.some((source) => source.id === id)
  ).length;
  const unreadySelectedCount = selectedSources.filter((source) => source.status !== "ready").length;
  const unindexedSelectedCount = selectedSources.filter((source) => source.status === "pending").length;
  const streamingStatusLabel = streamingStatus
    ? [
        streamingStatus.message,
        streamingStatus.gate ? `Gate: ${streamingStatus.gate}` : null,
        streamingStatus.owner ? `Owner: ${streamingStatus.owner}` : null,
        streamingStatus.owner_detail ? `Detail: ${streamingStatus.owner_detail}` : null,
      ]
        .filter(Boolean)
        .join(" - ")
    : "Starting chat";
  const scopeMode = sources.length === 0
    ? "all"
    : selectedSourceIds.size === 0
      ? "none"
      : selectedSourceIds.size === sources.length
        ? "all"
        : "explicit";

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="gloss-panel-header flex items-center justify-between gap-3 px-4 py-2">
        <div className="flex items-center gap-2">
          <button
            onClick={() => createConversation(notebookId)}
            disabled={isStreaming}
            className="flex items-center gap-1 rounded border border-accent/35 bg-accent/15 px-2 py-1 text-xs text-accent hover:bg-accent/20 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <Plus className="w-3 h-3" /> New Chat
          </button>
          {conversations.length > 0 && (
            <select
              value={activeConversationId || ""}
              disabled={isStreaming}
              onChange={(e) => {
                const id = e.target.value;
                if (id) {
                  setActiveConversation(id);
                  loadMessages(notebookId, id);
                }
              }}
              className="rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text focus:border-accent focus:outline-none disabled:cursor-not-allowed disabled:opacity-50"
            >
              <option value="">Select conversation</option>
              {conversations.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.title || `Chat ${c.id.slice(0, 8)}`}
                </option>
              ))}
            </select>
          )}
          {activeConversationId && (
            <button
              onClick={handleDeleteConversation}
              disabled={isStreaming}
              className="flex items-center gap-1 rounded border border-border bg-bg-secondary px-2 py-1 text-xs text-text-secondary hover:bg-error/15 hover:text-error disabled:cursor-not-allowed disabled:opacity-50"
              title="Delete conversation"
            >
              <Trash2 className="w-3 h-3" /> Delete
            </button>
          )}
        </div>
        <select
          value={`${settings["default_provider"] || models.find((model) => model.id === activeModel)?.provider_id || ""}::${activeModel}`}
          onChange={(e) => {
            const [nextProvider, ...modelParts] = e.target.value.split("::");
            const nextModel = modelParts.join("::");
            void updateSetting("default_model", nextModel);
            if (nextProvider) {
              void updateSetting("default_provider", nextProvider);
            }
            setActiveModel(nextModel);
          }}
          className="min-w-0 max-w-[320px] rounded-full border border-border bg-bg-tertiary px-3 py-1 text-xs text-text focus:border-accent focus:outline-none"
        >
          {models.length > 0 ? (
            models.map((m) => (
              <option
                key={`${m.provider_id}::${m.id}`}
                value={`${m.provider_id}::${m.id}`}
                disabled={!m.available || m.stale}
              >
                {m.display_name} ({m.provider_id}
                {!m.available || m.stale ? ", unavailable" : ""})
              </option>
            ))
          ) : (
            <option value={`::${activeModel}`}>{activeModel}</option>
          )}
        </select>
      </div>

      <div className="border-b border-border bg-bg-secondary/80 px-4 py-2">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <span className="gloss-mono text-[10px] uppercase tracking-[0.04em] text-text-muted">Scope</span>
          <span className={`rounded px-1.5 py-0.5 ${
            scopeMode === "none" ? "bg-warning/15 text-warning" : "gloss-pill-accent text-text-secondary"
          }`}>
            {scopeMode}
          </span>
          <span className="text-text-muted">
            {selectedSourceIds.size}/{sources.length} selected
          </span>
          {invalidSelectedCount > 0 && (
            <span className="rounded bg-error/15 px-1.5 py-0.5 text-error">
              {invalidSelectedCount} invalid
            </span>
          )}
          {unreadySelectedCount > 0 && (
            <span className="rounded bg-warning/15 px-1.5 py-0.5 text-warning">
              {unreadySelectedCount} not ready
            </span>
          )}
          {unindexedSelectedCount > 0 && (
            <span className="rounded bg-warning/15 px-1.5 py-0.5 text-warning">
              {unindexedSelectedCount} unindexed
            </span>
          )}
          {scopeMode === "none" && (
            <span className="text-warning">
              Selected scope has no valid sources; retrieval will stay empty.
            </span>
          )}
        </div>
      </div>

      {/* Messages */}
      <div className="gloss-chat-scroll flex-1 space-y-4 overflow-y-auto px-5 py-4">
        {messages.length === 0 && !isStreaming && (
          <div className="mx-auto mt-12 max-w-[680px] text-center">
            <MessageSquare className="mx-auto mb-3 h-10 w-10 text-text-muted" />
            <p className="gloss-serif mb-4 text-xl text-text">
              Ask a question about your sources
            </p>
            {suggestedQuestions.length > 0 && (
              <div className="flex flex-wrap gap-2 justify-center">
                {suggestedQuestions.map((q, i) => (
                  <button
                    key={i}
                    onClick={() => handleSuggestionClick(q)}
                    className="rounded-full border border-border bg-bg-secondary px-3 py-1.5 text-xs text-text-secondary hover:bg-bg-tertiary hover:text-text"
                  >
                    {q}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        {messages.map((msg, messageIndex) => {
          const parsedPayload = parseAssistantPayload(msg.citations);
          const parsedCitations = parsedPayload.citations;
          const evidence = parsedPayload.evidence;
          const evidenceOpen = expandedEvidence.has(msg.id);

          return (
            <div
              key={msg.id}
              className={`mx-auto flex max-w-[900px] ${msg.role === "user" ? "justify-end" : "justify-start"}`}
            >
              <div
                className={`max-w-[82%] px-3 py-2 text-sm ${
                  msg.role === "user"
                    ? "gloss-user-bubble text-white"
                    : "gloss-assistant-bubble text-text"
                }`}
              >
                {msg.role === "assistant" ? (
                  <>
                    <div className="mb-2 flex items-center gap-2 text-[11px] text-text-muted">
                      <span className="gloss-serif text-sm text-text-secondary">Gloss</span>
                      <span>·</span>
                      <span className="gloss-mono">{activeModel}</span>
                    </div>
                    <div className="prose prose-invert prose-sm max-w-none">
                      <ReactMarkdown>{msg.content}</ReactMarkdown>
                    </div>
                    <div className="gloss-mono mt-2 flex flex-wrap items-center gap-1.5 text-[10px] uppercase tracking-[0.03em]">
                      <button
                        onClick={() => void handleCopy(msg.content)}
                        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-secondary hover:text-text"
                        title="Copy markdown"
                      >
                        <Copy className="w-2.5 h-2.5" />
                        Copy
                      </button>
                      <button
                        onClick={() => void handleRegenerate(messageIndex)}
                        disabled={isStreaming}
                        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-secondary hover:text-text disabled:opacity-60"
                        title="Regenerate"
                      >
                        <RotateCcw className="w-2.5 h-2.5" />
                        Regenerate
                      </button>
                      <button
                        onClick={() => handleSaveResponse(msg.id)}
                        disabled={savingMessageId === msg.id}
                        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-secondary hover:text-text disabled:opacity-60"
                      >
                        <BookmarkPlus className="w-2.5 h-2.5" />
                        {savingMessageId === msg.id ? "Saving..." : "Save to notes"}
                      </button>
                      <button
                        onClick={() => toggleEvidence(msg.id)}
                        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-secondary hover:text-text"
                        title="Evidence"
                        aria-expanded={evidenceOpen}
                        aria-controls={`evidence-${msg.id}`}
                      >
                        {evidenceOpen ? (
                          <ChevronDown className="w-2.5 h-2.5" />
                        ) : (
                          <ChevronRight className="w-2.5 h-2.5" />
                        )}
                        Evidence
                      </button>
                    </div>
                    {evidenceOpen && (
                      <EvidenceDrawer id={`evidence-${msg.id}`} evidence={evidence} />
                    )}
                    {parsedCitations.length > 0 && (
                      <div className="mt-2 flex flex-wrap gap-1.5 border-t border-border/40 pt-2">
                        {parsedCitations.map((c, i) => (
                          <button
                            key={i}
                            title={c.quote || c.source_title}
                            onClick={() => setActiveCitation(c)}
                            className="inline-flex items-center gap-1 rounded border border-accent/25 bg-accent/15 px-1.5 py-0.5 text-[10px] text-accent transition-colors hover:bg-accent/25"
                          >
                            <BookMarked className="w-2.5 h-2.5" />
                            [{i + 1}] {c.source_title}
                          </button>
                        ))}
                      </div>
                    )}
                  </>
                ) : (
                  <div>
                    <p>{msg.content}</p>
                    <div className="mt-1 flex justify-end">
                      <button
                        onClick={() => handleEditUserMessage(msg)}
                        disabled={isStreaming}
                        className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-white/80 hover:bg-white/10 disabled:opacity-60"
                        title="Edit and rerun"
                      >
                        <FileEdit className="w-2.5 h-2.5" />
                        Edit
                      </button>
                    </div>
                  </div>
                )}
              </div>
            </div>
          );
        })}

        {isStreaming && streamingContent && (
          <div className="mx-auto flex max-w-[900px] justify-start">
            <div className="gloss-assistant-bubble max-w-[82%] px-3 py-2 text-sm text-text">
              <div className="prose prose-invert prose-sm max-w-none">
                <ReactMarkdown>{streamingContent}</ReactMarkdown>
              </div>
            </div>
          </div>
        )}

        {isStreaming && !streamingContent && (
          <div className="mx-auto flex max-w-[900px] justify-start">
            <div className="gloss-assistant-bubble flex items-center gap-2 px-3 py-2 text-sm text-text-secondary">
              <Loader2 className="w-4 h-4 text-text-muted animate-spin" />
              <span>{streamingStatusLabel}</span>
            </div>
          </div>
        )}

        {isStreaming && streamingContent && streamingStatus && streamingStatus.phase !== "streaming" && (
          <div className="mx-auto flex max-w-[900px] justify-start">
            <div className="rounded border border-border bg-bg-secondary px-2 py-1 text-xs text-text-muted">
              {streamingStatusLabel}
            </div>
          </div>
        )}

        {streamingError && (
          <div className="mx-auto flex max-w-[900px] justify-start">
            <div className="flex max-w-[82%] items-start gap-2 rounded-lg border border-error/30 bg-error/10 px-3 py-2 text-sm text-error">
              <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
              <span>{streamingError}</span>
            </div>
          </div>
        )}

        <div ref={messagesEndRef} />
      </div>

      {/* Input */}
      <div className="gloss-input-dock px-5 py-3">
        <div className="gloss-input-shell mx-auto flex max-w-[900px] items-center gap-2 p-2">
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && handleSend()}
            placeholder="Ask about your sources..."
            disabled={isStreaming}
            className="flex-1 rounded bg-transparent px-2 py-1.5 text-sm text-text placeholder:text-text-muted focus:outline-none disabled:opacity-50"
          />
          <button
            onClick={isStreaming ? handleStop : handleSend}
            disabled={!isStreaming && !input.trim()}
            className="rounded-lg bg-accent p-2 text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
            title={isStreaming ? "Stop generation" : editingUserMessageId ? "Rerun edited message" : "Send"}
          >
            {isStreaming ? <StopCircle className="w-4 h-4" /> : <Send className="w-4 h-4" />}
          </button>
        </div>
        {editingUserMessageId && (
          <div className="mx-auto mt-1 max-w-[900px] text-[10px] text-text-muted">
            Editing a previous question; sending will rerun it as a new turn.
          </div>
        )}
      </div>

      <SourceViewerModal
        notebookId={notebookId}
        citation={activeCitation}
        open={activeCitation != null}
        onClose={() => setActiveCitation(null)}
      />
    </div>
  );
}

function parseAssistantPayload(raw: Message["citations"]): ChatEvidencePayload {
  if (!raw) return { citations: [], evidence: nullEvidence() };
  try {
    const parsed = typeof raw === "string" ? JSON.parse(raw) : raw;
    if (Array.isArray(parsed)) {
      return { citations: parsed, evidence: nullEvidence(parsed.length) };
    }
    if (parsed && typeof parsed === "object" && Array.isArray(parsed.citations)) {
      return {
        citations: parsed.citations,
        evidence: parsed.evidence ?? nullEvidence(parsed.citations.length),
      };
    }
  } catch {
    return { citations: [], evidence: nullEvidence() };
  }
  return { citations: [], evidence: nullEvidence() };
}

function nullEvidence(citationCount = 0): ChatEvidenceDisclosure {
  return {
    backend_requested: "unknown",
    backend_used: "unknown",
    retrieval_mode: "unknown",
    fallback_used: false,
    fallback_reason: null,
    degradation_markers: [],
    source_scope_mode: "unknown",
    requested_source_ids: [],
    selected_source_ids: [],
    effective_source_ids: [],
    invalid_source_ids: [],
    excluded_source_ids: [],
    invalid_source_count: 0,
    effective_source_count: 0,
    excluded_source_count: 0,
    context_passage_count: 0,
    citation_valid_count: citationCount,
    citation_invalid_count: 0,
    omitted_candidate_count: 0,
    source_scope_preserved: true,
    index_status: "unknown",
    link_status: "unknown",
    receipt_id: "not recorded",
    semantic_memory_receipt_id: null,
    candidate_backend: null,
    turbo_quant_generation_id: null,
    vector_artifact_manifest_digest: null,
    exact_rerank: null,
    exact_rerank_count: null,
    approximate_candidate_count: null,
    semantic_memory_fallback_reason: null,
    retrieval_outcome: null,
  };
}

function EvidenceDrawer({ id, evidence }: { id: string; evidence: ChatEvidenceDisclosure }) {
  const degraded = evidence.fallback_used || evidence.degradation_markers.length > 0 || evidence.citation_invalid_count > 0;
  return (
    <div id={id} role="region" aria-label="Answer evidence" className="mt-2 rounded border border-border/70 bg-bg-secondary p-2 text-[10px] text-text-secondary">
      <div className="mb-2 flex items-center gap-1.5 text-text">
        {degraded ? (
          <ShieldAlert className="w-3 h-3 text-warning" />
        ) : (
          <ShieldCheck className="w-3 h-3 text-success" />
        )}
        <span>Answer evidence</span>
      </div>
      <div className="grid grid-cols-2 gap-x-3 gap-y-1">
        <EvidenceRow label="Backend requested" value={evidence.backend_requested} />
        <EvidenceRow label="Backend used" value={evidence.backend_used} />
        <EvidenceRow label="Retrieval" value={evidence.retrieval_mode} />
        <EvidenceRow label="Fallback" value={evidence.fallback_used ? evidence.fallback_reason || "yes" : "no"} />
        <EvidenceRow label="Scope" value={`${evidence.source_scope_mode} (${evidence.effective_source_count} selected, ${evidence.excluded_source_count} excluded, ${evidence.invalid_source_count} invalid)`} />
        <EvidenceRow label="Context" value={`${evidence.context_passage_count} passages, preserved: ${evidence.source_scope_preserved ? "yes" : "no"}`} />
        <EvidenceRow label="Citations" value={`${evidence.citation_valid_count} valid, ${evidence.citation_invalid_count} filtered`} />
        <EvidenceRow label="Omitted" value={`${evidence.omitted_candidate_count} candidates/passages`} />
        <EvidenceRow label="Index" value={evidence.index_status} />
        <EvidenceRow label="Links" value={evidence.link_status} />
        {evidence.candidate_backend && (
          <EvidenceRow label="Candidate backend" value={evidence.candidate_backend} />
        )}
        {evidence.exact_rerank !== null && evidence.exact_rerank !== undefined && (
          <EvidenceRow label="Exact rerank" value={evidence.exact_rerank ? `yes (${evidence.exact_rerank_count ?? 0})` : "no"} />
        )}
        {evidence.approximate_candidate_count !== null && evidence.approximate_candidate_count !== undefined && (
          <EvidenceRow label="Approx candidates" value={`${evidence.approximate_candidate_count}`} />
        )}
        {evidence.turbo_quant_generation_id && (
          <EvidenceRow label="TurboQuant generation" value={evidence.turbo_quant_generation_id} />
        )}
        {evidence.retrieval_outcome && (
          <>
            <EvidenceRow label="Retrieval mode" value={evidence.retrieval_outcome.mode} />
            <EvidenceRow
              label="Dense coverage"
              value={`${Math.round(evidence.retrieval_outcome.coverage.dense_coverage_ratio * 100)}% (${evidence.retrieval_outcome.coverage.embedded_chunks}/${evidence.retrieval_outcome.coverage.total_chunks})`}
            />
            <EvidenceRow
              label="Engines"
              value={evidence.retrieval_outcome.engines
                .map((engine) =>
                  `${engine.engine}:${engine.contributed ? "contributed" : engine.reason_code || "no candidates"}`
                )
                .join(", ")}
            />
          </>
        )}
      </div>
      {evidence.retrieval_outcome && (
        <div className="mt-2 rounded border border-border/60 bg-bg-tertiary/50 p-2">
          <p className="text-text-secondary">{evidence.retrieval_outcome.user_visible_summary}</p>
          {evidence.retrieval_outcome.fallback_chain.length > 0 && (
            <p className="mt-1 text-warning">
              Retrieval reasons: {evidence.retrieval_outcome.fallback_chain.join(", ")}
            </p>
          )}
          <button
            type="button"
            onClick={() => {
              void navigator.clipboard?.writeText(
                JSON.stringify(evidence.retrieval_outcome, null, 2),
              );
            }}
            className="mt-2 rounded border border-border px-2 py-1 text-[10px] text-text-secondary hover:bg-bg-tertiary hover:text-text"
          >
            Copy retrieval diagnostics JSON
          </button>
        </div>
      )}
      {evidence.semantic_memory_fallback_reason && (
        <p className="mt-2 text-warning">semantic-memory fallback: {evidence.semantic_memory_fallback_reason}</p>
      )}
      {evidence.vector_artifact_manifest_digest && (
        <p className="mt-2 text-text-muted">Vector artifact manifest: {evidence.vector_artifact_manifest_digest}</p>
      )}
      {evidence.invalid_source_ids.length > 0 && (
        <p className="mt-2 text-error">Invalid scope IDs: {evidence.invalid_source_ids.join(", ")}</p>
      )}
      {evidence.requested_source_ids.length > 0 && (
        <p className="mt-2">Requested source IDs: {evidence.requested_source_ids.join(", ")}</p>
      )}
      {evidence.effective_source_ids.length > 0 && (
        <p className="mt-2">Selected source IDs: {(evidence.selected_source_ids.length ? evidence.selected_source_ids : evidence.effective_source_ids).join(", ")}</p>
      )}
      {evidence.excluded_source_ids.length > 0 && (
        <p className="mt-2 text-text-muted">Excluded source IDs: {evidence.excluded_source_ids.join(", ")}</p>
      )}
      {evidence.degradation_markers.length > 0 && (
        <p className="mt-2 text-warning">Degraded: {evidence.degradation_markers.join(", ")}</p>
      )}
      <p className="mt-2 text-text-muted">Receipt: {evidence.receipt_id}</p>
      {evidence.semantic_memory_receipt_id && (
        <p className="mt-1 text-text-muted">semantic-memory receipt: {evidence.semantic_memory_receipt_id}</p>
      )}
    </div>
  );
}

function EvidenceRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <span className="text-text-muted">{label}: </span>
      <span className="break-words text-text-secondary">{value}</span>
    </div>
  );
}
