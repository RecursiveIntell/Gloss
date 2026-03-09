import { useEffect, useState } from "react";
import { useSettingsStore } from "../../stores/settingsStore";
import { useToastStore } from "../../stores/toastStore";
import {
  X,
  RefreshCw,
  Check,
  AlertCircle,
  Loader2,
  Server,
  Cpu,
  BookOpen,
  Key,
  Eye,
  EyeOff,
  Image,
  Wrench,
} from "lucide-react";

interface SettingsDialogProps {
  open: boolean;
  onClose: () => void;
}

type TestStatus = "idle" | "testing" | "success" | "error";

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

  useEffect(() => {
    setUrl(settings[urlKey] || urlDefault);
    if (apiKeyKey) setApiKey(settings[apiKeyKey] || "");
    setDirty(false);
  }, [settings, urlKey, urlDefault, apiKeyKey]);

  const handleSave = async () => {
    const updates: Record<string, string> = { [urlKey]: url };
    if (apiKeyKey) updates[apiKeyKey] = apiKey;
    await onSave(updates);
    setDirty(false);
  };

  const handleTest = async () => {
    if (dirty) await handleSave();
    setTestStatus("testing");
    const ok = await testProvider(id);
    setTestStatus(ok ? "success" : "error");
    if (ok) {
      useToastStore.getState().addToast({
        type: "success",
        title: `${label} Connected`,
        message: "Provider is reachable",
        duration: 3000,
      });
    } else {
      useToastStore.getState().addToast({
        type: "error",
        title: `${label} Failed`,
        message: "Could not connect to provider",
        duration: 5000,
      });
    }
    setTimeout(() => setTestStatus("idle"), 3000);
  };

  return (
    <section>
      <div className="flex items-center gap-2 mb-3">
        <Server className="w-4 h-4 text-text-secondary" />
        <h3 className="text-xs font-semibold text-text uppercase tracking-wide">
          {label}
        </h3>
      </div>
      <div className="space-y-2">
        <label className="block text-xs text-text-secondary">Server URL</label>
        <div className="flex gap-2">
          <input
            type="text"
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              setDirty(true);
            }}
            className="flex-1 px-2 py-1.5 text-sm bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
            placeholder={urlDefault}
          />
          <button
            onClick={handleSave}
            disabled={!dirty}
            className="px-3 py-1.5 text-xs bg-accent text-white rounded hover:bg-accent-hover disabled:opacity-40 disabled:cursor-not-allowed"
          >
            Save
          </button>
        </div>

        {apiKeyKey && (
          <>
            <label className="block text-xs text-text-secondary">
              <Key className="w-3 h-3 inline mr-1" />
              API Key
            </label>
            <div className="flex gap-2">
              <div className="flex-1 relative">
                <input
                  type={showKey ? "text" : "password"}
                  value={apiKey}
                  onChange={(e) => {
                    setApiKey(e.target.value);
                    setDirty(true);
                  }}
                  className="w-full px-2 py-1.5 pr-8 text-sm bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
                  placeholder="sk-..."
                />
                <button
                  onClick={() => setShowKey(!showKey)}
                  className="absolute right-2 top-1/2 -translate-y-1/2 text-text-muted hover:text-text"
                >
                  {showKey ? (
                    <EyeOff className="w-3.5 h-3.5" />
                  ) : (
                    <Eye className="w-3.5 h-3.5" />
                  )}
                </button>
              </div>
            </div>
          </>
        )}

        <button
          onClick={handleTest}
          disabled={testStatus === "testing"}
          className="flex items-center gap-1.5 px-2 py-1 text-xs bg-bg-tertiary rounded hover:bg-border text-text-secondary hover:text-text disabled:opacity-50"
        >
          {testStatus === "testing" && (
            <Loader2 className="w-3 h-3 animate-spin" />
          )}
          {testStatus === "success" && (
            <Check className="w-3 h-3 text-success" />
          )}
          {testStatus === "error" && (
            <AlertCircle className="w-3 h-3 text-error" />
          )}
          {testStatus === "idle" && <Server className="w-3 h-3" />}
          {testStatus === "testing"
            ? "Testing..."
            : testStatus === "success"
              ? "Connected"
              : testStatus === "error"
                ? "Connection failed"
                : "Test Connection"}
        </button>
      </div>
    </section>
  );
}

export function SettingsDialog({ open, onClose }: SettingsDialogProps) {
  const {
    models,
    settings,
    activeModel,
    loading,
    externalTools,
    loadSettings,
    loadProviders,
    loadModels,
    refreshModels,
    updateSetting,
    updateProvider,
    setActiveModel,
    loadExternalTools,
  } = useSettingsStore();

  useEffect(() => {
    if (open) {
      loadSettings();
      loadProviders();
      loadModels();
      loadExternalTools();
    }
  }, [open, loadSettings, loadProviders, loadModels, loadExternalTools]);

  if (!open) return null;

  const handleProviderSave = async (updates: Record<string, string>) => {
    for (const [key, value] of Object.entries(updates)) {
      await updateSetting(key, value);
    }
    // Update provider record if URL changed
    if (updates["ollama_url"]) {
      await updateProvider("ollama", true, updates["ollama_url"]);
    }
    if (updates["openai_api_key"] || updates["openai_base_url"]) {
      await updateProvider(
        "openai",
        true,
        updates["openai_base_url"],
        updates["openai_api_key"]
      );
    }
    if (updates["anthropic_api_key"] || updates["anthropic_base_url"]) {
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
  };

  const handleRefresh = async () => {
    await refreshModels();
  };

  const handleSelectModel = async (modelId: string) => {
    setActiveModel(modelId);
    await updateSetting("default_model", modelId);
  };

  const handleSelectSummaryModel = async (modelId: string) => {
    await updateSetting("summary_model", modelId);
  };

  const handleSelectVisionModel = async (modelId: string) => {
    await updateSetting("vision_model", modelId);
  };

  const handleBackdropClick = (e: React.MouseEvent) => {
    if (e.target === e.currentTarget) onClose();
  };

  // Group models by provider_id for display
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

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      onClick={handleBackdropClick}
    >
      <div className="w-[560px] max-h-[80vh] bg-bg-secondary border border-border rounded-lg shadow-xl flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-border">
          <h2 className="text-sm font-semibold text-text">Settings</h2>
          <button
            onClick={onClose}
            className="p-1 rounded hover:bg-bg-tertiary text-text-secondary hover:text-text"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-4 space-y-6">
          {/* Ollama */}
          <ProviderSection
            id="ollama"
            label="Ollama"
            urlKey="ollama_url"
            urlDefault="http://localhost:11434"
            settings={settings}
            onSave={handleProviderSave}
          />

          {/* OpenAI */}
          <ProviderSection
            id="openai"
            label="OpenAI"
            urlKey="openai_base_url"
            urlDefault="https://api.openai.com/v1"
            apiKeyKey="openai_api_key"
            settings={settings}
            onSave={handleProviderSave}
          />

          {/* Anthropic */}
          <ProviderSection
            id="anthropic"
            label="Anthropic"
            urlKey="anthropic_base_url"
            urlDefault="https://api.anthropic.com/v1"
            apiKeyKey="anthropic_api_key"
            settings={settings}
            onSave={handleProviderSave}
          />

          {/* llama.cpp */}
          <ProviderSection
            id="llamacpp"
            label="llama.cpp"
            urlKey="llamacpp_url"
            urlDefault="http://localhost:8080/v1"
            settings={settings}
            onSave={handleProviderSave}
          />

          {/* Models Section */}
          <section>
            <div className="flex items-center justify-between mb-3">
              <div className="flex items-center gap-2">
                <Cpu className="w-4 h-4 text-text-secondary" />
                <h3 className="text-xs font-semibold text-text uppercase tracking-wide">
                  Models
                </h3>
              </div>
              <button
                onClick={handleRefresh}
                disabled={loading}
                className="flex items-center gap-1 px-2 py-1 text-xs bg-bg-tertiary rounded hover:bg-border text-text-secondary hover:text-text disabled:opacity-50"
              >
                <RefreshCw
                  className={`w-3 h-3 ${loading ? "animate-spin" : ""}`}
                />
                Refresh
              </button>
            </div>

            {models.length === 0 ? (
              <p className="text-xs text-text-muted py-4 text-center">
                No models found. Configure a provider above and click Refresh.
              </p>
            ) : (
              <div className="space-y-1 max-h-64 overflow-y-auto">
                {Object.entries(providerGroups).map(
                  ([providerId, groupModels]) => (
                    <div key={providerId}>
                      <div className="px-3 py-1 text-[10px] font-semibold text-text-muted uppercase tracking-wider">
                        {providerLabels[providerId] || providerId}
                      </div>
                      {groupModels.map((model) => (
                        <button
                          key={model.id}
                          onClick={() => handleSelectModel(model.id)}
                          className={`w-full flex items-center gap-3 px-3 py-2 rounded text-left ${
                            activeModel === model.id
                              ? "bg-accent/10 border border-accent/30"
                              : "hover:bg-bg-tertiary border border-transparent"
                          }`}
                        >
                          <div
                            className={`w-3 h-3 rounded-full border-2 shrink-0 ${
                              activeModel === model.id
                                ? "border-accent bg-accent"
                                : "border-text-muted"
                            }`}
                          />
                          <div className="flex-1 min-w-0">
                            <p className="text-xs text-text truncate">
                              {model.display_name}
                            </p>
                          </div>
                          {model.parameter_size && (
                            <span className="text-[10px] px-1.5 py-0.5 bg-bg-tertiary rounded text-text-muted shrink-0">
                              {model.parameter_size}
                            </span>
                          )}
                          {model.context_window && (
                            <span className="text-[10px] text-text-muted shrink-0">
                              {(model.context_window / 1024).toFixed(0)}k ctx
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  )
                )}
              </div>
            )}
          </section>

          {/* Summary Model Section */}
          <section>
            <div className="flex items-center gap-2 mb-3">
              <BookOpen className="w-4 h-4 text-text-secondary" />
              <h3 className="text-xs font-semibold text-text uppercase tracking-wide">
                Summary Model
              </h3>
            </div>
            <p className="text-xs text-text-muted mb-2">
              Model used for generating source summaries. A smaller, faster
              model works well here since summaries run in the background.
            </p>
            <select
              value={settings["summary_model"] || ""}
              onChange={(e) => handleSelectSummaryModel(e.target.value)}
              className="w-full px-2 py-1.5 text-sm bg-bg-tertiary border border-border rounded text-text focus:outline-none focus:border-accent"
            >
              <option value="">Same as chat model ({activeModel})</option>
              {models.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.display_name}
                  {model.parameter_size ? ` (${model.parameter_size})` : ""}
                </option>
              ))}
            </select>
          </section>

          {/* Vision Model Section */}
          <section>
            <div className="flex items-center gap-2 mb-3">
              <Image className="w-4 h-4 text-text-secondary" />
              <h3 className="text-xs font-semibold text-text uppercase tracking-wide">
                Vision Model
              </h3>
            </div>
            <p className="text-xs text-text-muted mb-2">
              Model used for describing images. Must be a vision-capable model
              (e.g. llava, bakllava, moondream). Images are base64-encoded and
              sent to this model for description.
            </p>
            <select
              value={settings["vision_model"] || ""}
              onChange={(e) => handleSelectVisionModel(e.target.value)}
              className="w-full px-2 py-1.5 text-sm bg-bg-tertiary border border-border rounded text-text focus:outline-none focus:border-accent"
            >
              <option value="">Same as chat model ({activeModel})</option>
              {models.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.display_name}
                  {model.parameter_size ? ` (${model.parameter_size})` : ""}
                </option>
              ))}
            </select>
          </section>

          {/* External Tools Section */}
          <section>
            <div className="flex items-center gap-2 mb-3">
              <Wrench className="w-4 h-4 text-text-secondary" />
              <h3 className="text-xs font-semibold text-text uppercase tracking-wide">
                External Tools
              </h3>
            </div>
            <div className="space-y-1.5">
              <div className="flex items-center gap-2 text-xs">
                {externalTools["ffmpeg"] ? (
                  <Check className="w-3.5 h-3.5 text-success" />
                ) : (
                  <AlertCircle className="w-3.5 h-3.5 text-warning" />
                )}
                <span className="text-text">ffmpeg</span>
                <span className="text-text-muted">
                  {externalTools["ffmpeg"]
                    ? "Installed — video frame analysis enabled"
                    : "Not found — video import requires ffmpeg"}
                </span>
              </div>
              <div className="flex items-center gap-2 text-xs">
                {externalTools["ffprobe"] ? (
                  <Check className="w-3.5 h-3.5 text-success" />
                ) : (
                  <AlertCircle className="w-3.5 h-3.5 text-warning" />
                )}
                <span className="text-text">ffprobe</span>
                <span className="text-text-muted">
                  {externalTools["ffprobe"]
                    ? "Installed"
                    : "Not found — needed for video duration detection"}
                </span>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
