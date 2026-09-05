import { useEffect, useMemo, useRef, useState } from "react";
import type React from "react";
import { useSettingsStore } from "../../../stores/settingsStore";
import { useToastStore } from "../../../stores/toastStore";
import { useNotebookStore } from "../../../stores/notebookStore";
import * as api from "../../../lib/tauri";
import {
  canUseSemanticMemoryPreview,
  EXPERIMENTAL_FEATURES_ENABLED,
  featureById,
  featureSections,
  FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED,
  FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED,
} from "../../../lib/features";
import type {
  ChatAttemptTraceV1,
  DbDoctorReceipt,
  EmbeddingDiagnosticsReceipt,
  FeatureFlagStatus,
  MemoryBackendStatus,
  SemanticMemoryProfileStatus,
  SemanticMemoryLinkStatus,
} from "../../../lib/types";
import {
  AlertCircle,
  BookOpen,
  Check,
  Copy,
  Cpu,
  Database,
  Eye,
  EyeOff,
  Image,
  Key,
  Loader2,
  RefreshCw,
  Server,
  Settings2,
  ShieldCheck,
  TestTube2,
  Wrench,
  X,
} from "lucide-react";

interface SettingsDialogProps {
  open: boolean;
  onClose: () => void;
}

type TestStatus = "idle" | "testing" | "success" | "error";

function isVisionCapableModel(model: {
  id: string;
  display_name: string;
  capabilities?: string;
}) {
  if (model.capabilities) {
    const caps = model.capabilities
      .split(",")
      .map((cap) => cap.trim().toLowerCase());
    if (caps.includes("vision") || caps.includes("image") || caps.includes("multimodal")) {
      return true;
    }
  }

  const fingerprint = `${model.id} ${model.display_name}`.toLowerCase();
  return [
    "llava",
    "bakllava",
    "moondream",
    "minicpm-v",
    "qwen-vl",
    "qwen2-vl",
    "qwen2.5-vl",
    "gemma3",
    "gemma4",
    "vision",
    "vl",
  ].some((needle) => fingerprint.includes(needle));
}

function providerUrlClass(rawUrl: string | undefined): string {
  if (!rawUrl?.trim()) return "default";
  try {
    const parsed = new URL(rawUrl);
    const host = parsed.hostname.toLowerCase();
    if (host === "localhost" || host === "127.0.0.1" || host === "::1") return "loopback";
    if (
      host.startsWith("10.") ||
      host.startsWith("192.168.") ||
      /^172\.(1[6-9]|2\d|3[0-1])\./.test(host)
    ) {
      return "lan";
    }
    return parsed.protocol === "https:" ? "cloud_https" : "remote";
  } catch {
    return "invalid";
  }
}

/**
 * Debounce wrapper around `updateSetting` for text/number inputs that fire
 * onChange per keystroke. Without this, typing "http://localhost:11434"
 * would issue 21 IPC calls.
 *
 * Returns a tuple: [localValue, onChange, syncLocal]. The local value mirrors
 * `settings[key]` so the input stays controlled even while the debounce
 * timer is pending. `syncLocal` updates the local value WITHOUT scheduling a
 * write — use it when syncing from the canonical settings store, otherwise
 * every settings load would echo a redundant (or default-clobbering) write
 * back to the backend.
 */
function useDebouncedSetting(
  key: string,
  updateSetting: (key: string, value: string) => Promise<void>,
  delayMs: number = 400
): [string, (value: string) => void, (value: string) => void] {
  const [localValue, setLocalValue] = useState("");
  const lastEmittedRef = useRef<string | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);
  const onChange = (value: string) => {
    setLocalValue(value);
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      lastEmittedRef.current = value;
      timerRef.current = null;
      // Fire-and-forget: the store now handles rollback on failure.
      updateSetting(key, value).catch(() => {
        // Already toasted inside the store; do nothing extra.
      });
    }, delayMs);
  };
  const syncLocal = (value: string) => {
    // The store optimistically echoes our own debounced write back through
    // settings[key]; ignore it so keystrokes typed in that window survive.
    if (value === lastEmittedRef.current) return;
    setLocalValue(value);
    // A genuinely external canonical value arrived — drop any pending write
    // so a stale keystroke cannot overwrite it.
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  };
  return [localValue, onChange, syncLocal];
}

function SettingsSection({
  title,
  icon,
  children,
}: {
  title: string;
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3">
      <div className="flex items-center gap-2">
        <span className="text-text-secondary">{icon}</span>
        <h3 className="text-xs font-semibold text-text uppercase tracking-wide">{title}</h3>
      </div>
      {children}
    </section>
  );
}

function ProviderSection({
  id,
  label,
  urlKey,
  urlDefault,
  apiKeyKey,
  settings,
  onSave,
}: {
  id: string;
  label: string;
  urlKey: string;
  urlDefault: string;
  apiKeyKey?: string;
  settings: Record<string, string>;
  onSave: (updates: Record<string, string>) => Promise<void>;
}) {
  const { testProvider } = useSettingsStore();
  const [url, setUrl] = useState(urlDefault);
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [testStatus, setTestStatus] = useState<TestStatus>("idle");
  const [dirty, setDirty] = useState(false);
  const configuredKey = apiKeyKey ? `${apiKeyKey}_configured` : null;
  const hasStoredSecret = configuredKey ? settings[configuredKey] === "1" : false;

  useEffect(() => {
    setUrl(settings[urlKey] || urlDefault);
    if (apiKeyKey) setApiKey("");
    setDirty(false);
  }, [settings, urlKey, urlDefault, apiKeyKey]);

  const handleSave = async () => {
    const updates: Record<string, string> = { [urlKey]: url };
    if (apiKeyKey && (apiKey.trim() || !hasStoredSecret)) {
      updates[apiKeyKey] = apiKey;
    }
    await onSave(updates);
    if (apiKeyKey) setApiKey("");
    setDirty(false);
  };

  const handleClearKey = async () => {
    if (!apiKeyKey) return;
    await onSave({ [apiKeyKey]: "" });
    setApiKey("");
    setDirty(false);
  };

  const handleTest = async () => {
    if (dirty) await handleSave();
    setTestStatus("testing");
    const ok = await testProvider(id);
    setTestStatus(ok ? "success" : "error");
    useToastStore.getState().addToast({
      type: ok ? "success" : "error",
      title: ok ? `${label} Connected` : `${label} Failed`,
      message: ok ? "Provider is reachable" : "Could not connect to provider",
      duration: ok ? 3000 : 5000,
    });
    setTimeout(() => setTestStatus("idle"), 3000);
  };

  return (
    <div className="space-y-2 rounded border border-border bg-bg-tertiary/40 p-3">
      <div className="flex items-center justify-between gap-2">
        <p className="text-xs font-medium text-text">{label}</p>
        <button
          onClick={handleTest}
          disabled={testStatus === "testing"}
          className="flex items-center gap-1.5 rounded bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text disabled:opacity-50"
        >
          {testStatus === "testing" && <Loader2 className="h-3 w-3 animate-spin" />}
          {testStatus === "success" && <Check className="h-3 w-3 text-success" />}
          {testStatus === "error" && <AlertCircle className="h-3 w-3 text-error" />}
          {testStatus === "idle" && <Server className="h-3 w-3" />}
          {testStatus === "testing"
            ? "Testing..."
            : testStatus === "success"
              ? "Connected"
              : testStatus === "error"
                ? "Failed"
                : "Test"}
        </button>
      </div>
      <label className="block text-xs text-text-secondary">Server URL</label>
      <div className="flex gap-2">
        <input
          type="text"
          value={url}
          onChange={(e) => {
            setUrl(e.target.value);
            setDirty(true);
          }}
          className="min-w-0 flex-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text placeholder:text-text-muted focus:border-accent focus:outline-none"
          placeholder={urlDefault}
          aria-label={`${label} server URL`}
        />
        <button
          onClick={handleSave}
          disabled={!dirty}
          className="rounded bg-accent px-3 py-1.5 text-xs text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-40"
        >
          Save
        </button>
      </div>
      {apiKeyKey && (
        <>
          <label className="block text-xs text-text-secondary">
            <Key className="mr-1 inline h-3 w-3" />
            API Key
          </label>
          <div className="flex gap-2">
            <div className="relative min-w-0 flex-1">
              <input
                type={showKey ? "text" : "password"}
                value={apiKey}
                onChange={(e) => {
                  setApiKey(e.target.value);
                  setDirty(true);
                }}
                className="w-full rounded border border-border bg-bg-tertiary px-2 py-1.5 pr-8 text-sm text-text placeholder:text-text-muted focus:border-accent focus:outline-none"
                placeholder={hasStoredSecret ? "Stored securely on this device" : "sk-..."}
              />
              <button
                onClick={() => setShowKey(!showKey)}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-text-muted hover:text-text"
              >
                {showKey ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
              </button>
            </div>
            {hasStoredSecret && (
              <button
                onClick={handleClearKey}
                className="rounded bg-bg-tertiary px-3 py-1.5 text-xs text-text-secondary hover:bg-border hover:text-text"
              >
                Clear
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

function FeatureToggleRow({
  flag,
  onToggle,
  mutable,
}: {
  flag: FeatureFlagStatus;
  onToggle: (id: string, enabled: boolean) => Promise<void>;
  mutable: boolean;
}) {
  const disabled = !mutable || !flag.available;
  return (
    <div className="flex items-center justify-between gap-3 rounded border border-border bg-bg-tertiary/40 px-3 py-2">
      <div className="min-w-0">
        <p className="truncate text-xs font-medium text-text">{flag.label}</p>
        <p className="truncate text-[11px] text-text-muted">
          {flag.unavailable_reason || (flag.active ? "Active" : flag.enabled ? "Enabled" : "Off")}
        </p>
      </div>
      <input
        type="checkbox"
        checked={flag.enabled}
        disabled={disabled}
        onChange={(e) => onToggle(flag.id, e.target.checked)}
        className="h-4 w-4 shrink-0 accent-accent disabled:opacity-40"
      />
    </div>
  );
}

export function SettingsDialog({ open, onClose }: SettingsDialogProps) {
  const {
    models,
    settings,
    featureFlags,
    activeModel,
    loading,
    externalTools,
    providers,
    loadSettings,
    loadProviders,
    loadModels,
    loadFeatureFlags,
    refreshModels,
    updateSetting,
    updateProvider,
    updateFeatureFlag,
    selectModel,
    loadExternalTools,
  } = useSettingsStore();
  const activeNotebookId = useNotebookStore((s) => s.activeNotebookId);
  const [memoryStatus, setMemoryStatus] = useState<MemoryBackendStatus | null>(null);
  const [linkStatus, setLinkStatus] = useState<SemanticMemoryLinkStatus | null>(null);
  const [profileStatus, setProfileStatus] = useState<SemanticMemoryProfileStatus | null>(null);
  const [reindexingSemanticMemory, setReindexingSemanticMemory] = useState(false);
  const [rebuildingTurboQuant, setRebuildingTurboQuant] = useState(false);
  const [runningRetrievalProbe, setRunningRetrievalProbe] = useState(false);
  const [runningEmbeddingDiagnostics, setRunningEmbeddingDiagnostics] = useState(false);
  const [embeddingDiagnostics, setEmbeddingDiagnostics] =
    useState<EmbeddingDiagnosticsReceipt | null>(null);
  const [runningDbDoctor, setRunningDbDoctor] = useState<"check" | "repair" | null>(null);
  const [dbDoctorReceipt, setDbDoctorReceipt] = useState<DbDoctorReceipt | null>(null);
  const [runningChatSmoke, setRunningChatSmoke] = useState(false);
  const [lastTrace, setLastTrace] = useState<ChatAttemptTraceV1 | null>(null);
  const allowCustomCloudEndpoints =
    settings["allow_custom_cloud_endpoints"] === "true" ||
    settings["allow_custom_cloud_endpoints"] === "1";

  // Debounce the 5 text/number inputs that previously fired updateSetting on
  // every keystroke (H-4 from the hostile audit). Typing "http://localhost"
  // used to issue 17+ IPC calls; now it issues 1.
  const [embeddingUrl, setEmbeddingUrl, syncEmbeddingUrl] = useDebouncedSetting("semantic_memory_embedding_url", updateSetting);
  const [embeddingModel, setEmbeddingModel, syncEmbeddingModel] = useDebouncedSetting("semantic_memory_embedding_model", updateSetting);
  const [embeddingTimeout, setEmbeddingTimeout, syncEmbeddingTimeout] = useDebouncedSetting("semantic_memory_embedding_timeout_secs", updateSetting);
  const [searchTimeout, setSearchTimeout, syncSearchTimeout] = useDebouncedSetting("semantic_memory_search_timeout_ms", updateSetting);
  const [chunkTargetTokens, setChunkTargetTokens, syncChunkTargetTokens] = useDebouncedSetting("chunk_target_tokens", updateSetting);
  // Sync local debounced state from settings whenever the canonical value
  // changes (e.g. on dialog open or external update). Uses the sync-only
  // setter so syncing never schedules a write back to the backend.
  useEffect(() => {
    syncEmbeddingUrl(settings["semantic_memory_embedding_url"] || "http://localhost:11434");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings["semantic_memory_embedding_url"]]);
  useEffect(() => {
    syncEmbeddingModel(settings["semantic_memory_embedding_model"] || "bge-m3");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings["semantic_memory_embedding_model"]]);
  useEffect(() => {
    syncEmbeddingTimeout(settings["semantic_memory_embedding_timeout_secs"] || "10");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings["semantic_memory_embedding_timeout_secs"]]);
  useEffect(() => {
    syncSearchTimeout(settings["semantic_memory_search_timeout_ms"] || "8000");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings["semantic_memory_search_timeout_ms"]]);
  useEffect(() => {
    syncChunkTargetTokens(settings["chunk_target_tokens"] || "1100");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings["chunk_target_tokens"]]);

  useEffect(() => {
    if (open) {
      loadSettings();
      loadProviders();
      loadModels();
      loadFeatureFlags();
      loadExternalTools();
      api.memoryBackendStatus(activeNotebookId).then(setMemoryStatus).catch((err) => { console.warn("memoryBackendStatus failed:", err); setMemoryStatus(null); });
      if (activeNotebookId) {
        api.semanticMemoryLinkStatus(activeNotebookId).then(setLinkStatus).catch((err) => { console.warn("linkStatus failed:", err); setLinkStatus(null); });
        api.getSemanticMemoryProfileStatus(activeNotebookId, { kind: "all" }).then(setProfileStatus).catch((err) => { console.warn("profileStatus failed:", err); setProfileStatus(null); });
      } else {
        setLinkStatus(null);
        setProfileStatus(null);
      }
    }
  }, [
    activeNotebookId,
    open,
    loadSettings,
    loadProviders,
    loadModels,
    loadFeatureFlags,
    loadExternalTools,
  ]);

  const sections = useMemo(() => featureSections(featureFlags), [featureFlags]);
  const experimentalMaster = featureById(featureFlags, EXPERIMENTAL_FEATURES_ENABLED);
  const semanticPreview = featureById(featureFlags, FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED);
  const turboQuant = featureById(featureFlags, FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED);
  const semanticPreviewSelectable = canUseSemanticMemoryPreview(featureFlags);

  const providerGroups: Record<string, typeof models> = {};
  for (const m of models) {
    const group = m.provider_id || "unknown";
    if (!providerGroups[group]) providerGroups[group] = [];
    providerGroups[group].push(m);
  }

  const providerLabels: Record<string, string> = {
    ollama: "Ollama",
    openai: "OpenAI",
    anthropic: "Anthropic",
    llamacpp: "llama.cpp",
  };
  // Summary and vision models may be served by any enabled provider — not
  // just Ollama. Filter on `provider.enabled` rather than hardcoding a
  // provider id so OpenAI, Anthropic, and llama.cpp are selectable too.
  const enabledProviderIds = new Set(
    providers.filter((p) => p.enabled).map((p) => p.id)
  );
  const summaryModels = models.filter(
    (model) =>
      model.provider_id !== undefined &&
      enabledProviderIds.has(model.provider_id)
  );
  const visionModels = summaryModels.filter(isVisionCapableModel);
  const configuredProviderId = settings["default_provider"] ?? "ollama";
  const activeModelRow = models.find(
    (model) => model.id === activeModel && model.provider_id === configuredProviderId
  );
  const activeProviderId = activeModelRow?.provider_id ?? configuredProviderId;

  // Discovery is a cache, not authority to erase configured model intent.

  const handleProviderSave = async (updates: Record<string, string>) => {
    if (updates["ollama_url"]) {
      await updateProvider("ollama", true, updates["ollama_url"]);
    }
    if ("openai_api_key" in updates || "openai_base_url" in updates) {
      await updateProvider("openai", true, updates["openai_base_url"], updates["openai_api_key"]);
    }
    if ("anthropic_api_key" in updates || "anthropic_base_url" in updates) {
      await updateProvider(
        "anthropic",
        true,
        updates["anthropic_base_url"],
        updates["anthropic_api_key"]
      );
    }
    if (updates["llamacpp_url"]) {
      await updateProvider("llamacpp", true, updates["llamacpp_url"]);
    }
    await loadSettings();
  };

  const handleSelectModel = async (providerId: string, modelId: string) => {
    await selectModel(providerId, modelId);
  };

  const handleSelectMemoryBackend = async (backendId: string) => {
    await handleMemoryProfile(
      backendId === "semantic-memory-preview" ? "semantic-memory-safe" : "gloss-local"
    );
  };

  const handleMemoryProfile = async (profile: string) => {
    try {
      const receipt = await api.setMemoryBackendProfile(profile, activeNotebookId);
      await loadSettings();
      await loadFeatureFlags();
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: receipt.blocked ? "error" : "success",
        title: receipt.blocked ? "Memory profile blocked" : "Memory profile applied",
        message: receipt.blocked
          ? `${receipt.blocking_reasons.join(", ")}`
          : `${receipt.profile}: ${receipt.backend_used}`,
        duration: 4000,
      });
    } catch (error) {
      await loadSettings();
      await loadFeatureFlags();
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: "error",
        title: "Memory profile not applied",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    }
  };

  const refreshMemoryEvidence = async () => {
    await api.memoryBackendStatus(activeNotebookId).then(setMemoryStatus).catch((err) => { console.warn("memoryBackendStatus failed:", err); setMemoryStatus(null); });
    if (activeNotebookId) {
      await api.semanticMemoryLinkStatus(activeNotebookId).then(setLinkStatus).catch((err) => { console.warn("linkStatus failed:", err); setLinkStatus(null); });
      await api.getSemanticMemoryProfileStatus(activeNotebookId, { kind: "all" }).then(setProfileStatus).catch((err) => { console.warn("profileStatus failed:", err); setProfileStatus(null); });
    } else {
      setLinkStatus(null);
      setProfileStatus(null);
    }
  };

  const handleReindexSemanticMemoryNotebook = async () => {
    if (!activeNotebookId) return;
    setReindexingSemanticMemory(true);
    try {
      const receipt = await api.semanticMemoryBackfillNotebook(activeNotebookId);
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: "success",
        title: "Projection backfill complete",
        message: `${receipt.projected_sources} projected, ${receipt.skipped_no_chunks} skipped, ${receipt.failed_sources} failed.`,
        duration: 5000,
      });
    } catch (error) {
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: "error",
        title: "semantic-memory reindex failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    } finally {
      setReindexingSemanticMemory(false);
    }
  };

  const handleRebuildTurboQuantArtifacts = async () => {
    if (!activeNotebookId) return;
    setRebuildingTurboQuant(true);
    try {
      await api.semanticMemoryRebuildVectorArtifacts(activeNotebookId);
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: "success",
        title: "TurboQuant artifacts rebuilt",
        message: "Fresh artifact receipt recorded.",
        duration: 5000,
      });
    } catch (error) {
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: "error",
        title: "TurboQuant rebuild failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    } finally {
      setRebuildingTurboQuant(false);
    }
  };

  const handleRunRetrievalProbe = async () => {
    if (!activeNotebookId) return;
    setRunningRetrievalProbe(true);
    try {
      const probe = await api.runRetrievalProbe(activeNotebookId, "GLOSS_SM_TQ_SENTINEL_20260523", { kind: "all" }, 8);
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: probe.fallback_used ? "error" : "success",
        title: "Retrieval probe complete",
        message: `${probe.backend_used}: ${probe.bm25_candidates} BM25, ${probe.vector_candidates} semantic, exact rerank ${probe.exact_rerank_count}.`,
        duration: 7000,
      });
    } catch (error) {
      await refreshMemoryEvidence();
      useToastStore.getState().addToast({
        type: "error",
        title: "Retrieval probe failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    } finally {
      setRunningRetrievalProbe(false);
    }
  };

  const handleRunEmbeddingDiagnostics = async () => {
    setRunningEmbeddingDiagnostics(true);
    try {
      const receipt = await api.runEmbeddingDiagnostics();
      setEmbeddingDiagnostics(receipt);
      const native = receipt.native_fastembed;
      const healthy = native.init_ok && native.embed_one_ok;
      useToastStore.getState().addToast({
        type: healthy ? "success" : "error",
        title: "Embedding diagnostics complete",
        message: healthy
          ? `${receipt.semantic_memory_provider.provider} ready · ${native.dims ?? "?"} dims`
          : native.error || "Embedding backend not ready",
        duration: 6000,
      });
    } catch (error) {
      useToastStore.getState().addToast({
        type: "error",
        title: "Embedding diagnostics failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    } finally {
      setRunningEmbeddingDiagnostics(false);
    }
  };

  const handleRunDatabaseDoctor = async (repair: boolean) => {
    setRunningDbDoctor(repair ? "repair" : "check");
    try {
      const receipt = await api.runDatabaseDoctor(repair);
      setDbDoctorReceipt(receipt);
      useToastStore.getState().addToast({
        type: receipt.findings.some((finding) => finding.severity === "error") ? "error" : "success",
        title: repair ? "Database repair complete" : "Database check complete",
        message: `${receipt.findings.length} findings, ${receipt.repaired_source_count_mismatches + receipt.repaired_orphan_rows + receipt.quarantined_failed_import_sources + receipt.repaired_stale_queue_jobs} repairs.`,
        duration: 7000,
      });
    } catch (error) {
      useToastStore.getState().addToast({
        type: "error",
        title: repair ? "Database repair failed" : "Database check failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    } finally {
      setRunningDbDoctor(null);
    }
  };
  const handleCheckDatabaseDoctor = () => handleRunDatabaseDoctor(false);
  const handleRepairDatabaseDoctor = () => handleRunDatabaseDoctor(true);

  const handleFeatureToggle = async (id: string, enabled: boolean) => {
    try {
      await updateFeatureFlag(id, enabled);
    } catch (error) {
      useToastStore.getState().addToast({
        type: "error",
        title: "Feature flag not changed",
        message: error instanceof Error ? error.message : String(error),
        duration: 6000,
      });
      await loadFeatureFlags();
    }
  };

  const handleRunChatProviderSmoke = async () => {
    setRunningChatSmoke(true);
    try {
      const trace = await api.debugChatProviderSmoke(activeProviderId, activeModel);
      setLastTrace(trace);
      await navigator.clipboard.writeText(JSON.stringify(trace, null, 2));
      const summary = `${trace.provider}:${trace.model} first=${trace.first_token_seen} done=${trace.done_seen} persisted=${trace.assistant_persisted} error=${trace.error ?? "none"}`;
      useToastStore.getState().addToast({
        type: trace.error ? "error" : trace.first_token_seen && trace.done_seen ? "success" : "warning",
        title: "Chat smoke trace copied",
        message: summary,
        duration: 7000,
      });
    } catch (error) {
      useToastStore.getState().addToast({
        type: "error",
        title: "Chat smoke failed",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    } finally {
      setRunningChatSmoke(false);
    }
  };

  const handleCopyLastChatTrace = async () => {
    try {
      const trace = await api.getLastChatAttemptTrace();
      if (!trace) {
        setLastTrace(null);
        useToastStore.getState().addToast({
          type: "warning",
          title: "No chat trace found",
          message: "Run a chat or provider smoke first.",
          duration: 5000,
        });
        return;
      }
      setLastTrace(trace);
      const summary = `${trace.provider}:${trace.model} first=${trace.first_token_seen} done=${trace.done_seen} persisted=${trace.assistant_persisted} error=${trace.error ?? "none"}`;
      await navigator.clipboard.writeText(JSON.stringify(trace, null, 2));
      useToastStore.getState().addToast({
        type: "success",
        title: "Chat trace copied",
        message: summary,
        duration: 5000,
      });
    } catch (error) {
      useToastStore.getState().addToast({
        type: "error",
        title: "Could not copy chat trace",
        message: error instanceof Error ? error.message : String(error),
        duration: 7000,
      });
    }
  };

  const handleCopyProviderConfigSummary = async () => {
    // Redact base URLs — only include classification, not full URLs,
    // to avoid leaking internal network topology via clipboard.
    const summary = {
      schema: "ProviderConfigSummaryV1",
      active_provider: activeProviderId,
      active_model: activeModel,
      selected_model_available: activeModelRow?.available ?? null,
      selected_model_stale: activeModelRow?.stale ?? null,
      providers: [
        { id: "ollama", base_url_class: providerUrlClass(settings["ollama_url"] || "http://localhost:11434") },
        { id: "openai", base_url_class: providerUrlClass(settings["openai_base_url"] || "https://api.openai.com/v1"), api_key_configured: settings["openai_api_key_configured"] === "1" },
        { id: "anthropic", base_url_class: providerUrlClass(settings["anthropic_base_url"] || "https://api.anthropic.com/v1"), api_key_configured: settings["anthropic_api_key_configured"] === "1" },
        { id: "llamacpp", base_url_class: providerUrlClass(settings["llamacpp_url"] || "http://localhost:8080/v1") },
      ],
    };
    try {
      await navigator.clipboard.writeText(JSON.stringify(summary, null, 2));
      useToastStore.getState().addToast({
        type: "success",
        title: "Provider config copied",
        message: `${summary.active_provider}:${summary.active_model}`,
        duration: 4000,
      });
    } catch (err) {
      console.warn("Failed to copy provider config summary:", err);
    }
  };

  const handleBackdropClick = (e: React.MouseEvent) => {
    if (e.target === e.currentTarget) onClose();
  };

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      onClick={handleBackdropClick}
    >
      <div className="flex max-h-[84vh] w-[680px] max-w-[calc(100vw-24px)] flex-col rounded-lg border border-border bg-bg-secondary shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <h2 className="text-sm font-semibold text-text">Settings</h2>
          <button
            onClick={onClose}
            className="rounded p-1 text-text-secondary hover:bg-bg-tertiary hover:text-text"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="flex-1 space-y-7 overflow-y-auto p-4">
          <SettingsSection title="Providers" icon={<Server className="h-4 w-4" />}>
            <div className="grid gap-3 sm:grid-cols-2">
              <ProviderSection
                id="ollama"
                label="Ollama"
                urlKey="ollama_url"
                urlDefault="http://localhost:11434"
                settings={settings}
                onSave={handleProviderSave}
              />
              <ProviderSection
                id="openai"
                label="OpenAI"
                urlKey="openai_base_url"
                urlDefault="https://api.openai.com/v1"
                apiKeyKey="openai_api_key"
                settings={settings}
                onSave={handleProviderSave}
              />
              <ProviderSection
                id="anthropic"
                label="Anthropic"
                urlKey="anthropic_base_url"
                urlDefault="https://api.anthropic.com/v1"
                apiKeyKey="anthropic_api_key"
                settings={settings}
                onSave={handleProviderSave}
              />
              <ProviderSection
                id="llamacpp"
                label="llama.cpp"
                urlKey="llamacpp_url"
                urlDefault="http://localhost:8080/v1"
                settings={settings}
                onSave={handleProviderSave}
              />
            </div>
            <label className="mt-3 flex items-center gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={settings["allow_lan_local_providers"] === "true" || settings["allow_lan_local_providers"] === "1"}
                onChange={(e) => {
                  updateSetting("allow_lan_local_providers", e.target.checked ? "true" : "false");
                }}
                className="rounded border-border"
              />
              Allow LAN local providers (RFC1918)
              <span className="text-text-tertiary">— permits Ollama/llama.cpp on private-network IPs (e.g. 192.168.x.x, 10.x.x.x). Default: loopback only.</span>
            </label>
            <label className="mt-2 flex items-start gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={allowCustomCloudEndpoints}
                onChange={(e) => {
                  updateSetting(
                    "allow_custom_cloud_endpoints",
                    e.target.checked ? "true" : "false"
                  );
                }}
                className="mt-0.5 rounded border-border"
              />
              <div className="leading-tight">
                Allow custom OpenAI/Anthropic cloud endpoints
                <p className="mt-0.5 text-text-tertiary">
                  Warning: this permits non-default HTTPS cloud endpoints (for example
                  OpenAI-compatible/OpenRouter/Azure endpoints). Credentials, query strings,
                  and fragments remain blocked. Default: off.
                </p>
              </div>
            </label>
          </SettingsSection>

          <SettingsSection title="Models" icon={<Cpu className="h-4 w-4" />}>
            <div className="flex items-center justify-between gap-2">
              <p className="text-xs text-text-secondary">Default chat model</p>
              <button
                onClick={refreshModels}
                disabled={loading}
                className="flex items-center gap-1 rounded bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text disabled:opacity-50"
              >
                <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
                Refresh
              </button>
            </div>
            {models.length === 0 ? (
              <p className="rounded border border-border bg-bg-tertiary/40 px-3 py-3 text-center text-xs text-text-muted">
                No models found.
              </p>
            ) : (
              <div className="max-h-64 space-y-1 overflow-y-auto">
                {Object.entries(providerGroups).map(([providerId, groupModels]) => (
                  <div key={providerId}>
                    <div className="px-3 py-1 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                      {providerLabels[providerId] || providerId}
                    </div>
                    {groupModels.map((model) => (
                      <button
                        key={`${model.provider_id}:${model.id}`}
                        onClick={() => handleSelectModel(model.provider_id, model.id)}
                        className={`flex w-full items-center gap-3 rounded border px-3 py-2 text-left ${
                          activeModel === model.id && activeProviderId === model.provider_id
                            ? "border-accent/30 bg-accent/10"
                            : "border-transparent hover:bg-bg-tertiary"
                        }`}
                      >
                        <div
                          className={`h-3 w-3 shrink-0 rounded-full border-2 ${
                            activeModel === model.id && activeProviderId === model.provider_id ? "border-accent bg-accent" : "border-text-muted"
                          }`}
                        />
                        <p className="min-w-0 flex-1 truncate text-xs text-text">{model.display_name}</p>
                        {model.parameter_size && (
                          <span className="shrink-0 rounded bg-bg-tertiary px-1.5 py-0.5 text-[10px] text-text-muted">
                            {model.parameter_size}
                          </span>
                        )}
                      </button>
                    ))}
                  </div>
                ))}
              </div>
            )}
          </SettingsSection>

          <SettingsSection title="Chat" icon={<Settings2 className="h-4 w-4" />}>
            <div className="grid gap-2 sm:grid-cols-2">
              <HealthCard
                title="Provider"
                status={activeModel}
                detail={`Chat provider: ${activeProviderId}`}
                tone="neutral"
              />
              <HealthCard
                title="Diagnostics"
                status={featureById(featureFlags, "feature_chat_diagnostics_enabled")?.active ? "active" : "off"}
                detail={
                  lastTrace
                    ? `${lastTrace.provider}:${lastTrace.model} first=${lastTrace.first_token_seen} done=${lastTrace.done_seen} persisted=${lastTrace.assistant_persisted}`
                    : "ChatAttemptTraceV1 evidence is copyable."
                }
                tone={
                  lastTrace
                    ? lastTrace.error
                      ? "error"
                      : lastTrace.first_token_seen && lastTrace.done_seen
                        ? "success"
                        : "warning"
                    : featureById(featureFlags, "feature_chat_diagnostics_enabled")?.active
                      ? "success"
                      : "warning"
                }
              />
            </div>
            {lastTrace && (
              <div className="mt-2 rounded border border-border bg-bg-tertiary p-3 text-xs font-mono">
                <div className="mb-1 font-semibold text-text-secondary">Last ChatAttemptTraceV1</div>
                <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
                  <span className="text-text-secondary">provider:</span>
                  <span>{lastTrace.provider}</span>
                  <span className="text-text-secondary">base_url:</span>
                  <span>{lastTrace.provider_base_url ?? "(default)"}</span>
                  <span className="text-text-secondary">model:</span>
                  <span>{lastTrace.model}</span>
                  <span className="text-text-secondary">first_token_seen:</span>
                  <span className={lastTrace.first_token_seen ? "text-green-500" : "text-red-500"}>
                    {String(lastTrace.first_token_seen)}
                  </span>
                  <span className="text-text-secondary">done_seen:</span>
                  <span className={lastTrace.done_seen ? "text-green-500" : "text-red-500"}>
                    {String(lastTrace.done_seen)}
                  </span>
                  <span className="text-text-secondary">assistant_persisted:</span>
                  <span className={lastTrace.assistant_persisted ? "text-green-500" : "text-red-500"}>
                    {String(lastTrace.assistant_persisted)}
                  </span>
                  {lastTrace.error && (
                    <>
                      <span className="text-text-secondary">error:</span>
                      <span className="text-red-500">{lastTrace.error}</span>
                    </>
                  )}
                  {lastTrace.events && lastTrace.events.length > 0 && (
                    <>
                      <span className="text-text-secondary">phase:</span>
                      <span>{lastTrace.events[lastTrace.events.length - 1].phase}</span>
                    </>
                  )}
                </div>
              </div>
            )}
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={handleRunChatProviderSmoke}
                disabled={runningChatSmoke}
                className="flex items-center gap-1.5 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text disabled:opacity-50"
              >
                {runningChatSmoke ? <Loader2 className="h-3 w-3 animate-spin" /> : <TestTube2 className="h-3 w-3" />}
                Run Chat Provider Smoke
              </button>
              <button
                type="button"
                onClick={handleCopyLastChatTrace}
                className="flex items-center gap-1.5 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
              >
                <Copy className="h-3 w-3" />
                Copy Last ChatAttemptTraceV1
              </button>
              <button
                type="button"
                onClick={handleCopyProviderConfigSummary}
                className="flex items-center gap-1.5 rounded border border-border bg-bg-tertiary px-2 py-1 text-xs text-text-secondary hover:bg-border hover:text-text"
              >
                <ShieldCheck className="h-3 w-3" />
                Copy Provider Config Summary
              </button>
            </div>
          </SettingsSection>

          <SettingsSection title="Memory & Retrieval" icon={<Database className="h-4 w-4" />}>
            <div className="grid gap-2 sm:grid-cols-2">
              <HealthCard
                title="Memory"
                status={memoryStatus?.backend_used ?? "gloss-local"}
                detail={
                  memoryStatus?.backend_used !== memoryStatus?.active_backend
                    ? `Requested ${memoryStatus?.active_backend}; fallback disclosed`
                    : `Default ${memoryStatus?.default_backend ?? "gloss-local"}`
                }
                tone={memoryStatus?.degraded ? "warning" : "success"}
              />
              <HealthCard
                title="Embedding / Index"
                status={memoryStatus?.index_sync_status ?? "unknown"}
                detail={
                  linkStatus
                    ? `${linkStatus.synced_links}/${linkStatus.total_links} semantic links synced`
                    : "Gloss local index remains available"
                }
                tone={
                  memoryStatus?.index_sync_status === "failed"
                    ? "error"
                    : memoryStatus?.index_sync_status === "degraded"
                      ? "warning"
                      : "neutral"
                }
              />
            </div>
            {linkStatus && (
              <div className="rounded border border-border bg-bg-tertiary/40 px-3 py-2 text-xs text-text-secondary">
                <div className="grid gap-1 sm:grid-cols-3">
                  <span>Stale {linkStatus.stale_links}</span>
                  <span>Failed {linkStatus.failed_links}</span>
                  <span>Degraded {linkStatus.degraded_links}</span>
                </div>
                {linkStatus.reason_codes.length > 0 && (
                  <p className="mt-1 text-[11px] text-text-muted">
                    {linkStatus.reason_codes.join(", ")}
                  </p>
                )}
                {linkStatus.last_sync_error && (
                  <p className="mt-1 text-[11px] text-warning">{linkStatus.last_sync_error}</p>
                )}
              </div>
            )}
            {memoryStatus?.embedding_index_metadata?.length ? (
              <div className="rounded border border-border bg-bg-tertiary/40 px-3 py-2 text-xs text-text-secondary">
                <div className="grid gap-1 sm:grid-cols-2">
                  {memoryStatus.embedding_index_metadata.map((metadata) => (
                    <div key={metadata.index_id} className="min-w-0">
                      <span className="text-text-muted">{metadata.index_id}</span>{" "}
                      <span>{metadata.status}</span>{" "}
                      <span>{metadata.provider}:{metadata.model}</span>{" "}
                      <span>{metadata.dimensions ? `${metadata.dimensions}d` : "dims unknown"}</span>
                      {metadata.status_reason && (
                        <p className="truncate text-[11px] text-warning">{metadata.status_reason}</p>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            ) : null}
            {memoryStatus?.fallback_reason && (
              <p className="rounded border border-warning/30 bg-warning/5 px-2 py-1 text-xs text-warning">
                {memoryStatus.fallback_reason}
              </p>
            )}
            <button
              onClick={handleRunEmbeddingDiagnostics}
              disabled={runningEmbeddingDiagnostics}
              className="inline-flex items-center gap-2 rounded border border-border bg-bg-tertiary px-3 py-2 text-xs text-text hover:bg-border disabled:opacity-50"
            >
              {runningEmbeddingDiagnostics ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <TestTube2 className="h-3.5 w-3.5" />
              )}
              Run embedding diagnostics
            </button>
            {embeddingDiagnostics && (
              <div className="rounded border border-border bg-bg-tertiary/40 px-3 py-2 text-xs text-text-secondary">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span
                    className={
                      embeddingDiagnostics.native_fastembed.init_ok &&
                      embeddingDiagnostics.native_fastembed.embed_one_ok
                        ? "text-success"
                        : "text-warning"
                    }
                  >
                    {embeddingDiagnostics.native_fastembed.init_ok &&
                    embeddingDiagnostics.native_fastembed.embed_one_ok
                      ? "● Ready"
                      : "● Not ready"}
                  </span>
                  <span>
                    Backend: {embeddingDiagnostics.semantic_memory_provider.provider}
                  </span>
                  {embeddingDiagnostics.native_fastembed.dims != null && (
                    <span>{embeddingDiagnostics.native_fastembed.dims}d</span>
                  )}
                  <span>
                    Model cached:{" "}
                    {embeddingDiagnostics.native_fastembed.model_cached ? "yes" : "no"}
                  </span>
                </div>
                {embeddingDiagnostics.native_fastembed.error && (
                  <p className="mt-1 text-[11px] text-warning">
                    {embeddingDiagnostics.native_fastembed.error}
                  </p>
                )}
              </div>
            )}
            {profileStatus?.projection_summary && (
              <div className="rounded border border-border bg-bg-tertiary/40 px-3 py-2 text-xs text-text-secondary">
                <div className="grid gap-1 sm:grid-cols-4">
                  <span>Sources {profileStatus.projection_summary.total_sources}</span>
                  <span>Chunks {profileStatus.projection_summary.total_chunks}</span>
                  <span>Healthy {profileStatus.projection_summary.healthy_links}</span>
                  <span>Missing {profileStatus.projection_summary.missing_links}</span>
                </div>
                <div className="mt-1 grid gap-1 sm:grid-cols-3">
                  <span>Skipped {profileStatus.projection_summary.skipped_no_chunks}</span>
                  <span>Failed {profileStatus.projection_summary.failed_sources}</span>
                  <span>Fallback {profileStatus.fallback_allowed ? "on" : "off"}</span>
                </div>
                {profileStatus.blocking_reasons.length > 0 && (
                  <p className="mt-1 text-[11px] text-warning">
                    {profileStatus.blocking_reasons.join(", ")}
                  </p>
                )}
              </div>
            )}
            {profileStatus?.turbo_quant_status && (
              <div className="rounded border border-border bg-bg-tertiary/40 px-3 py-2 text-xs text-text-secondary">
                <div className="grid gap-1 sm:grid-cols-3">
                  <span>Compiled TQ {profileStatus.turbo_quant_status.compiled_turbo_quant ? "yes" : "no"}</span>
                  <span>Runtime TQ {profileStatus.turbo_quant_status.runtime_turbo_quant_enabled ? "on" : "off"}</span>
                  <span>Exact rerank {profileStatus.turbo_quant_status.exact_rerank ? "yes" : "no"}</span>
                </div>
                <p className="mt-1 truncate text-[11px] text-text-muted">
                  {profileStatus.turbo_quant_status.vector_artifact_manifest_digest || "No vector artifact digest"}
                </p>
              </div>
            )}
            <div className="grid gap-2 sm:grid-cols-3">
              {[
                ["gloss-local", "Gloss local", true],
                ["semantic-memory-safe", "Enable semantic-memory", semanticPreviewSelectable],
                ["semantic-memory-turbo-quant-safe", "TurboQuant", Boolean(turboQuant?.available)],
              ].map(([profile, label, enabled]) => (
                <button
                  key={String(profile)}
                  onClick={() => handleMemoryProfile(String(profile))}
                  disabled={!enabled}
                  className="inline-flex items-center justify-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <Database className="h-3.5 w-3.5" />
                  {label}
                </button>
              ))}
            </div>
            <div className="grid gap-2 sm:grid-cols-2">
              <button
                onClick={() => handleMemoryProfile("semantic-memory-strict")}
                disabled={!activeNotebookId || !semanticPreviewSelectable}
                className="inline-flex items-center justify-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
              >
                <ShieldCheck className="h-3.5 w-3.5" />
                Strict semantic-memory
              </button>
              <button
                onClick={() => handleMemoryProfile("semantic-memory-turbo-quant-strict")}
                disabled={!activeNotebookId || !turboQuant?.available}
                className="inline-flex items-center justify-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
              >
                <ShieldCheck className="h-3.5 w-3.5" />
                Strict TurboQuant
              </button>
            </div>
            <select
              value={settings["memory_backend"] || "gloss-local"}
              onChange={(e) => handleSelectMemoryBackend(e.target.value)}
              className="w-full rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
            >
              <option value="gloss-local">Gloss local</option>
              <option value="semantic-memory-preview" disabled={!semanticPreviewSelectable}>
                semantic-memory preview
                {semanticPreviewSelectable ? "" : ` (${semanticPreview?.unavailable_reason || "unavailable"})`}
              </option>
            </select>
            <label className="flex items-center gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={(settings["memory_backend_fallback"] || "true") !== "false"}
                readOnly
                disabled
                className="accent-accent"
              />
              Fallback to Gloss local when preview retrieval fails
            </label>
            <label className="flex items-center gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={(settings["semantic_memory_auto_project"] || "false") === "true"}
                readOnly
                disabled
                className="accent-accent"
              />
              Auto-project imports into semantic-memory
            </label>
            <label className="flex items-center gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={
                  (settings["semantic_memory_turbo_quant_require_fresh_artifacts"] || "true") !==
                  "false"
                }
                readOnly
                disabled
                className="accent-accent"
              />
              Require fresh TurboQuant artifact evidence
            </label>
            <label className="flex items-center gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={
                  (settings["semantic_memory_provekv_pool_candidates_enabled"] || "false") ===
                  "true"
                }
                onChange={(e) =>
                  updateSetting(
                    "semantic_memory_provekv_pool_candidates_enabled",
                    e.target.checked ? "true" : "false"
                  )
                }
                disabled={!turboQuant?.available}
                className="accent-accent"
              />
              Use proveKV pool candidates; exact f32 rerank stays mandatory
            </label>
            <div className="rounded border border-border bg-bg-tertiary/40 px-3 py-2">
              <div className="mb-1 flex items-center gap-1.5 text-xs font-medium text-text">
                <Cpu className="h-3.5 w-3.5 text-accent" />
                Embedding backend
              </div>
              <select
                value={settings["semantic_memory_embedding_provider"] || "fastembed"}
                onChange={(e) =>
                  updateSetting("semantic_memory_embedding_provider", e.target.value)
                }
                aria-label="Embedding backend"
                className="w-full rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
              >
                <option value="fastembed">
                  Automatic · CPU candle (local, no setup)
                </option>
                <option value="ollama">Ollama (external server)</option>
              </select>
              <p className="mt-1 text-[11px] text-text-muted">
                No Ollama? Gloss automatically falls back to the built-in CPU
                embedder (candle, nomic-embed-text-v1.5) — nothing else needs
                configuring.
              </p>
            </div>
            <label className="flex items-center gap-2 text-xs text-text-secondary">
              <input
                type="checkbox"
                checked={(settings["fastembed_download_consent"] || "true") === "true"}
                onChange={(e) =>
                  updateSetting("fastembed_download_consent", e.target.checked ? "true" : "false")
                }
                className="accent-accent"
              />
              Automatically download the embedding model on first use (~550MB)
            </label>
            <div className="flex flex-wrap items-center gap-2">
              <button
                onClick={handleReindexSemanticMemoryNotebook}
                disabled={
                  !activeNotebookId ||
                  !semanticPreview?.active ||
                  reindexingSemanticMemory
                }
                className="inline-flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
              >
                {reindexingSemanticMemory ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <RefreshCw className="h-3.5 w-3.5" />
                )}
                Run projection backfill
              </button>
              <button
                onClick={handleRebuildTurboQuantArtifacts}
                disabled={
                  !activeNotebookId ||
                  !turboQuant?.active ||
                  rebuildingTurboQuant
                }
                className="inline-flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
              >
                {rebuildingTurboQuant ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Database className="h-3.5 w-3.5" />
                )}
                Rebuild TurboQuant artifacts
              </button>
              <button
                onClick={handleRunRetrievalProbe}
                disabled={!activeNotebookId || runningRetrievalProbe}
                className="inline-flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
              >
                {runningRetrievalProbe ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <TestTube2 className="h-3.5 w-3.5" />
                )}
                Run retrieval probe
              </button>
              <button
                onClick={refreshMemoryEvidence}
                className="inline-flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text"
              >
                <RefreshCw className="h-3.5 w-3.5" />
                Refresh evidence
              </button>
            </div>
            <div className="grid gap-2 sm:grid-cols-2">
              <div className="relative">
                <input
                  value={embeddingUrl}
                  onChange={(e) => setEmbeddingUrl(e.target.value)}
                  className="rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none w-full"
                  aria-label="Embedding URL"
                />
                {(() => {
                  const urlClass = providerUrlClass(embeddingUrl);
                  if (urlClass === "lan" || urlClass === "remote" || urlClass === "cloud_https") {
                    return (
                      <p className="text-xs text-yellow-400 mt-1">
                        ⚠ Non-loopback embedding URL ({urlClass}) — ensure provider authority is explicit.
                      </p>
                    );
                  }
                  return null;
                })()}
              </div>
              <input
                value={embeddingModel}
                onChange={(e) => setEmbeddingModel(e.target.value)}
                className="rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
                aria-label="Embedding model"
              />
              <input
                type="number"
                min="1"
                value={embeddingTimeout}
                onChange={(e) => setEmbeddingTimeout(e.target.value)}
                className="rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
                aria-label="Embedding timeout seconds"
              />
              <input
                type="number"
                min="1"
                value={searchTimeout}
                onChange={(e) => setSearchTimeout(e.target.value)}
                className="rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
                aria-label="Search timeout milliseconds"
              />
              <input
                type="number"
                min="100"
                max="3000"
                value={chunkTargetTokens}
                onChange={(e) => setChunkTargetTokens(e.target.value)}
                className="rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
                aria-label="Chunk target tokens"
              />
            </div>
          </SettingsSection>

          <SettingsSection title="Sources & Ingestion" icon={<BookOpen className="h-4 w-4" />}>
            <FeatureStatusGrid flags={sections["Sources & Ingestion"] || []} />
          </SettingsSection>

          <SettingsSection title="Summaries" icon={<BookOpen className="h-4 w-4" />}>
            <FeatureStatusGrid flags={sections["Summaries"] || []} />
            <select
              value={settings["summary_model"] || ""}
              onChange={(e) => updateSetting("summary_model", e.target.value)}
              className="w-full rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
            >
              <option value="">Same as chat model ({activeModel})</option>
              {summaryModels.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.display_name}
                  {model.parameter_size ? ` (${model.parameter_size})` : ""}
                </option>
              ))}
            </select>
          </SettingsSection>

          <SettingsSection title="Vision & Media" icon={<Image className="h-4 w-4" />}>
            <FeatureStatusGrid flags={sections["Vision & Media"] || []} />
            <select
              value={settings["vision_model"] || ""}
              onChange={(e) => updateSetting("vision_model", e.target.value)}
              className="w-full rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none"
            >
              <option value="">Same as chat model ({activeModel})</option>
              {visionModels.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.display_name}
                  {model.parameter_size ? ` (${model.parameter_size})` : ""}
                </option>
              ))}
            </select>
          </SettingsSection>

          <SettingsSection title="External Tools" icon={<Wrench className="h-4 w-4" />}>
            <FeatureStatusGrid flags={sections["External Tools"] || []} />
            <ToolStatus name="ffmpeg" ready={externalTools["ffmpeg"]?.available ?? false} />
            <ToolStatus name="ffprobe" ready={externalTools["ffprobe"]?.available ?? false} />
          </SettingsSection>

          <SettingsSection title="Diagnostics" icon={<TestTube2 className="h-4 w-4" />}>
            <FeatureStatusGrid flags={sections["Diagnostics"] || []} />
            <div className="space-y-3 rounded border border-border bg-bg-tertiary/40 p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="min-w-0">
                  <p className="text-xs font-medium text-text">Database doctor</p>
                  <p className="truncate text-[11px] text-text-muted">
                    {dbDoctorReceipt
                      ? `${dbDoctorReceipt.schema} ${dbDoctorReceipt.receipt_id}`
                      : "Check first; repair writes provenance receipts only for material fixes."}
                  </p>
                </div>
                <div className="flex shrink-0 gap-2">
                  <button
                    onClick={handleCheckDatabaseDoctor}
                    disabled={runningDbDoctor !== null}
                    className="inline-flex items-center gap-1 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    {runningDbDoctor === "check" ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <TestTube2 className="h-3.5 w-3.5" />
                    )}
                    Check
                  </button>
                  <button
                    onClick={handleRepairDatabaseDoctor}
                    disabled={runningDbDoctor !== null}
                    className="inline-flex items-center gap-1 rounded border border-warning/40 bg-warning/10 px-2 py-1.5 text-xs text-warning hover:border-warning disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    {runningDbDoctor === "repair" ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Wrench className="h-3.5 w-3.5" />
                    )}
                    Repair
                  </button>
                </div>
              </div>
              {dbDoctorReceipt && (
                <div className="grid gap-2 sm:grid-cols-4">
                  <HealthCard
                    title="Notebooks"
                    status={String(dbDoctorReceipt.notebooks_checked)}
                    detail={dbDoctorReceipt.repair ? "repair mode" : "check mode"}
                    tone="neutral"
                  />
                  <HealthCard
                    title="Findings"
                    status={String(dbDoctorReceipt.findings.length)}
                    detail={
                      dbDoctorReceipt.findings[0]?.code || "No database findings"
                    }
                    tone={
                      dbDoctorReceipt.findings.some((finding) => finding.severity === "error")
                        ? "error"
                        : dbDoctorReceipt.findings.length > 0
                          ? "warning"
                          : "success"
                    }
                  />
                  <HealthCard
                    title="Source counts"
                    status={String(dbDoctorReceipt.repaired_source_count_mismatches)}
                    detail="source-count drift repairs"
                    tone={dbDoctorReceipt.repaired_source_count_mismatches > 0 ? "warning" : "success"}
                  />
                  <HealthCard
                    title="Orphans"
                    status={String(dbDoctorReceipt.repaired_orphan_rows)}
                    detail="auxiliary rows repaired"
                    tone={dbDoctorReceipt.repaired_orphan_rows > 0 ? "warning" : "success"}
                  />
                  <HealthCard
                    title="Failed imports"
                    status={String(dbDoctorReceipt.failed_import_sources)}
                    detail={`${dbDoctorReceipt.quarantined_failed_import_sources} quarantined`}
                    tone={dbDoctorReceipt.failed_import_sources > 0 ? "warning" : "success"}
                  />
                  <HealthCard
                    title="Queue jobs"
                    status={String(dbDoctorReceipt.queue_jobs_checked)}
                    detail={`${dbDoctorReceipt.repaired_stale_queue_jobs}/${dbDoctorReceipt.stale_queue_jobs} stale repaired`}
                    tone={dbDoctorReceipt.stale_queue_jobs > 0 ? "warning" : "success"}
                  />
                </div>
              )}
              {dbDoctorReceipt?.findings.length ? (
                <div className="max-h-32 space-y-1 overflow-y-auto rounded border border-border bg-bg-secondary/70 p-2">
                  {dbDoctorReceipt.findings.slice(0, 8).map((finding, index) => (
                    <div key={`${finding.notebook_id}:${finding.code}:${index}`} className="text-[11px] text-text-secondary">
                      <span className="font-medium text-text">{finding.code}</span>{" "}
                      <span className="text-text-muted">x{finding.count}</span>{" "}
                      <span className={finding.repaired ? "text-success" : "text-warning"}>
                        {finding.repaired ? "repaired" : finding.severity}
                      </span>
                    </div>
                  ))}
                </div>
              ) : null}
            </div>
          </SettingsSection>

          <SettingsSection title="Experimental Features" icon={<ShieldCheck className="h-4 w-4" />}>
            {experimentalMaster && (
              <FeatureToggleRow flag={experimentalMaster} mutable onToggle={handleFeatureToggle} />
            )}
            {[semanticPreview, turboQuant, ...(sections["Experimental Features"] || [])]
              .filter((flag): flag is FeatureFlagStatus => Boolean(flag))
              .filter((flag, index, flags) => flags.findIndex((item) => item.id === flag.id) === index)
              .filter((flag) => flag.id !== EXPERIMENTAL_FEATURES_ENABLED)
              .map((flag) => (
                <FeatureToggleRow key={flag.id} flag={flag} mutable onToggle={handleFeatureToggle} />
              ))}
          </SettingsSection>

          <SettingsSection title="Release / Validation" icon={<ShieldCheck className="h-4 w-4" />}>
            {(sections["Release / Validation"] || []).map((flag) => (
              <FeatureToggleRow key={flag.id} flag={flag} mutable onToggle={handleFeatureToggle} />
            ))}
          </SettingsSection>
        </div>
      </div>
    </div>
  );
}

function FeatureStatusGrid({ flags }: { flags: FeatureFlagStatus[] }) {
  if (flags.length === 0) {
    return (
      <p className="rounded border border-border bg-bg-tertiary/40 px-3 py-2 text-xs text-text-muted">
        No backend feature flags registered for this section.
      </p>
    );
  }
  return (
    <div className="grid gap-2 sm:grid-cols-2">
      {flags.map((flag) => (
        <HealthCard
          key={flag.id}
          title={flag.label}
          status={flag.active ? "active" : flag.enabled ? "enabled" : "off"}
          detail={flag.unavailable_reason || (flag.stable ? "Release default surface" : "Experimental surface")}
          tone={flag.active ? "success" : flag.available ? "neutral" : "warning"}
        />
      ))}
    </div>
  );
}

function ToolStatus({ name, ready }: { name: string; ready?: boolean }) {
  return (
    <div className="flex items-center gap-2 text-xs">
      {ready ? <Check className="h-3.5 w-3.5 text-success" /> : <AlertCircle className="h-3.5 w-3.5 text-warning" />}
      <span className="text-text">{name}</span>
      <span className="text-text-muted">{ready ? "Installed" : "Not found"}</span>
    </div>
  );
}

function HealthCard({
  title,
  status,
  detail,
  tone,
}: {
  title: string;
  status: string;
  detail: string;
  tone: "success" | "warning" | "error" | "neutral";
}) {
  const toneClass =
    tone === "success"
      ? "border-success/30 bg-success/5"
      : tone === "warning"
        ? "border-warning/30 bg-warning/5"
        : tone === "error"
          ? "border-error/30 bg-error/5"
          : "border-border bg-bg-tertiary";
  return (
    <div className={`rounded border px-3 py-2 ${toneClass}`}>
      <p className="text-[10px] uppercase tracking-wide text-text-muted">{title}</p>
      <p className="truncate text-xs font-medium text-text">{status}</p>
      <p className="mt-1 line-clamp-2 text-[11px] text-text-secondary">{detail}</p>
    </div>
  );
}
