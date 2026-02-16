import { useEffect, useState } from "react";
import { useSettingsStore } from "../../stores/settingsStore";
import {
  X,
  RefreshCw,
  Check,
  AlertCircle,
  Loader2,
  Server,
  Cpu,
  BookOpen,
} from "lucide-react";

interface SettingsDialogProps {
  open: boolean;
  onClose: () => void;
}

export function SettingsDialog({ open, onClose }: SettingsDialogProps) {
  const {
    providers,
    models,
    settings,
    activeModel,
    loading,
    loadSettings,
    loadProviders,
    loadModels,
    refreshModels,
    updateSetting,
    updateProvider,
    setActiveModel,
    testProvider,
  } = useSettingsStore();

  const [ollamaUrl, setOllamaUrl] = useState("http://localhost:11434");
  const [testStatus, setTestStatus] = useState<
    "idle" | "testing" | "success" | "error"
  >("idle");
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    if (open) {
      loadSettings();
      loadProviders();
      loadModels();
    }
  }, [open, loadSettings, loadProviders, loadModels]);

  useEffect(() => {
    const url = settings["ollama_url"] || "http://localhost:11434";
    setOllamaUrl(url);
    setDirty(false);
  }, [settings]);

  if (!open) return null;

  const handleSave = async () => {
    await updateSetting("ollama_url", ollamaUrl);
    const ollama = providers.find((p) => p.id === "ollama");
    await updateProvider("ollama", ollama?.enabled ?? true, ollamaUrl);
    setDirty(false);
  };

  const handleTest = async () => {
    // Save first so test uses the current URL
    if (dirty) await handleSave();
    setTestStatus("testing");
    const ok = await testProvider("ollama");
    setTestStatus(ok ? "success" : "error");
    setTimeout(() => setTestStatus("idle"), 3000);
  };

  const handleRefresh = async () => {
    if (dirty) await handleSave();
    await refreshModels();
  };

  const handleSelectModel = async (modelId: string) => {
    setActiveModel(modelId);
    await updateSetting("default_model", modelId);
  };

  const handleSelectSummaryModel = async (modelId: string) => {
    await updateSetting("summary_model", modelId);
  };

  const handleBackdropClick = (e: React.MouseEvent) => {
    if (e.target === e.currentTarget) onClose();
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
          {/* Provider Section */}
          <section>
            <div className="flex items-center gap-2 mb-3">
              <Server className="w-4 h-4 text-text-secondary" />
              <h3 className="text-xs font-semibold text-text uppercase tracking-wide">
                Ollama Provider
              </h3>
            </div>

            <div className="space-y-2">
              <label className="block text-xs text-text-secondary">
                Server URL
              </label>
              <div className="flex gap-2">
                <input
                  type="text"
                  value={ollamaUrl}
                  onChange={(e) => {
                    setOllamaUrl(e.target.value);
                    setDirty(true);
                  }}
                  className="flex-1 px-2 py-1.5 text-sm bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
                  placeholder="http://localhost:11434"
                />
                <button
                  onClick={handleSave}
                  disabled={!dirty}
                  className="px-3 py-1.5 text-xs bg-accent text-white rounded hover:bg-accent-hover disabled:opacity-40 disabled:cursor-not-allowed"
                >
                  Save
                </button>
              </div>

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
                {testStatus === "idle" && (
                  <Server className="w-3 h-3" />
                )}
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
                No models found. Click Refresh to fetch from Ollama.
              </p>
            ) : (
              <div className="space-y-0.5 max-h-64 overflow-y-auto">
                {models.map((model) => (
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
              Model used for generating source summaries. A smaller, faster model
              works well here since summaries run in the background.
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
        </div>
      </div>
    </div>
  );
}
