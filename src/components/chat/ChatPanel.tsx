import { useState, useRef, useEffect } from "react";
import { useChatStore } from "../../stores/chatStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { useSourceStore } from "../../stores/sourceStore";
import { Send, Plus, MessageSquare, Loader2, AlertCircle } from "lucide-react";
import ReactMarkdown from "react-markdown";

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
    sendMessage,
    createConversation,
    setActiveConversation,
    loadMessages,
    suggestedQuestions,
  } = useChatStore();
  const { activeModel, models } = useSettingsStore();
  const { selectedSourceIds } = useSourceStore();
  const setActiveModel = useSettingsStore((s) => s.setActiveModel);

  const [input, setInput] = useState("");
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, streamingContent]);

  const handleSend = async () => {
    if (!input.trim() || isStreaming) return;
    const query = input.trim();
    setInput("");
    await sendMessage(notebookId, query, Array.from(selectedSourceIds), activeModel);
  };

  const handleSuggestionClick = (question: string) => {
    setInput(question);
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="p-2 border-b border-border flex items-center justify-between">
        <div className="flex items-center gap-2">
          <button
            onClick={() => createConversation(notebookId)}
            className="flex items-center gap-1 px-2 py-1 text-xs bg-bg-tertiary rounded hover:bg-border text-text-secondary hover:text-text"
          >
            <Plus className="w-3 h-3" /> New Chat
          </button>
          {conversations.length > 0 && (
            <select
              value={activeConversationId || ""}
              onChange={(e) => {
                const id = e.target.value;
                if (id) {
                  setActiveConversation(id);
                  loadMessages(notebookId, id);
                }
              }}
              className="text-xs bg-bg-tertiary border border-border rounded px-2 py-1 text-text focus:outline-none focus:border-accent"
            >
              <option value="">Select conversation</option>
              {conversations.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.title || `Chat ${c.id.slice(0, 8)}`}
                </option>
              ))}
            </select>
          )}
        </div>
        <select
          value={activeModel}
          onChange={(e) => setActiveModel(e.target.value)}
          className="text-xs bg-bg-tertiary border border-border rounded px-2 py-1 text-text focus:outline-none focus:border-accent"
        >
          {models.length > 0 ? (
            models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.display_name}
              </option>
            ))
          ) : (
            <option value={activeModel}>{activeModel}</option>
          )}
        </select>
      </div>

      {/* Messages */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {messages.length === 0 && !isStreaming && (
          <div className="text-center mt-8">
            <MessageSquare className="w-10 h-10 text-text-muted mx-auto mb-3" />
            <p className="text-sm text-text-secondary mb-4">
              Ask a question about your sources
            </p>
            {suggestedQuestions.length > 0 && (
              <div className="flex flex-wrap gap-2 justify-center">
                {suggestedQuestions.map((q, i) => (
                  <button
                    key={i}
                    onClick={() => handleSuggestionClick(q)}
                    className="px-3 py-1.5 text-xs bg-bg-tertiary rounded-full hover:bg-border text-text-secondary hover:text-text"
                  >
                    {q}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        {messages.map((msg) => (
          <div
            key={msg.id}
            className={`flex ${msg.role === "user" ? "justify-end" : "justify-start"}`}
          >
            <div
              className={`max-w-[80%] rounded-lg px-3 py-2 text-sm ${
                msg.role === "user"
                  ? "bg-accent text-white"
                  : "bg-bg-tertiary text-text"
              }`}
            >
              {msg.role === "assistant" ? (
                <div className="prose prose-invert prose-sm max-w-none">
                  <ReactMarkdown>{msg.content}</ReactMarkdown>
                </div>
              ) : (
                <p>{msg.content}</p>
              )}
            </div>
          </div>
        ))}

        {isStreaming && streamingContent && (
          <div className="flex justify-start">
            <div className="max-w-[80%] rounded-lg px-3 py-2 text-sm bg-bg-tertiary text-text">
              <div className="prose prose-invert prose-sm max-w-none">
                <ReactMarkdown>{streamingContent}</ReactMarkdown>
              </div>
            </div>
          </div>
        )}

        {isStreaming && !streamingContent && (
          <div className="flex justify-start">
            <div className="rounded-lg px-3 py-2 bg-bg-tertiary">
              <Loader2 className="w-4 h-4 text-text-muted animate-spin" />
            </div>
          </div>
        )}

        {streamingError && (
          <div className="flex justify-start">
            <div className="max-w-[80%] rounded-lg px-3 py-2 text-sm bg-error/10 border border-error/30 text-error flex items-start gap-2">
              <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
              <span>{streamingError}</span>
            </div>
          </div>
        )}

        <div ref={messagesEndRef} />
      </div>

      {/* Input */}
      <div className="p-3 border-t border-border">
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && handleSend()}
            placeholder="Ask about your sources..."
            disabled={isStreaming}
            className="flex-1 px-3 py-2 text-sm bg-bg-tertiary border border-border rounded-lg text-text placeholder:text-text-muted focus:outline-none focus:border-accent disabled:opacity-50"
          />
          <button
            onClick={handleSend}
            disabled={!input.trim() || isStreaming}
            className="p-2 rounded-lg bg-accent text-white hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Send className="w-4 h-4" />
          </button>
        </div>
      </div>
    </div>
  );
}
