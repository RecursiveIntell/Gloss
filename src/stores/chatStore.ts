import { create } from 'zustand';
import type { Conversation, Message } from '../lib/types';
import * as api from '../lib/tauri';

interface ChatStore {
  conversations: Conversation[];
  activeConversationId: string | null;
  messages: Message[];
  isStreaming: boolean;
  streamingContent: string;
  streamingMessageId: string | null;
  streamingError: string | null;
  suggestedQuestions: string[];
  loadConversations: (notebookId: string) => Promise<void>;
  createConversation: (notebookId: string) => Promise<string>;
  deleteConversation: (notebookId: string, conversationId: string) => Promise<void>;
  setActiveConversation: (id: string | null) => void;
  loadMessages: (notebookId: string, conversationId: string) => Promise<void>;
  sendMessage: (notebookId: string, query: string, selectedSourceIds: string[], model: string) => Promise<void>;
  appendToken: (messageId: string, token: string) => void;
  finalizeMessage: (messageId: string) => void;
  setStreamingError: (messageId: string, error: string) => void;
  resetForNotebookSwitch: () => void;
  loadSuggestedQuestions: (notebookId: string) => Promise<void>;
}

export const useChatStore = create<ChatStore>((set, get) => ({
  conversations: [],
  activeConversationId: null,
  messages: [],
  isStreaming: false,
  streamingContent: '',
  streamingMessageId: null,
  streamingError: null,
  suggestedQuestions: [],

  loadConversations: async (notebookId) => {
    try {
      const conversations = await api.listConversations(notebookId);
      set({ conversations });
    } catch (e) {
      console.error('Failed to load conversations:', e);
    }
  },

  createConversation: async (notebookId) => {
    const id = await api.createConversation(notebookId);
    await get().loadConversations(notebookId);
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
    try {
      const messages = await api.loadMessages(notebookId, conversationId);
      set({ messages, activeConversationId: conversationId });
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
      set({ streamingMessageId: messageId });
    } catch (e) {
      console.error('Failed to send message:', e);
      set({ isStreaming: false });
    }
  },

  appendToken: (messageId, token) => {
    // Guard: ignore tokens from a stale streaming session
    const { streamingMessageId } = get();
    if (streamingMessageId && streamingMessageId !== messageId) return;
    set((state) => ({
      streamingContent: state.streamingContent + token,
    }));
  },

  finalizeMessage: (messageId) => {
    // Guard: ignore finalize for a stale message
    const { streamingMessageId } = get();
    if (streamingMessageId && streamingMessageId !== messageId) return;
    const finalContent = get().streamingContent;
    const { activeConversationId } = get();
    const assistantMsg: Message = {
      id: messageId,
      conversation_id: activeConversationId || '',
      role: 'assistant',
      content: finalContent,
      created_at: new Date().toISOString(),
    };
    set((state) => ({
      messages: [...state.messages, assistantMsg],
      isStreaming: false,
      streamingContent: '',
      streamingMessageId: null,
      streamingError: null,
    }));
  },

  setStreamingError: (messageId, error) => {
    const { streamingMessageId } = get();
    if (streamingMessageId && streamingMessageId !== messageId) return;
    set({
      streamingError: error,
      isStreaming: false,
      streamingContent: '',
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
      streamingMessageId: null,
      streamingError: null,
      suggestedQuestions: [],
    });
  },

  loadSuggestedQuestions: async (notebookId) => {
    try {
      const questions = await api.getSuggestedQuestions(notebookId);
      set({ suggestedQuestions: questions });
    } catch {
      set({ suggestedQuestions: [] });
    }
  },
}));
