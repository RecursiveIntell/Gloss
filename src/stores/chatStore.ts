import { create } from 'zustand';
import type { ChatEvidencePayload, ChatStatusPayload, Conversation, Message, SourceScope } from '../lib/types';
import * as api from '../lib/tauri';
import { useToastStore } from './toastStore';
import { useNotebookStore } from './notebookStore';

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  try {
    return JSON.stringify(error);
  } catch {
    return 'Chat request failed';
  }
}

interface ChatStore {
  conversations: Conversation[];
  activeConversationId: string | null;
  messages: Message[];
  isStreaming: boolean;
  streamingContent: string;
  streamingNotebookId: string | null;
  streamingMessageId: string | null;
  preparingMessageId: string | null;
  streamingError: string | null;
  streamingStatus: ChatStatusPayload | null;
  pendingEvidence: Record<string, ChatEvidencePayload>;
  /**
   * Current attempt's accepted assistant ID. Never carry acceptance across attempts.
   */
  pendingMessageIds: Record<string, true>;
  lastChatEventSeq: number;
  replayCursors: Record<string, number>;
  suggestedQuestions: string[];
  style: string;
  customGoal: string;
  responseLength: string;
  loadConversations: (notebookId: string) => Promise<void>;
  createConversation: (notebookId: string, requestMessageId?: string) => Promise<string>;
  deleteConversation: (notebookId: string, conversationId: string) => Promise<void>;
  setActiveConversation: (id: string | null) => void;
  loadMessages: (notebookId: string, conversationId: string) => Promise<void>;
  rehydrateConversation: (notebookId: string, conversationId: string) => Promise<void>;
  replayChatEvents: (notebookId: string, conversationId: string) => Promise<void>;
  sendMessage: (notebookId: string, query: string, sourceScope: SourceScope, model: string, historyBeforeUserMessageId?: string) => Promise<void>;
  stopStreaming: (notebookId: string) => Promise<void>;
  attachAssistantEvidence: (notebookId: string, conversationId: string, messageId: string, payload: ChatEvidencePayload) => void;
  appendToken: (notebookId: string, conversationId: string, messageId: string, token: string) => void;
  finalizeMessage: (notebookId: string, conversationId: string, messageId: string) => Promise<void>;
  setStreamingError: (notebookId: string, conversationId: string, messageId: string, error: string) => void;
  handleChatCancelled: (notebookId: string, conversationId: string, messageId: string, reason: string) => void;
  setStreamingStatus: (payload: ChatStatusPayload) => void;
  resetForNotebookSwitch: () => void;
  loadSuggestedQuestions: (notebookId: string) => Promise<void>;
  clearSuggestedQuestions: () => void;
  setStyle: (style: string) => void;
  setCustomGoal: (goal: string) => void;
  setResponseLength: (length: string) => void;
}

export const useChatStore = create<ChatStore>((set, get) => ({
  conversations: [],
  activeConversationId: null,
  messages: [],
  isStreaming: false,
  streamingContent: '',
  streamingNotebookId: null,
  streamingMessageId: null,
  preparingMessageId: null,
  streamingError: null,
  streamingStatus: null,
  pendingEvidence: {},
  pendingMessageIds: {},
  lastChatEventSeq: 0,
  replayCursors: {},
  suggestedQuestions: [],
  style: 'default',
  customGoal: '',
  responseLength: 'default',

  loadConversations: async (notebookId) => {
    try {
      const conversations = await api.listConversations(notebookId);
      if (useNotebookStore.getState().activeNotebookId !== notebookId) {
        return;
      }
      set({ conversations });
    } catch (e) {
      console.warn('Failed to load conversations:', e);
      useToastStore.getState().addToast({ type: 'error', title: 'Load Failed', message: 'Failed to load conversations', duration: 5000 });
    }
  },

  createConversation: async (notebookId, requestMessageId) => {
    const previousConversationId = get().activeConversationId;
    const id = await api.createConversation(notebookId);
    await get().loadConversations(notebookId);
    if ((requestMessageId !== undefined && get().streamingMessageId !== requestMessageId) ||
        useNotebookStore.getState().activeNotebookId !== notebookId ||
        get().activeConversationId !== previousConversationId) {
      return id;
    }
    set({ activeConversationId: id, messages: [] });
    return id;
  },

  deleteConversation: async (notebookId, conversationId) => {
    await api.deleteConversation(notebookId, conversationId);
    const { activeConversationId } = get();
    if (activeConversationId === conversationId) {
      set({
        activeConversationId: null,
        messages: [],
        isStreaming: false,
        streamingContent: '',
        streamingNotebookId: null,
        streamingMessageId: null,
        streamingError: null,
        streamingStatus: null,
        pendingEvidence: {},
      });
    }
    await get().loadConversations(notebookId);
  },

  setActiveConversation: (id) => set({ activeConversationId: id }),

  loadMessages: async (notebookId, conversationId) => {
    set({ activeConversationId: conversationId });
    await get().rehydrateConversation(notebookId, conversationId);
  },

  rehydrateConversation: async (notebookId, conversationId) => {
    try {
      const messages = await api.loadMessages(notebookId, conversationId);
      if (useNotebookStore.getState().activeNotebookId !== notebookId) {
        return;
      }
      if (get().activeConversationId !== conversationId) {
        return;
      }
      set({ messages });
    } catch (e) {
      console.warn('Failed to load messages:', e);
      useToastStore.getState().addToast({ type: 'error', title: 'Load Failed', message: 'Failed to load messages', duration: 5000 });
    }
  },

  replayChatEvents: async (notebookId, conversationId) => {
    const replayKey = `${notebookId}:${conversationId}`;
    const afterSeq = get().replayCursors[replayKey] ?? 0;
    const events = await api.getChatEventsSince(notebookId, conversationId, afterSeq);
    if (events.length === 0) return;
    const maxSeq = events.reduce((max, event) => Math.max(max, event.seq), afterSeq);
    set((state) => ({ replayCursors: { ...state.replayCursors, [replayKey]: maxSeq }, lastChatEventSeq: state.activeConversationId === conversationId && useNotebookStore.getState().activeNotebookId === notebookId ? maxSeq : state.lastChatEventSeq }));
    for (const event of events) {
      if (useNotebookStore.getState().activeNotebookId !== notebookId || get().activeConversationId !== conversationId) return;
      if (event.notebook_id !== notebookId || event.conversation_id !== conversationId) continue;
      const payload = event.payload as Record<string, unknown>;
      const messageId = typeof payload.message_id === 'string' ? payload.message_id : event.message_id;
      if (event.kind === 'token') {
        const token = typeof payload.token === 'string' ? payload.token : '';
        if (token) get().appendToken(event.notebook_id, event.conversation_id, messageId, token);
      } else if (event.kind === 'done') {
        await get().finalizeMessage(event.notebook_id, event.conversation_id, messageId);
      } else if (event.kind === 'evidence') {
        const citations = Array.isArray(payload.citations) ? payload.citations : [];
        get().attachAssistantEvidence(event.notebook_id, event.conversation_id, messageId, {
          citations,
          evidence: payload.evidence as ChatEvidencePayload['evidence'],
        });
      } else if (event.kind === 'status') {
        get().setStreamingStatus(payload as unknown as ChatStatusPayload);
      } else if (event.kind === 'error') {
        get().setStreamingError(
          event.notebook_id,
          event.conversation_id,
          messageId,
          typeof payload.error === 'string' ? payload.error : 'Chat request failed'
        );
      } else if (event.kind === 'cancelled') {
        get().handleChatCancelled(
          event.notebook_id,
          event.conversation_id,
          messageId,
          typeof payload.reason === 'string' ? payload.reason : 'Chat cancelled'
        );
      }
    }
  },

  sendMessage: async (notebookId, query, sourceScope, model, historyBeforeUserMessageId) => {
    if (get().isStreaming || useNotebookStore.getState().activeNotebookId !== notebookId) return;
    const assistantMessageId = crypto.randomUUID();
    const ownsRequest = () => get().streamingMessageId === assistantMessageId;
    // Reserve the single frontend stream before any asynchronous conversation creation.
    set({
      isStreaming: true, streamingNotebookId: notebookId,
      streamingMessageId: assistantMessageId, streamingContent: '',
      preparingMessageId: assistantMessageId,
      streamingError: null, streamingStatus: null,
      pendingMessageIds: { [assistantMessageId]: true }, pendingEvidence: {},
    });
    let userMsg: Message | null = null;
    try {
      let { activeConversationId } = get();
      if (!activeConversationId) {
        activeConversationId = await get().createConversation(notebookId, assistantMessageId);
      }
      if (!ownsRequest()) return;
      if (useNotebookStore.getState().activeNotebookId !== notebookId ||
          get().activeConversationId !== activeConversationId) {
        set({ isStreaming: false, streamingNotebookId: null, streamingMessageId: null,
          streamingContent: '', streamingStatus: null, pendingMessageIds: {}, pendingEvidence: {} });
        return;
      }
      userMsg = {
        id: crypto.randomUUID(), conversation_id: activeConversationId,
        role: 'user', content: query, created_at: new Date().toISOString(),
      };
      set((state) => ({
        messages: [...state.messages, userMsg!],
        preparingMessageId: null,
        streamingStatus: {
          notebook_id: notebookId, conversation_id: activeConversationId,
          message_id: assistantMessageId, phase: 'queued', message: 'Queued',
          elapsed_ms: 0, truncated: false,
        },
      }));
      const { style, customGoal, responseLength } = get();
      const messageId = await api.sendMessage(
        notebookId, activeConversationId, query, sourceScope, model, assistantMessageId,
        style !== 'default' ? style : undefined, customGoal || undefined,
        responseLength !== 'default' ? responseLength : undefined,
        historyBeforeUserMessageId, userMsg.id,
      );
      // Done/error may arrive before the invoke acknowledgement, or a newer
      // stream may already own the store. Neither case permits late mutation.
      if (!ownsRequest()) return;
      if (messageId !== assistantMessageId) {
        get().setStreamingError(notebookId, activeConversationId, assistantMessageId,
          'Chat protocol error: backend returned a different assistant message ID.');
      }
      // The acknowledgement is not a terminal event; keep the bound lifecycle.
    } catch (e) {
      if (!ownsRequest()) return;
      const message = errorMessage(e);
      console.warn('Failed to send message:', e);
      set((state) => ({
        messages: userMsg ? state.messages.filter((m) => m.id !== userMsg!.id) : state.messages,
        streamingError: useNotebookStore.getState().activeNotebookId === notebookId ? message : null,
        isStreaming: false, streamingContent: '', streamingNotebookId: null,
        streamingMessageId: null, streamingStatus: null,
        pendingMessageIds: {}, pendingEvidence: {},
      }));
    }
  },

  stopStreaming: async (notebookId) => {
    const requestedMessageId = get().streamingMessageId;
    if (get().streamingNotebookId !== notebookId) return;
    if (requestedMessageId && get().preparingMessageId === requestedMessageId) {
      // No backend attempt exists yet. Invalidate this owner synchronously so
      // a late conversation-creation response cannot submit or activate it.
      set({ isStreaming: false, preparingMessageId: null,
        streamingMessageId: null, streamingNotebookId: null,
        streamingContent: '', streamingStatus: null,
        pendingMessageIds: {}, pendingEvidence: {} });
      return;
    }
    try {
      await api.stopChat(notebookId);
      if (get().streamingMessageId !== requestedMessageId) return;
      const { isStreaming, streamingMessageId, activeConversationId } = get();
      if (!isStreaming || !streamingMessageId || !activeConversationId) return;
      // The command only acknowledges a cancellation request. The backend
      // stream task is authoritative for terminal cleanup via chat:cancelled,
      // chat:error, or chat:done.
      set({
        streamingStatus: {
          notebook_id: notebookId,
          conversation_id: activeConversationId,
          message_id: streamingMessageId,
          phase: 'cancelling',
          message: 'Cancellation requested',
          elapsed_ms: 0,
          truncated: false,
        },
      });
    } catch (error) {
      if (get().streamingMessageId !== requestedMessageId) return;
      const { activeConversationId, streamingMessageId } = get();
      if (!activeConversationId || !streamingMessageId) return;
      get().setStreamingError(
        notebookId,
        activeConversationId,
        streamingMessageId,
        `Cancellation request failed: ${errorMessage(error)}`
      );
    }
  },

  attachAssistantEvidence: (notebookId, _conversationId, messageId, payload) => {
    const { streamingNotebookId, pendingMessageIds } = get();
    if (streamingNotebookId && streamingNotebookId !== notebookId) return;
    if (!pendingMessageIds[messageId]) return;
    set((state) => ({
      pendingEvidence: { ...state.pendingEvidence, [messageId]: payload },
      messages: state.messages.map((message) =>
        message.id === messageId ? { ...message, citations: payload } : message
      ),
    }));
  },

  appendToken: (notebookId, _conversationId, messageId, token) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
      pendingMessageIds,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== notebookId) return;
    // Accept tokens for the currently-bound streamingMessageId OR any other
    // id we previously registered (covers the case where the backend returned
    // a different messageId from the one the frontend asked for, and tokens
    // were in flight before the streamingMessageId swap completed).
    const accepted =
      (streamingMessageId && streamingMessageId === messageId) ||
      pendingMessageIds[messageId];
    if (!accepted) return;
    // If the messageId isn't the currently-bound one but is in the pending
    // set, promote it to streamingMessageId so subsequent token/evidence/done
    // events line up with a single id.
    if (streamingMessageId !== messageId) {
      set({ streamingMessageId: messageId });
    }
    set((state) => ({
      streamingContent: state.streamingContent + token,
    }));
  },

  finalizeMessage: async (notebookId, conversationId, messageId) => {
    const {
      isStreaming,
      streamingMessageId,
      streamingNotebookId,
      pendingMessageIds,
    } = get();
    // Terminal event: MUST be processed even after a notebook switch so the
    // frontend exits streaming state. But an old notebook's assistant text must
    // not be appended into the newly active notebook message list.
    if (!isStreaming) {
      if (get().activeConversationId === conversationId) {
        await get().rehydrateConversation(notebookId, conversationId);
      }
      return;
    }
    const accepted =
      (streamingMessageId && streamingMessageId === messageId) ||
      pendingMessageIds[messageId];
    if (!accepted) return;
    set((state) => ({
      pendingEvidence: Object.fromEntries(
        Object.entries(state.pendingEvidence).filter(([id]) => id !== messageId)
      ),
      pendingMessageIds: Object.fromEntries(
        Object.entries(state.pendingMessageIds).filter(([id]) => id !== messageId)
      ),
      isStreaming: false,
      streamingContent: '',
      streamingNotebookId: null,
      streamingMessageId: null,
      streamingError: null,
      streamingStatus: null,
    }));
    const activeNotebookId = useNotebookStore.getState().activeNotebookId;
    if (streamingNotebookId === notebookId && activeNotebookId === notebookId && get().activeConversationId === conversationId) {
      await get().rehydrateConversation(notebookId, conversationId);
    }
  },

  setStreamingError: (_notebookId, _conversationId, messageId, error) => {
    const {
      isStreaming,
      streamingMessageId,
      pendingMessageIds,
    } = get();
    // Terminal event: MUST be processed regardless of notebookId — the
    // frontend must always exit streaming state on error. Match on messageId
    // to ensure we close the correct stream.
    if (!isStreaming) return;
    const accepted =
      (streamingMessageId && streamingMessageId === messageId) ||
      pendingMessageIds[messageId];
    if (!accepted) return;
    set((state) => ({
      streamingError: error,
      isStreaming: false,
      streamingContent: get().streamingContent,
      streamingNotebookId: null,
      streamingMessageId: null,
      pendingEvidence: Object.fromEntries(
        Object.entries(state.pendingEvidence).filter(([id]) => id !== messageId)
      ),
      pendingMessageIds: Object.fromEntries(
        Object.entries(state.pendingMessageIds).filter(([id]) => id !== messageId)
      ),
      streamingStatus: null,
    }));
  },

  handleChatCancelled: (_notebookId, _conversationId, messageId, reason) => {
    const {
      isStreaming,
      streamingMessageId,
      pendingMessageIds,
    } = get();
    // Terminal event: MUST be processed regardless of notebookId — the
    // frontend must always exit streaming state on cancellation. Match on
    // messageId to ensure we close the correct stream.
    if (!isStreaming) return;
    const accepted =
      (streamingMessageId && streamingMessageId === messageId) ||
      pendingMessageIds[messageId];
    if (!accepted) return;
    set((state) => ({
      streamingError: reason,
      isStreaming: false,
      streamingContent: '',
      streamingNotebookId: null,
      streamingMessageId: null,
      pendingEvidence: Object.fromEntries(
        Object.entries(state.pendingEvidence).filter(([id]) => id !== messageId)
      ),
      pendingMessageIds: Object.fromEntries(
        Object.entries(state.pendingMessageIds).filter(([id]) => id !== messageId)
      ),
      streamingStatus: null,
    }));
  },

  setStreamingStatus: (payload) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
      pendingMessageIds,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== payload.notebook_id) return;
    const accepted =
      (streamingMessageId && streamingMessageId === payload.message_id) ||
      pendingMessageIds[payload.message_id];
    if (!accepted) return;
    set({ streamingStatus: payload });
  },

  resetForNotebookSwitch: () => {
    const current = get();
    set({
      conversations: [],
      activeConversationId: null,
      messages: [],
      isStreaming: current.isStreaming,
      streamingContent: current.isStreaming ? current.streamingContent : '',
      streamingNotebookId: current.isStreaming ? current.streamingNotebookId : null,
      streamingMessageId: current.isStreaming ? current.streamingMessageId : null,
      streamingError: current.isStreaming ? current.streamingError : null,
      streamingStatus: current.isStreaming ? current.streamingStatus : null,
      pendingEvidence: current.isStreaming ? current.pendingEvidence : {},
      pendingMessageIds: current.isStreaming ? current.pendingMessageIds : {},
      lastChatEventSeq: current.lastChatEventSeq,
      replayCursors: current.replayCursors,
      suggestedQuestions: [],
    });
  },

  loadSuggestedQuestions: async (notebookId) => {
    try {
      const questions = await api.getSuggestedQuestions(notebookId);
      if (useNotebookStore.getState().activeNotebookId !== notebookId) {
        return;
      }
      set({ suggestedQuestions: questions });
    } catch {
      if (useNotebookStore.getState().activeNotebookId !== notebookId) {
        return;
      }
      set({ suggestedQuestions: [] });
    }
  },

  clearSuggestedQuestions: () => {
    set({ suggestedQuestions: [] });
  },

  setStyle: (style) => set({ style }),
  setCustomGoal: (goal) => set({ customGoal: goal }),
  setResponseLength: (length) => set({ responseLength: length }),
}));
