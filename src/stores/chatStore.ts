import { create } from 'zustand';
import type { Conversation, Message } from '../lib/types';
import * as api from '../lib/tauri';

const ACTIVE_NB_KEY = 'gloss:activeNotebookId';

interface ChatStore {
  conversations: Conversation[];
  activeConversationId: string | null;
  messages: Message[];
  isStreaming: boolean;
  streamingContent: string;
  streamingNotebookId: string | null;
  streamingMessageId: string | null;
  streamingError: string | null;
  suggestedQuestions: string[];
  loadConversations: (notebookId: string) => Promise<void>;
  createConversation: (notebookId: string) => Promise<string>;
  deleteConversation: (notebookId: string, conversationId: string) => Promise<void>;
  setActiveConversation: (id: string | null) => void;
  loadMessages: (notebookId: string, conversationId: string) => Promise<void>;
  sendMessage: (notebookId: string, query: string, selectedSourceIds: string[], model: string) => Promise<void>;
  appendToken: (notebookId: string, conversationId: string, messageId: string, token: string) => void;
  finalizeMessage: (notebookId: string, conversationId: string, messageId: string) => void;
  setStreamingError: (notebookId: string, conversationId: string, messageId: string, error: string) => void;
  resetForNotebookSwitch: () => void;
  loadSuggestedQuestions: (notebookId: string) => Promise<void>;
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
      set({ activeConversationId: null, messages: [] });
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

  sendMessage: async (notebookId, query, selectedSourceIds, model) => {
    let { activeConversationId } = get();
    if (!activeConversationId) {
      activeConversationId = await get().createConversation(notebookId);
    }

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
      streamingError: null,
    }));

    try {
      const messageId = await api.sendMessage(
        notebookId,
        activeConversationId,
        query,
        selectedSourceIds,
        model
      );
      if (localStorage.getItem(ACTIVE_NB_KEY) !== notebookId) {
        return;
      }
      if (get().activeConversationId !== activeConversationId) {
        return;
      }
      set({ streamingMessageId: messageId });
    } catch (e) {
      console.error('Failed to send message:', e);
      set({
        isStreaming: false,
        streamingContent: '',
        streamingNotebookId: null,
        streamingMessageId: null,
      });
    }
  },

  appendToken: (notebookId, conversationId, messageId, token) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
      activeConversationId,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== notebookId) return;
    if (activeConversationId !== conversationId) return;
    if (streamingMessageId && streamingMessageId !== messageId) return;
    set((state) => ({
      streamingContent: state.streamingContent + token,
    }));
  },

  finalizeMessage: (notebookId, conversationId, messageId) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
      activeConversationId,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== notebookId) return;
    if (activeConversationId !== conversationId) return;
    if (streamingMessageId && streamingMessageId !== messageId) return;
    const finalContent = get().streamingContent;
    const assistantMsg: Message = {
      id: messageId,
      conversation_id: conversationId,
      role: 'assistant',
      content: finalContent,
      created_at: new Date().toISOString(),
    };
    set((state) => ({
      messages: [...state.messages, assistantMsg],
      isStreaming: false,
      streamingContent: '',
      streamingNotebookId: null,
      streamingMessageId: null,
      streamingError: null,
    }));
  },

  setStreamingError: (notebookId, conversationId, messageId, error) => {
    const {
      isStreaming,
      streamingNotebookId,
      streamingMessageId,
      activeConversationId,
    } = get();
    if (!isStreaming) return;
    if (streamingNotebookId !== notebookId) return;
    if (activeConversationId !== conversationId) return;
    if (streamingMessageId && streamingMessageId !== messageId) return;
    set({
      streamingError: error,
      isStreaming: false,
      streamingContent: '',
      streamingNotebookId: null,
      streamingMessageId: null,
    });
  },

  resetForNotebookSwitch: () => {
    set({
      conversations: [],
      activeConversationId: null,
      messages: [],
      isStreaming: false,
      streamingContent: '',
      streamingNotebookId: null,
      streamingMessageId: null,
      streamingError: null,
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
}));
