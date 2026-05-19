import { create } from 'zustand';
import type { ChatEvidencePayload, ChatStatusPayload, Conversation, Message, SourceScope } from '../lib/types';
import * as api from '../lib/tauri';

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';

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
  streamingError: string | null;
  streamingStatus: ChatStatusPayload | null;
  pendingEvidence: Record<string, ChatEvidencePayload>;
  suggestedQuestions: string[];
  loadConversations: (notebookId: string) => Promise<void>;
  createConversation: (notebookId: string) => Promise<string>;
  deleteConversation: (notebookId: string, conversationId: string) => Promise<void>;
  setActiveConversation: (id: string | null) => void;
  loadMessages: (notebookId: string, conversationId: string) => Promise<void>;
  sendMessage: (notebookId: string, query: string, sourceScope: SourceScope, model: string) => Promise<void>;
  stopStreaming: (notebookId: string) => Promise<void>;
  attachAssistantEvidence: (notebookId: string, conversationId: string, messageId: string, payload: ChatEvidencePayload) => void;
  appendToken: (notebookId: string, conversationId: string, messageId: string, token: string) => void;
  finalizeMessage: (notebookId: string, conversationId: string, messageId: string) => void;
  setStreamingError: (notebookId: string, conversationId: string, messageId: string, error: string) => void;
  setStreamingStatus: (payload: ChatStatusPayload) => void;
  resetForNotebookSwitch: () => void;
  loadSuggestedQuestions: (notebookId: string) => Promise<void>;
  clearSuggestedQuestions: () => void;
}

export const useChatStore = create<ChatStore>((set, get) => ({
  conversations: [],
  activeConversationId: null,
  messages: [],
  isStreaming: false,
  streamingContent: '',
  streamingNotebookId: null,
  streamingMessageId: null,
  streamingError: null,
  streamingStatus: null,
  pendingEvidence: {},
  suggestedQuestions: [],

  loadConversations: async (notebookId) => {
    try {
      const conversations = await api.listConversations(notebookId);
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      set({ conversations });
    } catch (e) {
      console.error('Failed to load conversations:', e);
    }
  },

  createConversation: async (notebookId) => {
    const id = await api.createConversation(notebookId);
    await get().loadConversations(notebookId);
    if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
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
    try {
      const messages = await api.loadMessages(notebookId, conversationId);
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      if (get().activeConversationId !== conversationId) {
        return;
      }
      set({ messages });
    } catch (e) {
      console.error('Failed to load messages:', e);
    }
  },

  sendMessage: async (notebookId, query, sourceScope, model) => {
    let { activeConversationId } = get();
    if (!activeConversationId) {
      activeConversationId = await get().createConversation(notebookId);
    }

    const assistantMessageId = crypto.randomUUID();

    // Add user message to local state immediately
    const userMsg: Message = {
      id: crypto.randomUUID(),
      conversation_id: activeConversationId,
      role: 'user',
      content: query,
      created_at: new Date().toISOString(),
    };
    set((state) => ({
      messages: [...state.messages, userMsg],
      isStreaming: true,
      streamingContent: '',
      streamingNotebookId: notebookId,
      streamingMessageId: assistantMessageId,
      streamingError: null,
      streamingStatus: {
        notebook_id: notebookId,
        conversation_id: activeConversationId,
        message_id: assistantMessageId,
        phase: 'queued',
        message: 'Queued',
        elapsed_ms: 0,
        truncated: false,
      },
    }));

    try {
      const messageId = await api.sendMessage(
        notebookId,
        activeConversationId,
        query,
        sourceScope,
        model,
        assistantMessageId
      );
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      if (get().activeConversationId !== activeConversationId) {
        return;
      }
      if (messageId !== assistantMessageId) {
        set({ streamingMessageId: messageId });
      }
    } catch (e) {
      const message = errorMessage(e);
      console.error('Failed to send message:', e);
      set({
        streamingError: message,
        isStreaming: false,
        streamingContent: '',
        streamingNotebookId: null,
        streamingMessageId: null,
        streamingStatus: null,
      });
    }
  },

  stopStreaming: async (notebookId) => {
    try {
      await api.stopChat(notebookId);
    } finally {
      const { isStreaming, streamingMessageId, activeConversationId, streamingContent } = get();
      if (!isStreaming || !streamingMessageId || !activeConversationId) return;
      const pendingEvidence = get().pendingEvidence[streamingMessageId];
      const assistantMsg: Message = {
        id: streamingMessageId,
        conversation_id: activeConversationId,
        role: 'assistant',
        content: streamingContent || '[Stopped]',
        citations: pendingEvidence,
        created_at: new Date().toISOString(),
      };
      set((state) => ({
        messages: streamingContent ? [...state.messages, assistantMsg] : state.messages,
        pendingEvidence: Object.fromEntries(
          Object.entries(state.pendingEvidence).filter(([id]) => id !== streamingMessageId)
        ),
        isStreaming: false,
        streamingContent: '',
        streamingNotebookId: null,
        streamingMessageId: null,
        streamingError: null,
        streamingStatus: null,
      }));
    }
  },

  attachAssistantEvidence: (notebookId, _conversationId, messageId, payload) => {
    const { streamingNotebookId } = get();
    if (streamingNotebookId && streamingNotebookId !== notebookId) return;
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
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== notebookId) return;
    if (!streamingMessageId || streamingMessageId !== messageId) return;
    set((state) => ({
      streamingContent: state.streamingContent + token,
    }));
  },

  finalizeMessage: (notebookId, conversationId, messageId) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== notebookId) return;
    if (!streamingMessageId || streamingMessageId !== messageId) return;
    const finalContent = get().streamingContent;
    if (!finalContent.trim()) {
      set((state) => ({
        streamingError: 'Chat completed without response content.',
        isStreaming: false,
        streamingContent: '',
        streamingNotebookId: null,
        streamingMessageId: null,
        streamingStatus: null,
        pendingEvidence: Object.fromEntries(
          Object.entries(state.pendingEvidence).filter(([id]) => id !== messageId)
        ),
      }));
      return;
    }
    const pendingEvidence = get().pendingEvidence[messageId];
    const assistantMsg: Message = {
      id: messageId,
      conversation_id: conversationId,
      role: 'assistant',
      content: finalContent,
      citations: pendingEvidence,
      created_at: new Date().toISOString(),
    };
    set((state) => ({
      messages: [...state.messages, assistantMsg],
      pendingEvidence: Object.fromEntries(
        Object.entries(state.pendingEvidence).filter(([id]) => id !== messageId)
      ),
      isStreaming: false,
      streamingContent: '',
      streamingNotebookId: null,
      streamingMessageId: null,
      streamingError: null,
      streamingStatus: null,
    }));
  },

  setStreamingError: (notebookId, _conversationId, messageId, error) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== notebookId) return;
    if (!streamingMessageId || streamingMessageId !== messageId) return;
    set({
      streamingError: error,
      isStreaming: false,
      streamingContent: '',
      streamingNotebookId: null,
      streamingMessageId: null,
      pendingEvidence: {},
      streamingStatus: null,
    });
  },

  setStreamingStatus: (payload) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== payload.notebook_id) return;
    if (!streamingMessageId || streamingMessageId !== payload.message_id) return;
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
      suggestedQuestions: [],
    });
  },

  loadSuggestedQuestions: async (notebookId) => {
    try {
      const questions = await api.getSuggestedQuestions(notebookId);
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      set({ suggestedQuestions: questions });
    } catch {
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      set({ suggestedQuestions: [] });
    }
  },

  clearSuggestedQuestions: () => {
    set({ suggestedQuestions: [] });
  },
}));
