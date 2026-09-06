import { memo, useContext, useState, useEffect, useMemo, useRef, createContext } from "react";
import { useChatStore } from "../../stores/chatStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { useSourceStore } from "../../stores/sourceStore";
import { useNoteStore } from "../../stores/noteStore";
import { useNotebookStore } from "../../stores/notebookStore";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
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
  RefreshCw,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import type { ChatEvidenceDisclosure, Citation, Message } from "../../lib/types";

import { parseAssistantPayload } from "../../lib/chatEvidence";

interface ChatPanelProps {
  notebookId: string;
}

type ChatStoreState = ReturnType<typeof useChatStore.getState>;
type SettingsStoreState = ReturnType<typeof useSettingsStore.getState>;

interface ChatListContext { streamingContent: string }

function ChatStreamingFooter({ context }: { context?: ChatListContext }) {
  return context?.streamingContent ? <StreamingMessage content={context.streamingContent} /> : null;
}

const CHAT_LIST_COMPONENTS = { Footer: ChatStreamingFooter };

export function ChatPanel({ notebookId }: ChatPanelProps) {
  const conversations = useChatStore((s: ChatStoreState) => s.conversations);
  const activeConversationId = useChatStore((s: ChatStoreState) => s.activeConversationId);
  const messages = useChatStore((s: ChatStoreState) => s.messages);
  const isStreaming = useChatStore((s: ChatStoreState) => s.isStreaming);
  const streamingContent = useChatStore((s: ChatStoreState) => s.streamingContent);
  const streamingError = useChatStore((s: ChatStoreState) => s.streamingError);
  const streamingStatus = useChatStore((s: ChatStoreState) => s.streamingStatus);
  const sendMessage = useChatStore((s: ChatStoreState) => s.sendMessage);
  const stopStreaming = useChatStore((s: ChatStoreState) => s.stopStreaming);
  const createConversation = useChatStore((s: ChatStoreState) => s.createConversation);
  const deleteConversation = useChatStore((s: ChatStoreState) => s.deleteConversation);
  const setActiveConversation = useChatStore((s: ChatStoreState) => s.setActiveConversation);
  const loadMessages = useChatStore((s: ChatStoreState) => s.loadMessages);
  const rehydrateConversation = useChatStore((s: ChatStoreState) => s.rehydrateConversation);
  const replayChatEvents = useChatStore((s: ChatStoreState) => s.replayChatEvents);
  const suggestedQuestions = useChatStore((s: ChatStoreState) => s.suggestedQuestions);
  const style = useChatStore((s: ChatStoreState) => s.style);
  const customGoal = useChatStore((s: ChatStoreState) => s.customGoal);
  const responseLength = useChatStore((s: ChatStoreState) => s.responseLength);
  const setStyle = useChatStore((s: ChatStoreState) => s.setStyle);
  const setCustomGoal = useChatStore((s: ChatStoreState) => s.setCustomGoal);
  const setResponseLength = useChatStore((s: ChatStoreState) => s.setResponseLength);
  const saveResponse = useNoteStore((s) => s.saveResponse);
  const activeModel = useSettingsStore((s: SettingsStoreState) => s.activeModel);
  const models = useSettingsStore((s: SettingsStoreState) => s.models);
  const settings = useSettingsStore((s: SettingsStoreState) => s.settings);
  const refreshModels = useSettingsStore((s: SettingsStoreState) => s.refreshModels);
  const modelsLoading = useSettingsStore((s: SettingsStoreState) => s.loading);
  const getSourceScope = useSourceStore((s) => s.getSourceScope);
  const sources = useSourceStore((s) => s.sources);
  const selectedSourceIds = useSourceStore((s) => s.selectedSourceIds);
  const sourceScopeMode = useSourceStore((s) => s.sourceScopeMode);
  const sourceListStatus = useSourceStore((s) => s.sourceListStatus);
  const sourceListError = useSourceStore((s) => s.sourceListError);
  const selectModel = useSettingsStore((s) => s.selectModel);
  const selectionPending = useSettingsStore((s) => s.selectionPending);
  const selectionError = useSettingsStore((s) => s.selectionError);

  const emptyDraft = {
    notebookId, conversationId: activeConversationId, input: "",
    editingUserMessageId: null as string | null, restoreToken: null as object | null,
  };
  const [storedDraft, setDraft] = useState(emptyDraft);
  let draft = storedDraft;
  if (draft.notebookId !== notebookId || draft.conversationId !== activeConversationId) {
    // A first send may create its own conversation. Explicit selection actions
    // clear the token before switching; every other context change drops it.
    draft = {
      ...emptyDraft,
      restoreToken: draft.notebookId === notebookId && draft.conversationId === null
        ? draft.restoreToken : null,
    };
    setDraft(draft);
  }
  const { input, editingUserMessageId } = draft;
  const [savingMessageId, setSavingMessageId] = useState<string | null>(null);
  const [activeCitation, setActiveCitation] = useState<Citation | null>(null);
  const [expandedEvidence, setExpandedEvidence] = useState<Set<string>>(new Set());
  const listRef = useRef<VirtuosoHandle>(null);
  const navigationIntent = useRef<{
    notebookId: string; conversationId: string | null; activation: number;
    assistantId: string; previousIds: Set<string>;
  } | null>(null);
  const followingLatest = useRef<{
    notebookId: string; conversationId: string | null; activation: number;
  } | null>(null);
  const [bottomState, setBottomState] = useState({ notebookId, conversationId: activeConversationId, atBottom: false });
  const atBottom = messages.length === 0 ||
    (bottomState.notebookId === notebookId && bottomState.conversationId === activeConversationId && bottomState.atBottom);
  const scrollToLatest = () => listRef.current?.scrollToIndex({ index: "LAST", align: "end", behavior: "auto" });
  const cancelNavigation = () => {
    navigationIntent.current = null;
    followingLatest.current = null;
  };
  const jumpToLatest = () => {
    const notebook = useNotebookStore.getState();
    if (notebook.activeNotebookId !== notebookId ||
        useChatStore.getState().activeConversationId !== activeConversationId) return;
    followingLatest.current = { notebookId, conversationId: activeConversationId, activation: notebook.activationRequestId };
    scrollToLatest();
  };
  const followMeasuredHeight = () => {
    const intent = followingLatest.current;
    const notebook = useNotebookStore.getState();
    if (!intent || intent.notebookId !== notebookId || intent.conversationId !== activeConversationId ||
        notebook.activeNotebookId !== notebookId || notebook.activationRequestId !== intent.activation ||
        useChatStore.getState().activeConversationId !== activeConversationId ||
        useChatStore.getState().streamingError) return;
    // Tokens change footer height without changing item count. Preserve the
    // explicit follow intent through measured growth and terminal hydration.
    scrollToLatest();
  };

  useEffect(() => {
    const intent = navigationIntent.current;
    if (!intent) return;
    const notebook = useNotebookStore.getState();
    const chat = useChatStore.getState();
    if (intent.notebookId !== notebookId || notebook.activeNotebookId !== notebookId ||
        notebook.activationRequestId !== intent.activation || streamingError ||
        (intent.conversationId !== null && intent.conversationId !== activeConversationId) ||
        (chat.streamingMessageId && chat.streamingMessageId !== intent.assistantId)) {
      navigationIntent.current = null;
      return;
    }
    if (!activeConversationId || !listRef.current) return;
    // A first send may create its own conversation before appending the user.
    if (intent.conversationId === null && chat.streamingMessageId !== intent.assistantId) return;
    if (!messages.some((message) => message.role === "user" &&
        message.conversation_id === activeConversationId && !intent.previousIds.has(message.id))) return;
    navigationIntent.current = null;
    jumpToLatest();
  }, [messages, notebookId, activeConversationId, streamingError]);

  const sendWithNavigation = async (query: string, beforeUserId?: string) => {
    followingLatest.current = null;
    const previousIds = new Set(useChatStore.getState().messages.map((message) => message.id));
    const activation = useNotebookStore.getState().activationRequestId;
    const sending = sendMessage(notebookId, query, getSourceScope(), activeModel, beforeUserId);
    const chat = useChatStore.getState();
    const intent = chat.streamingNotebookId === notebookId && chat.streamingMessageId
      ? { notebookId, conversationId: activeConversationId, activation,
          assistantId: chat.streamingMessageId, previousIds } : null;
    navigationIntent.current = intent;
    try {
      await sending;
    } finally {
      if (useChatStore.getState().streamingError && navigationIntent.current === intent) {
        cancelNavigation();
      }
    }
  };

  useEffect(() => {
    if (streamingError) cancelNavigation();
  }, [streamingError]);

  useEffect(() => {
    if (!activeConversationId) return;
    void replayChatEvents(notebookId, activeConversationId)
      .catch((error) => {
        console.warn("Failed to replay chat events on conversation switch:", error);
      })
      .finally(() => {
        void rehydrateConversation(notebookId, activeConversationId);
      });
  }, [notebookId, activeConversationId, replayChatEvents, rehydrateConversation]);

  const handleSend = async () => {
    if (!input.trim() || isStreaming || selectionPending) return;
    const query = input.trim();
    const historyBeforeUserMessageId = editingUserMessageId ?? undefined;
    const restoreToken = {};
    const notebookActivation = useNotebookStore.getState().activationRequestId;
    setDraft({ ...emptyDraft, restoreToken });
    await sendWithNavigation(query, historyBeforeUserMessageId);
    if (useChatStore.getState().streamingError) {
      setDraft((current) => {
        const notebook = useNotebookStore.getState();
        const conversationId = useChatStore.getState().activeConversationId;
        if (current.restoreToken !== restoreToken || current.notebookId !== notebookId ||
            notebook.activeNotebookId !== notebookId || notebook.activationRequestId !== notebookActivation ||
            (current.conversationId !== null && current.conversationId !== conversationId)) return current;
        return { ...current, conversationId, input: query, editingUserMessageId: historyBeforeUserMessageId ?? null, restoreToken: null };
      });
    }
  };

  const handleSuggestionClick = (question: string) => {
    setDraft({ ...emptyDraft, input: question });
  };

  const handleDeleteConversation = async () => {
    if (!activeConversationId) return;
    cancelNavigation();
    setDraft(emptyDraft);
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
    cancelNavigation();
    await stopStreaming(notebookId);
  };

  const handleCopy = async (content: string) => {
    try {
      await navigator.clipboard.writeText(content);
    } catch (err) {
      console.warn("Failed to copy to clipboard:", err);
    }
  };

  const handleRegenerate = async (messageId: string) => {
    if (isStreaming || selectionPending) return;
    const messageIndex = messages.findIndex((m: Message) => m.id === messageId);
    const priorUser = [...messages]
      .slice(0, messageIndex)
      .reverse()
      .find((message) => message.role === "user");
    if (priorUser) {
      setDraft((current) => ({ ...current, restoreToken: null }));
      await sendWithNavigation(priorUser.content, priorUser.id);
    }
  };

  const handleContinue = async () => {
    if (isStreaming || selectionPending) return;
    setDraft((current) => ({ ...current, restoreToken: null }));
    await sendWithNavigation("Continue from the previous partial answer.");
  };

  const handleEditUserMessage = (message: Message) => {
    setDraft({ ...emptyDraft, input: message.content, editingUserMessageId: message.id });
  };

  const toggleEvidence = (messageId: string) => {
    setExpandedEvidence((previous) => {
      const next = new Set(previous);
      if (next.has(messageId)) next.delete(messageId);
      else next.add(messageId);
      return next;
    });
  };

  // D10 — memoize the derived counts. These run .filter() over `sources`
  // on every render; if sources is large, that's a hot path. Keyed on
  // sources + selectedSourceIds which are the only inputs.
  const { invalidSelectedCount, unreadySelectedCount, unindexedSelectedCount, projectionProblemCount } = useMemo(() => {
    const sel = sources.filter((s) => selectedSourceIds.has(s.id));
    const lifecycle = (s: (typeof sources)[number]) => s.processing_state?.lifecycle_status ?? s.status;
    const dense = (s: (typeof sources)[number]) => s.processing_state?.dense_index_status ?? "missing";
    const proj = (s: (typeof sources)[number]) => s.processing_state?.semantic_projection_status ?? "disabled";
    return {
      invalidSelectedCount: Array.from(selectedSourceIds).filter((id) => !sources.some((s) => s.id === id)).length,
      unreadySelectedCount: sel.filter((s) => lifecycle(s) !== "ready").length,
      unindexedSelectedCount: sel.filter((s) => ["missing", "failed", "stale"].includes(dense(s))).length,
      projectionProblemCount: sel.filter((s) => ["failed", "partial", "degraded", "stale", "not_projected"].includes(proj(s))).length,
    };
  }, [sources, selectedSourceIds]);
  // D12 — replace internal jargon with user-readable labels.
  // "GPU gate" -> "queue" and "background_summary" -> "background task".
  // Map is intentionally narrow; unknown values pass through unchanged.
  const humanizeGate = (raw: string) =>
    raw === "GPU gate" ? "queue" : raw === "LLM gate" ? "model queue" : raw;
  const humanizeOwner = (raw: string) =>
    raw === "background_summary" ? "background task" : raw;
  const formatMs = (value: number) =>
    value >= 1000 ? `${(value / 1000).toFixed(1)}s` : `${value}ms`;
  const streamingStatusLabel = streamingStatus
    ? [
        streamingStatus.message,
        streamingStatus.provider ? `Provider: ${streamingStatus.provider}` : null,
        streamingStatus.model ? `Model: ${streamingStatus.model}` : null,
        streamingStatus.elapsed_ms > 0 ? `Elapsed: ${formatMs(streamingStatus.elapsed_ms)}` : null,
        streamingStatus.timeout_ms ? `Timeout: ${formatMs(streamingStatus.timeout_ms)}` : null,
        streamingStatus.reason_code ? `Reason: ${streamingStatus.reason_code}` : null,
        streamingStatus.gate ? `Gate: ${humanizeGate(streamingStatus.gate)}` : null,
        streamingStatus.owner ? `Owner: ${humanizeOwner(streamingStatus.owner)}` : null,
        streamingStatus.owner_detail ? `Detail: ${streamingStatus.owner_detail}` : null,
      ]
        .filter(Boolean)
        .join(" - ")
    : "Starting chat";
  const sourceListDegraded =
    sourceListStatus === "loading" || sourceListStatus === "partial" || sourceListStatus === "error";
  const scopeMode = sourceScopeMode;

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="gloss-panel-header flex shrink-0 flex-wrap items-center gap-2 px-4 py-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <button
            onClick={() => {
              cancelNavigation();
              setDraft(emptyDraft);
              return createConversation(notebookId);
            }}
            disabled={isStreaming}
            className="flex items-center gap-1 rounded border border-accent/35 bg-accent/15 px-2 py-1 text-xs text-accent hover:bg-accent/20 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <Plus className="w-3 h-3" /> New Chat
          </button>
          {conversations.length > 0 && (
            <select
              aria-label="Conversation"
              value={activeConversationId || ""}
              disabled={isStreaming}
              onChange={(e) => {
                const id = e.target.value;
                if (id) {
                  cancelNavigation();
                  setDraft(emptyDraft);
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
          value={`${settings["default_provider"] || ""}::${activeModel}`}
          disabled={isStreaming || selectionPending}
          aria-label="Chat model and provider"
          onChange={(e) => {
            const [nextProvider, ...modelParts] = e.target.value.split("::");
            const nextModel = modelParts.join("::");
            if (nextProvider) void selectModel(nextProvider, nextModel).catch(() => undefined);
          }}
          className="min-w-0 max-w-full basis-48 grow rounded-full border border-border bg-bg-tertiary px-3 py-1 text-xs text-text focus:border-accent focus:outline-none"
        >
          {!models.some((m) => m.provider_id === settings["default_provider"] && m.id === activeModel) && (
            <option value={`${settings["default_provider"] || ""}::${activeModel}`} disabled>
              {activeModel ? `${activeModel} (unavailable)` : "Select a model"}
            </option>
          )}
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
        <button
          onClick={() => refreshModels()}
          disabled={modelsLoading}
          className="rounded-full border border-border bg-bg-tertiary p-1 text-text-muted hover:bg-bg-tertiary/80 hover:text-text disabled:opacity-50"
          title="Refresh model list from providers"
        >
          <RefreshCw className={`w-3 h-3 ${modelsLoading ? "animate-spin" : ""}`} />
        </button>
        <select
          value={style}
          onChange={(e) => setStyle(e.target.value)}
          disabled={isStreaming}
          className="rounded-full border border-border bg-bg-tertiary px-2 py-1 text-xs text-text focus:border-accent focus:outline-none disabled:cursor-not-allowed disabled:opacity-50"
          title="Conversational style"
          aria-label="Conversational style"
        >
          <option value="default">Default</option>
          <option value="learning_guide">Learning Guide</option>
          <option value="custom">Custom</option>
        </select>
        {style === "custom" && (
          <input
            type="text"
            value={customGoal}
            onChange={(e) => setCustomGoal(e.target.value)}
            placeholder="Custom goal (e.g. You are a code reviewer...)"
            aria-label="Custom conversation goal"
            disabled={isStreaming}
            className="min-w-[180px] rounded-full border border-border bg-bg-tertiary px-2 py-1 text-xs text-text placeholder:text-text-muted focus:border-accent focus:outline-none disabled:cursor-not-allowed disabled:opacity-50"
          />
        )}
        <select
          value={responseLength}
          onChange={(e) => setResponseLength(e.target.value)}
          disabled={isStreaming}
          className="rounded-full border border-border bg-bg-tertiary px-2 py-1 text-xs text-text focus:border-accent focus:outline-none disabled:cursor-not-allowed disabled:opacity-50"
          title="Response length"
          aria-label="Response length"
        >
          <option value="default">Default Length</option>
          <option value="short">Short</option>
          <option value="long">Long</option>
        </select>
      </div>

      {(selectionPending || selectionError) && (
        <p role={selectionError ? "alert" : "status"} className={`px-4 py-2 text-xs ${selectionError ? "text-error" : "text-text-muted"}`}>
          {selectionPending ? "Saving model selection…" : selectionError}
        </p>
      )}

      <div className="border-b border-border bg-bg-secondary/80 px-4 py-2">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <span className="gloss-mono text-[10px] uppercase tracking-[0.04em] text-text-muted">Scope</span>
          <span className={`rounded px-1.5 py-0.5 ${
            scopeMode === "none" || sourceListDegraded ? "bg-warning/15 text-warning" : "gloss-pill-accent text-text-secondary"
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
              {unindexedSelectedCount} dense missing
            </span>
          )}
          {projectionProblemCount > 0 && (
            <span className="rounded bg-warning/15 px-1.5 py-0.5 text-warning">
              {projectionProblemCount} projection pending
            </span>
          )}
          {sourceListDegraded && (
            <span className="flex items-center gap-1 text-warning">
              <AlertCircle className="w-3 h-3" />
              {sourceListStatus === "error"
                ? sourceListError || "Source list failed to load; chat will run without retrieval."
                : sourceListStatus === "partial"
                  ? "Source list partially loaded; retrieval may be incomplete."
                : "Source list still loading; retrieval may be incomplete."}
            </span>
          )}
          {scopeMode === "none" && !sourceListDegraded && (
            <span className="text-warning">
              Selected scope has no valid sources; retrieval will stay empty.
            </span>
          )}
        </div>
      </div>

      {/* Messages */}
      <div className="flex h-7 shrink-0 justify-end px-5">
        {!atBottom && messages.length > 0 && (
          <button type="button" aria-label="Jump to latest" onClick={jumpToLatest}
            className="rounded px-2 text-xs text-accent hover:bg-bg-secondary">
            Jump to latest
          </button>
        )}
      </div>
      <div role="region" aria-label="Chat messages" data-chat-at-bottom={atBottom}
        data-chat-latest-message-id={messages[messages.length - 1]?.id}
        onPointerDownCapture={cancelNavigation}
        onWheelCapture={cancelNavigation}
        onTouchMoveCapture={cancelNavigation}
        onKeyDownCapture={(event) => {
          if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"].includes(event.key)) {
            cancelNavigation();
          }
        }}
        className="gloss-chat-scroll min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
        {messages.length === 0 && !isStreaming && (
          <div className="mt-12 w-full text-center">
            <MessageSquare className="mx-auto mb-3 h-10 w-10 text-text-muted" />
            <p className="gloss-serif mb-4 text-xl text-text">
              Ask a question about your sources
            </p>
            {suggestedQuestions.length > 0 && (
              <div className="flex flex-wrap gap-2 justify-center">
                {suggestedQuestions.map((q, _i) => (
                  <button
                    key={q}
                    aria-label={`Suggested question: ${q}`}
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

        <MessageRowContext.Provider
          value={useMemo(
            () => ({
              isStreaming,
              selectionPending,
              savingMessageId,
              expandedEvidence,
              onCopy: handleCopy,
              onRegenerate: handleRegenerate,
              onSaveResponse: handleSaveResponse,
              onEditUserMessage: handleEditUserMessage,
              onContinue: handleContinue,
              onToggleEvidence: toggleEvidence,
              onSetActiveCitation: setActiveCitation,
            }),
            [
              isStreaming,
              selectionPending,
              savingMessageId,
              expandedEvidence,
              handleCopy,
              handleRegenerate,
              handleSaveResponse,
              handleEditUserMessage,
              handleContinue,
              toggleEvidence,
              setActiveCitation,
            ]
          )}
        >
          <Virtuoso
            key={`${notebookId}:${activeConversationId ?? "new"}`}
            ref={listRef}
            data={messages}
            context={{ streamingContent: isStreaming ? streamingContent : "" }}
            computeItemKey={(_index, message) => message.id}
            atBottomStateChange={(value) => setBottomState({ notebookId, conversationId: activeConversationId, atBottom: value })}
            totalListHeightChanged={followMeasuredHeight}
            followOutput="auto"
            initialTopMostItemIndex={Math.max(messages.length - 1, 0)}
            itemContent={(_index, msg) => <MessageRow key={msg.id} msg={msg} />}
            components={CHAT_LIST_COMPONENTS}
          />
        </MessageRowContext.Provider>

        {isStreaming && !streamingContent && (
          <div className="flex w-full justify-start">
            <div role="status" className="gloss-assistant-bubble flex items-center gap-2 px-3 py-2 text-sm text-text-secondary">
              <Loader2 className="w-4 h-4 text-text-muted animate-spin" />
              <span>{streamingStatusLabel}</span>
            </div>
          </div>
        )}

        {isStreaming && streamingContent && streamingStatus && streamingStatus.phase !== "streaming" && (
          <div className="flex w-full justify-start">
            <div className="rounded border border-border bg-bg-secondary px-2 py-1 text-xs text-text-muted">
              {streamingStatusLabel}
            </div>
          </div>
        )}

        {streamingStatus && streamingStatus.truncated && (
          <div className="flex w-full justify-start">
            <div className="flex items-start gap-2 rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-400">
              <AlertCircle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
              <span>Context budgeted: prompt exceeded model window and was truncated to fit.</span>
            </div>
          </div>
        )}

        {streamingError && (
          <div className="flex w-full justify-start">
            <div role="alert" className="flex max-w-[82%] items-start gap-2 rounded-lg border border-error/30 bg-error/10 px-3 py-2 text-sm text-error">
              <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
              <span>{streamingError}</span>
            </div>
          </div>
        )}

      </div>

      {/* Input */}
      <div className="gloss-input-dock px-5 py-3">
        <div className="gloss-input-shell flex w-full items-center gap-2 p-2">
          <textarea
            id="gloss-chat-composer"
            rows={2}
            aria-label="Chat message"
            aria-describedby="gloss-chat-shortcuts"
            value={input}
            onChange={(e) => {
              const value = e.target.value;
              setDraft((current) => ({ ...current, input: value, restoreToken: null }));
            }}
            onKeyDown={(e) => {
              if (shouldSubmitChat(e.key, e.shiftKey, e.nativeEvent.isComposing, e.keyCode)) {
                e.preventDefault();
                void handleSend();
              }
            }}
            placeholder="Ask about your sources..."
            disabled={isStreaming}
            className="min-w-0 max-h-48 flex-1 resize-y rounded bg-transparent px-2 py-1.5 text-sm text-text placeholder:text-text-muted focus:outline-none disabled:opacity-50"
          />
          <button
            onClick={isStreaming ? handleStop : handleSend}
            disabled={!isStreaming && (!input.trim() || selectionPending)}
            className="rounded-lg bg-accent p-2 text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
            title={isStreaming ? "Stop generation" : editingUserMessageId ? "Rerun edited message" : "Send"}
            aria-label={isStreaming ? "Stop generation" : editingUserMessageId ? "Rerun edited message" : "Send message"}
          >
            {isStreaming ? <StopCircle className="w-4 h-4" /> : <Send className="w-4 h-4" />}
          </button>
        </div>
        <p id="gloss-chat-shortcuts" className="mt-1 text-[10px] text-text-muted">Enter to send · Shift+Enter for a new line</p>
        {editingUserMessageId && (
          <div className="mx-auto mt-1 max-w-[900px] text-[10px] text-text-muted">
            Rerun uses the conversation before this question. All saved turns are retained.
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

export function shouldSubmitChat(key: string, shiftKey: boolean, isComposing: boolean, keyCode?: number): boolean {
  return key === "Enter" && !shiftKey && !isComposing && keyCode !== 229;
}

export function capturedModelLabel(message: Message): string {
  const receipt = message.citations?.evidence?.generation_receipt;
  const model = message.model_used || receipt?.model;
  if (!model) return "Model not captured";
  return receipt?.model === model && receipt.provider ? `${model} · ${receipt.provider}` : model;
}


type MessageRowContextValue = {
  isStreaming: boolean;
  selectionPending: boolean;
  savingMessageId: string | null;
  expandedEvidence: Set<string>;
  onCopy: (content: string) => void;
  onRegenerate: (messageId: string) => Promise<void>;
  onSaveResponse: (messageId: string) => Promise<void>;
  onEditUserMessage: (message: Message) => void;
  onContinue: () => Promise<void>;
  onToggleEvidence: (messageId: string) => void;
  onSetActiveCitation: (citation: Citation) => void;
};

const MessageRowContext = createContext<MessageRowContextValue | null>(null);

const useMessageRowContext = () => {
  const context = useContext(MessageRowContext);
  if (!context) {
    throw new Error("MessageRowContext missing");
  }
  return context;
};

const MessageRow = memo(function MessageRow({
  msg,
}: {
  msg: Message;
}) {
  const {
    isStreaming,
    selectionPending,
    savingMessageId,
    expandedEvidence,
    onCopy,
    onRegenerate,
    onSaveResponse,
    onEditUserMessage,
    onContinue,
    onToggleEvidence,
    onSetActiveCitation,
  } = useMessageRowContext();

  const parsedPayload = useMemo(() => parseAssistantPayload(msg.citations), [msg.id, msg.citations]);
  const parsedCitations = parsedPayload.citations;
  const evidence = parsedPayload.evidence;
  const evidenceOpen = expandedEvidence.has(msg.id);

  return (
    <div className={`flex w-full ${msg.role === "user" ? "justify-end" : "justify-start"}`}>
      <div
        data-chat-message-id={msg.id}
        data-chat-message-role={msg.role}
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
              <span className="gloss-mono">{capturedModelLabel(msg)}</span>
            </div>
            <div className="prose prose-invert prose-sm max-w-none">
              <ReactMarkdown>{msg.content}</ReactMarkdown>
            </div>
            <div className="gloss-mono mt-2 flex flex-wrap items-center gap-1.5 text-[10px] uppercase tracking-[0.03em]">
              <button
                onClick={() => void onCopy(msg.content)}
                className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-secondary hover:text-text"
                title="Copy markdown"
              >
                <Copy className="w-2.5 h-2.5" />
                Copy
              </button>
              <button
                onClick={() => void onRegenerate(msg.id)}
                disabled={isStreaming || selectionPending}
                className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-secondary hover:text-text disabled:opacity-60"
                title="Regenerate"
              >
                <RotateCcw className="w-2.5 h-2.5" />
                Regenerate
              </button>
              <button
                onClick={() => void onSaveResponse(msg.id)}
                disabled={savingMessageId === msg.id}
                className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-secondary hover:text-text disabled:opacity-60"
              >
                <BookmarkPlus className="w-2.5 h-2.5" />
                {savingMessageId === msg.id ? "Saving..." : "Save to notes"}
              </button>
              <button
                onClick={() => onToggleEvidence(msg.id)}
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
            {evidenceOpen && <EvidenceDrawer id={`evidence-${msg.id}`} evidence={evidence} />}
            {parsedCitations.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1.5 border-t border-border/40 pt-2">
                {parsedCitations.map((c, i) => (
                  <button
                      key={c.source_id ?? c.quote ?? `c-${i}`}
                    title={c.quote || c.source_title}
                    onClick={() => onSetActiveCitation(c)}
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
                onClick={onContinue}
                disabled={isStreaming || selectionPending}
                className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-white/80 hover:bg-white/10 disabled:opacity-60"
                title="Continue generation"
              >
                <RotateCcw className="w-2.5 h-2.5" />
                Continue
              </button>
              <button
                onClick={() => onEditUserMessage(msg)}
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
});

function StreamingMessage({ content }: { content: string }) {
  return (
    <div className="flex w-full justify-start">
      <div className="gloss-assistant-bubble max-w-[82%] px-3 py-2 text-sm text-text">
        <pre className="m-0 whitespace-pre-wrap break-words font-mono text-xs leading-relaxed">{content}</pre>
      </div>
    </div>
  );
}


export function EvidenceDrawer({ id, evidence }: { id: string; evidence: ChatEvidenceDisclosure }) {
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
        <EvidenceRow label="Decision" value={evidence.retrieval_capability_decision.decision_reason_code || evidence.retrieval_capability_decision.decision_reason || (evidence.retrieval_capability_decision.degraded ? "degraded" : "ready")} />
        <EvidenceRow label="Retrieval" value={evidence.retrieval_mode} />
        <EvidenceRow label="Fallback" value={evidence.fallback_used ? evidence.fallback_reason_code || evidence.fallback_reason || "yes" : "no"} />
        <EvidenceRow label="Scope" value={`${evidence.source_scope_mode} (${evidence.effective_source_count} selected, ${evidence.excluded_source_count} excluded, ${evidence.invalid_source_count} invalid)`} />
        <EvidenceRow label="Invalid scope IDs" value={`${evidence.invalid_source_ids.length} recorded`} />
        <EvidenceRow label="Requested source IDs" value={`${evidence.requested_source_ids.length} recorded`} />
        <EvidenceRow label="Selected source IDs" value={`${evidence.selected_source_ids.length} recorded`} />
        <EvidenceRow label="Excluded source IDs" value={`${evidence.excluded_source_ids.length} recorded`} />
        <EvidenceRow label="Context" value={`${evidence.context_passage_count} passages, preserved: ${evidence.source_scope_preserved ? "yes" : "no"}`} />
        <EvidenceRow label="Citations" value={`${evidence.citation_valid_count} valid, ${evidence.citation_invalid_count} filtered`} />
        {(evidence.citation_filter_reasons ?? []).length > 0 && (
          <EvidenceRow label="Citation filters" value={(evidence.citation_filter_reasons ?? []).map((r) => `${r.ref_number}:${r.reason_code}`).join(", ")} />
        )}
        <EvidenceRow label="Omitted" value={`${evidence.omitted_candidate_count} candidates/passages`} />
        <EvidenceRow label="Index" value={evidence.index_status} />
        <EvidenceRow label="Links" value={evidence.link_status} />
        {evidence.decoding_settings_receipt && (
          <EvidenceRow
            label="Temperature"
            value={evidence.decoding_settings_receipt.provider_capability.supports_temperature === false
              ? "Provider default"
              : `${evidence.decoding_settings_receipt.effective.temperature}`}
          />
        )}
        {evidence.prompt_receipt && (
          <EvidenceRow label="Prompt" value={evidence.prompt_receipt.capture_state} />
        )}
        {evidence.generation_receipt && (
          <EvidenceRow label="Generation" value={evidence.generation_receipt.status} />
        )}
        {evidence.prompt_budget_receipt && (
          <EvidenceRow
            label="Prompt budget"
            value={`${evidence.prompt_budget_receipt.estimated_prompt_tokens} est tokens, context budgeted: ${evidence.prompt_budget_receipt.context_budgeted ? "yes" : "no"}`}
          />
        )}
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
              navigator.clipboard?.writeText(
                JSON.stringify(evidence.retrieval_outcome, null, 2),
              ).catch((err) => console.warn("Failed to copy retrieval outcome:", err));
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
      {(evidence.requested_source_ids.length > 0 || evidence.effective_source_ids.length > 0 || evidence.excluded_source_ids.length > 0 || evidence.invalid_source_ids.length > 0) && (
        <p className="mt-2 text-text-muted">
          Source scope: {evidence.effective_source_count} effective, {evidence.excluded_source_count} excluded, {evidence.invalid_source_count} invalid
        </p>
      )}
      {(evidence.requested_source_ids.length > 0 || evidence.effective_source_ids.length > 0 || evidence.excluded_source_ids.length > 0 || evidence.invalid_source_ids.length > 0) && (
        <details className="mt-2 rounded border border-border/60 bg-bg-tertiary/40 p-2">
          <summary className="cursor-pointer text-text-secondary">Source scope diagnostics</summary>
          <button
            type="button"
            onClick={() => {
              navigator.clipboard?.writeText(JSON.stringify({
                requested_source_ids: evidence.requested_source_ids,
                selected_source_ids: evidence.selected_source_ids,
                effective_source_ids: evidence.effective_source_ids,
                excluded_source_ids: evidence.excluded_source_ids,
                invalid_source_ids: evidence.invalid_source_ids,
              }, null, 2)).catch((err) => console.warn("Failed to copy source selection:", err));
            }}
            className="mt-2 rounded border border-border px-2 py-1 text-[10px] text-text-secondary hover:bg-bg-tertiary hover:text-text"
          >
            Copy source scope JSON
          </button>
        </details>
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
