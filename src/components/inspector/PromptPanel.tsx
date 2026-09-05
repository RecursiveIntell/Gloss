import { useChatStore } from "../../stores/chatStore";
import type { DecodingSettingsReceiptV1, PromptReceiptV1, GenerationReceiptV1 } from "../../lib/types";
import { Copy, CheckCircle, XCircle } from "lucide-react";
import { useState, useEffect, useRef } from "react";

/**
 * Prompt Inspector panel — shows system prompt info, retrieval context,
 * prompt receipts, and decoding settings for the most recent assistant message.
 */
export function PromptPanel() {
  const messages = useChatStore((s) => s.messages);
  const isStreaming = useChatStore((s) => s.isStreaming);
  const streamingContent = useChatStore((s) => s.streamingContent);

  const lastAssistant = [...messages]
    .reverse()
    .find((m) => m.role === "assistant");

  const evidence = lastAssistant?.citations?.evidence;
  const promptReceipt: PromptReceiptV1 | undefined = evidence?.prompt_receipt ?? undefined;
  const decodingReceipt: DecodingSettingsReceiptV1 | undefined = evidence?.decoding_settings_receipt ?? undefined;
  const generationReceipt: GenerationReceiptV1 | undefined = evidence?.generation_receipt ?? undefined;

  if (isStreaming && streamingContent && !evidence) {
    return (
      <div className="flex h-full items-center justify-center p-4 text-center text-xs text-text-muted">
        Generating — prompt data arrives after response completes
      </div>
    );
  }

  if (!evidence) {
    return (
      <div className="p-3 space-y-3 text-xs">
        <div className="text-text-muted text-center py-8">
          No prompt data available yet. Send a chat message to generate prompt receipts.
        </div>
      </div>
    );
  }

  return (
    <div className="p-3 space-y-3 text-xs overflow-y-auto h-full">
      <div role="status" className="text-text-muted">
        {isStreaming ? "Showing the previous completed response; the current prompt is not yet available." : "Showing the latest assistant response."}
      </div>
      {lastAssistant && <CopyableLabel label="Message ID" value={lastAssistant.id} />}
      {/* Prompt Receipt */}
      {promptReceipt && (
        <Section title="Prompt Receipt">
          <KVRow label="Schema" value={promptReceipt.schema} />
          <KVRow label="Receipt ID" value={promptReceipt.receipt_id.slice(0, 16)} />
          <KVRow label="Capture" value={promptReceipt.capture_state} />
          <KVRow label="Redaction" value={promptReceipt.redaction_state} />
          <KVRow label="System prompt" value={promptReceipt.system_prompt_digest.slice(0, 16)} mono />
          <KVRow label="User turn" value={promptReceipt.user_turn_digest.slice(0, 16)} mono />
          <KVRow label="Context digest" value={promptReceipt.context_payload_digest.slice(0, 16)} mono />
          <KVRow label="Source passages" value={`${promptReceipt.source_passage_count}`} />
          <KVRow label="Prompt digest" value={promptReceipt.prompt_digest.slice(0, 16)} mono />
          <CopyableLabel label="Prompt receipt ID" value={promptReceipt.receipt_id} />
        </Section>
      )}

      {/* System Prompt — the actual text sent to the model */}
      {promptReceipt?.system_prompt_text && (
        <Section title="System Prompt">
          <div className="rounded border border-border bg-bg-tertiary p-2 font-mono text-[11px] text-text-secondary whitespace-pre-wrap leading-relaxed max-h-96 overflow-y-auto">
            {promptReceipt.system_prompt_text}
          </div>
          <div className="flex items-center gap-2 mt-1">
            <button
              onClick={() => {
                navigator.clipboard?.writeText(promptReceipt.system_prompt_text!)
                  .catch((err: unknown) => console.warn("Failed to copy system prompt:", err));
              }}
              className="flex items-center gap-1 rounded px-2 py-0.5 text-[10px] text-text-muted hover:bg-bg-tertiary hover:text-text"
            >
              <Copy className="w-3 h-3" />
              Copy full prompt
            </button>
            <span className="text-[10px] text-text-muted">
              ~{promptReceipt.system_prompt_text.length.toLocaleString()} chars
            </span>
          </div>
        </Section>
      )}
      {!promptReceipt?.system_prompt_text && (
        <Section title="System Prompt">
          <p className="text-text-muted">{promptReceipt ? "System prompt text is not included in this receipt." : "System prompt text was not captured for this response."}</p>
        </Section>
      )}

      {/* Decoding Settings Receipt */}
      {decodingReceipt && (
        <Section title="Decoding Settings">
          <KVRow label="Provider" value={decodingReceipt.provider} />
          <KVRow label="Model" value={decodingReceipt.model} />
          <KVRow label="Temperature" value={`${decodingReceipt.effective.temperature}`} />
          {decodingReceipt.effective.top_p != null && (
            <KVRow label="Top P" value={`${decodingReceipt.effective.top_p}`} />
          )}
          {decodingReceipt.effective.top_k != null && (
            <KVRow label="Top K" value={`${decodingReceipt.effective.top_k}`} />
          )}
          {decodingReceipt.effective.min_p != null && (
            <KVRow label="Min P" value={`${decodingReceipt.effective.min_p}`} />
          )}
          {decodingReceipt.effective.repeat_penalty != null && (
            <KVRow label="Repeat penalty" value={`${decodingReceipt.effective.repeat_penalty}`} />
          )}
          <KVRow label="Max tokens" value={`${decodingReceipt.effective.max_tokens}`} />
          {decodingReceipt.unsupported_fields.length > 0 && (
            <KVRow label="Unsupported" value={decodingReceipt.unsupported_fields.join(", ")} warn />
          )}
          <KVRow label="Receipt ID" value={decodingReceipt.receipt_id.slice(0, 16)} />
          <CopyableLabel label="Decoding receipt ID" value={decodingReceipt.receipt_id} />
        </Section>
      )}

      {/* Generation Receipt */}
      {generationReceipt && (
        <Section title="Generation Receipt">
          <KVRow label="Provider" value={generationReceipt.provider} />
          <KVRow label="Model" value={generationReceipt.model} />
          <KVRow label="Status" value={generationReceipt.status} warn={generationReceipt.status !== "complete"} />
          {generationReceipt.error && (
            <div className="flex items-start gap-1 rounded bg-error/10 px-2 py-1 text-error">
              <XCircle className="w-3 h-3 shrink-0 mt-0.5" />
              <span>{generationReceipt.error}</span>
            </div>
          )}
          {generationReceipt.terminal_cause && (
            <KVRow label="Terminal cause" value={generationReceipt.terminal_cause} />
          )}
          <KVRow label="Done frame" value={generationReceipt.done_frame_seen ? "yes" : "no"} />
          <KVRow label="EOF seen" value={generationReceipt.eof_seen ? "yes" : "no"} />
          <KVRow label="Partial" value={generationReceipt.partial_persisted ? "yes" : "no"} />
          <KVRow label="Chunks seen" value={`${generationReceipt.chunks_seen}`} />
          <KVRow label="Request digest" value={generationReceipt.provider_request_digest.slice(0, 16)} mono />
          {generationReceipt.response_digest && (
            <KVRow label="Response digest" value={generationReceipt.response_digest.slice(0, 16)} mono />
          )}
          <CopyableLabel label="Generation receipt ID" value={generationReceipt.receipt_id} />
        </Section>
      )}

      {/* Prompt digest from evidence (even without receipt) */}
      {evidence.prompt_digest && !promptReceipt && (
        <Section title="Prompt Digest">
          <KVRow label="Digest" value={evidence.prompt_digest.slice(0, 16)} mono />
        </Section>
      )}

      {/* Context info */}
      <Section title="Context">
        <KVRow label="Passages" value={`${evidence.context_passage_count}`} />
        <KVRow label="Context digest" value={evidence.context_digest.slice(0, 16)} mono />
        <KVRow label="Source ctx digest" value={evidence.source_context_digest.slice(0, 16)} mono />
      </Section>
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

function KVRow({ label, value, warn, mono }: { label: string; value: string; warn?: boolean; mono?: boolean }) {
  return (
    <div className="grid grid-cols-[5.5rem_1fr] gap-1">
      <span className="text-text-muted truncate">{label}</span>
      <span className={`break-words ${mono ? "font-mono" : ""} ${warn ? "text-warning" : "text-text-secondary"}`}>
        {value}
      </span>
    </div>
  );
}

function CopyableLabel({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);
  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(value);
    } catch (err) {
      console.warn("Failed to copy:", err);
      return;
    }
    setCopied(true);
    setTimeout(() => { if (mountedRef.current) setCopied(false); }, 1500);
  };
  return (
    <div className="flex items-center gap-1">
      <span className="text-text-muted text-[10px]">{label}:</span>
      <span className="font-mono text-text-secondary text-[10px] truncate">{value.slice(0, 24)}</span>
      <button
        onClick={handleCopy}
        className="shrink-0 rounded p-0.5 hover:bg-bg-tertiary text-text-muted hover:text-text"
        title="Copy full ID"
      >
        {copied ? <CheckCircle className="w-3 h-3 text-success" /> : <Copy className="w-3 h-3" />}
      </button>
    </div>
  );
}
