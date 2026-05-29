import { useChatStore } from "../../stores/chatStore";
import type { GenerationReceiptV1, DecodingSettingsReceiptV1, PromptReceiptV1 } from "../../lib/types";
import { CheckCircle, XCircle, AlertTriangle, Copy } from "lucide-react";
import { useState } from "react";

/**
 * Receipt Inspector panel — summarizes the generation, decoding, and prompt
 * receipts for the most recent assistant message, with copyable receipt IDs
 * and a status overview.
 */
export function ReceiptPanel() {
  const messages = useChatStore((s) => s.messages);
  const isStreaming = useChatStore((s) => s.isStreaming);
  const streamingContent = useChatStore((s) => s.streamingContent);

  const lastAssistantWithEvidence = [...messages]
    .reverse()
    .find((m) => m.role === "assistant" && m.citations?.evidence);

  const evidence = lastAssistantWithEvidence?.citations?.evidence;
  const promptReceipt: PromptReceiptV1 | undefined = evidence?.prompt_receipt ?? undefined;
  const decodingReceipt: DecodingSettingsReceiptV1 | undefined = evidence?.decoding_settings_receipt ?? undefined;
  const generationReceipt: GenerationReceiptV1 | undefined = evidence?.generation_receipt ?? undefined;

  if (isStreaming && streamingContent && !evidence) {
    return (
      <div className="flex h-full items-center justify-center p-4 text-center text-xs text-text-muted">
        Generating — receipts arrive after response completes
      </div>
    );
  }

  if (!evidence) {
    return (
      <div className="p-3 space-y-3 text-xs">
        <div className="text-text-muted text-center py-8">
          No receipts available yet. Send a chat message to generate receipts.
        </div>
      </div>
    );
  }

  const generationOk = generationReceipt?.status === "complete" && !generationReceipt?.error;
  const generationFailed = generationReceipt?.status !== "complete" || !!generationReceipt?.error;

  return (
    <div className="p-3 space-y-3 text-xs overflow-y-auto h-full">
      {/* Status Overview */}
      <div className="rounded border border-border p-2 space-y-1">
        <div className="font-medium text-text">Generation Status</div>
        <div className="flex items-center gap-1.5">
          {generationOk ? (
            <CheckCircle className="w-3.5 h-3.5 text-success" />
          ) : generationFailed ? (
            <XCircle className="w-3.5 h-3.5 text-error" />
          ) : (
            <AlertTriangle className="w-3.5 h-3.5 text-warning" />
          )}
          <span className={generationOk ? "text-success" : generationFailed ? "text-error" : "text-warning"}>
            {generationReceipt?.status ?? "unknown"}
          </span>
          {generationReceipt?.terminal_cause && (
            <span className="text-text-muted">({generationReceipt.terminal_cause})</span>
          )}
        </div>
        {generationReceipt?.error && (
          <div className="rounded bg-error/10 px-2 py-1 text-error mt-1">
            {generationReceipt.error}
          </div>
        )}
      </div>

      {/* Generation Receipt */}
      {generationReceipt && (
        <ReceiptCard
          title="Generation Receipt"
          schema={generationReceipt.schema}
          receiptId={generationReceipt.receipt_id}
          fields={[
            { label: "Provider", value: generationReceipt.provider },
            { label: "Model", value: generationReceipt.model },
            { label: "Done frame", value: generationReceipt.done_frame_seen ? "yes" : "no" },
            { label: "EOF seen", value: generationReceipt.eof_seen ? "yes" : "no" },
            { label: "Partial", value: generationReceipt.partial_persisted ? "yes" : "no" },
            { label: "Chunks", value: `${generationReceipt.chunks_seen}` },
            { label: "Prompt receipt", value: generationReceipt.prompt_receipt_id.slice(0, 16), mono: true },
            { label: "Decoding receipt", value: generationReceipt.decoding_settings_receipt_id.slice(0, 16), mono: true },
          ]}
        />
      )}

      {/* Decoding Settings Receipt */}
      {decodingReceipt && (
        <ReceiptCard
          title="Decoding Settings"
          schema={decodingReceipt.schema}
          receiptId={decodingReceipt.receipt_id}
          fields={[
            { label: "Provider", value: decodingReceipt.provider },
            { label: "Model", value: decodingReceipt.model },
            { label: "Temperature", value: `${decodingReceipt.effective.temperature}` },
            { label: "Max tokens", value: `${decodingReceipt.effective.max_tokens}` },
            ...(decodingReceipt.unsupported_fields.length > 0
              ? [{ label: "Unsupported", value: decodingReceipt.unsupported_fields.join(", "), warn: true }]
              : []),
          ]}
        />
      )}

      {/* Prompt Receipt */}
      {promptReceipt && (
        <ReceiptCard
          title="Prompt Receipt"
          schema={promptReceipt.schema}
          receiptId={promptReceipt.receipt_id}
          fields={[
            { label: "Capture", value: promptReceipt.capture_state },
            { label: "Redaction", value: promptReceipt.redaction_state },
            { label: "Source passages", value: `${promptReceipt.source_passage_count}` },
          ]}
        />
      )}

      {/* Evidence-level receipts */}
      <Section title="Evidence Receipts">
        <KVRow label="Receipt ID" value={evidence.receipt_id.slice(0, 16)} mono />
        {evidence.semantic_memory_receipt_id && (
          <KVRow label="SM receipt" value={evidence.semantic_memory_receipt_id.slice(0, 16)} mono />
        )}
        {evidence.turbo_quant_generation_id && (
          <KVRow label="TQ generation" value={evidence.turbo_quant_generation_id.slice(0, 16)} mono />
        )}
      </Section>

      {/* Digests */}
      <Section title="Digests">
        <KVRow label="Context" value={evidence.context_digest.slice(0, 20)} mono />
        <KVRow label="Source context" value={evidence.source_context_digest.slice(0, 20)} mono />
        {evidence.prompt_digest && (
          <KVRow label="Prompt" value={evidence.prompt_digest.slice(0, 20)} mono />
        )}
        {evidence.vector_artifact_manifest_digest && (
          <KVRow label="Vector manifest" value={evidence.vector_artifact_manifest_digest.slice(0, 20)} mono />
        )}
      </Section>

      {/* Copy all receipt IDs */}
      <CopyAllReceipts
        generationId={generationReceipt?.receipt_id}
        decodingId={decodingReceipt?.receipt_id}
        promptId={promptReceipt?.receipt_id}
        evidenceId={evidence.receipt_id}
      />
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

function ReceiptCard({
  title,
  schema,
  receiptId,
  fields,
}: {
  title: string;
  schema: string;
  receiptId: string;
  fields: { label: string; value: string; mono?: boolean; warn?: boolean }[];
}) {
  const [copied, setCopied] = useState(false);
  const handleCopy = async () => {
    await navigator.clipboard.writeText(receiptId);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="rounded border border-border bg-bg-secondary/50 p-2 space-y-1">
      <div className="flex items-center justify-between">
        <span className="font-medium text-text">{title}</span>
        <button
          onClick={handleCopy}
          className="flex items-center gap-0.5 rounded px-1 py-0.5 text-[10px] text-text-muted hover:bg-bg-tertiary hover:text-text"
          title="Copy receipt ID"
        >
          {copied ? <CheckCircle className="w-2.5 h-2.5 text-success" /> : <Copy className="w-2.5 h-2.5" />}
          {copied ? "Copied" : "Copy ID"}
        </button>
      </div>
      <KVRow label="Schema" value={schema} />
      <KVRow label="Receipt ID" value={receiptId.slice(0, 16)} mono />
      {fields.map((f) => (
        <KVRow key={f.label} label={f.label} value={f.value} mono={f.mono} warn={f.warn} />
      ))}
    </div>
  );
}

function CopyAllReceipts({
  generationId,
  decodingId,
  promptId,
  evidenceId,
}: {
  generationId?: string;
  decodingId?: string;
  promptId?: string;
  evidenceId: string;
}) {
  const [copied, setCopied] = useState(false);
  const handleCopy = async () => {
    const lines = [
      `Evidence receipt: ${evidenceId}`,
      generationId ? `Generation receipt: ${generationId}` : "",
      decodingId ? `Decoding receipt: ${decodingId}` : "",
      promptId ? `Prompt receipt: ${promptId}` : "",
    ].filter(Boolean).join("\n");
    await navigator.clipboard.writeText(lines);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <button
      onClick={handleCopy}
      className="w-full rounded border border-border bg-bg-secondary px-2 py-1.5 text-[11px] text-text-muted hover:bg-bg-tertiary hover:text-text flex items-center justify-center gap-1"
    >
      {copied ? <CheckCircle className="w-3 h-3 text-success" /> : <Copy className="w-3 h-3" />}
      {copied ? "All receipt IDs copied" : "Copy all receipt IDs"}
    </button>
  );
}