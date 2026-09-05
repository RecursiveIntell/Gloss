import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSettingsStore } from "../settingsStore";
import * as api from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({
  getSettings: vi.fn(),
  getProviders: vi.fn(),
  getAllModels: vi.fn(),
  refreshModels: vi.fn(),
  updateSetting: vi.fn(),
  updateEmbeddingSettings: vi.fn(),
  updateFeatureFlag: vi.fn(),
  updateProvider: vi.fn(),
  getFeatureFlags: vi.fn(),
  testProvider: vi.fn(),
  checkExternalTools: vi.fn(),
}));

vi.mock("../toastStore", () => ({
  useToastStore: {
    getState: () => ({ addToast: vi.fn() }),
  },
}));

describe("settingsStore model readiness", () => {
  beforeEach(() => {
    useSettingsStore.setState({
      providers: [],
      models: [],
      settings: {},
      featureFlags: [],
      activeModel: "",
      selectionError: null,
      loading: false,
      externalTools: {},
    });
    vi.clearAllMocks();
  });

  it("serializes saves and does not display an unacknowledged value", async () => {
    let rejectFirst!: (reason: Error) => void;
    vi.mocked(api.updateSetting).mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { rejectFirst = reject; })).mockResolvedValueOnce();
    useSettingsStore.setState({ settings: { chunk_target_tokens: "1100" } });
    const first = useSettingsStore.getState().updateSetting("chunk_target_tokens", "1200");
    const rejected = expect(first).rejects.toThrow("disk full");
    const second = useSettingsStore.getState().updateSetting("chunk_target_tokens", "1300");
    await Promise.resolve();
    expect(api.updateSetting).toHaveBeenCalledTimes(1);
    expect(useSettingsStore.getState().settings.chunk_target_tokens).toBe("1100");
    rejectFirst(new Error("disk full"));
    await rejected;
    await second;
    expect(vi.mocked(api.updateSetting).mock.calls).toEqual([["chunk_target_tokens", "1200"], ["chunk_target_tokens", "1300"]]);
    expect(useSettingsStore.getState().settings.chunk_target_tokens).toBe("1300");
  });

  it("applies the whole acknowledged embedding pair and preserves unrelated settings", async () => {
    let acknowledge!: (warnings: string[]) => void;
    vi.mocked(api.updateEmbeddingSettings).mockImplementationOnce(() => new Promise(resolve => { acknowledge = resolve; }));
    useSettingsStore.setState({ settings: { semantic_memory_embedding_provider: "ollama", semantic_memory_embedding_model: "old", summary_model: "summary" } });
    const config = { provider: "fastembed", url: "http://localhost:11434", model: "next", timeout_secs: 60, download_consent: false, search_timeout_ms: 8000, chunk_target_tokens: 1100 };
    const saving = useSettingsStore.getState().applyEmbeddingSettings(config);
    await Promise.resolve();
    expect(useSettingsStore.getState().settings.semantic_memory_embedding_model).toBe("old");
    acknowledge([]);
    await saving;
    expect(useSettingsStore.getState().settings).toMatchObject({ semantic_memory_embedding_provider: "fastembed", semantic_memory_embedding_model: "next", fastembed_download_consent: "false", summary_model: "summary" });
  });

  it("does not invent a fallback model when the backend has no configured default", async () => {
    vi.mocked(api.getSettings).mockResolvedValue({ default_provider: "ollama" });

    await useSettingsStore.getState().loadSettings();

    expect(useSettingsStore.getState().activeModel).toBe("");
    expect(useSettingsStore.getState().selectionError).toBe(
      "No default model is configured. Select an available model in Settings."
    );
  });

  it("keeps the backend configured default model as the active selection", async () => {
    vi.mocked(api.getSettings).mockResolvedValue({
      default_provider: "ollama",
      default_model: "qwen3.5:4b",
    });

    await useSettingsStore.getState().loadSettings();

    expect(useSettingsStore.getState().activeModel).toBe("qwen3.5:4b");
    expect(useSettingsStore.getState().selectionError).toBeNull();
  });

  it("surfaces a stale selected model after refresh instead of treating it as chat-ready", async () => {
    useSettingsStore.setState({
      activeModel: "removed-model",
      settings: { default_provider: "ollama", default_model: "removed-model" },
    });
    vi.mocked(api.getAllModels).mockResolvedValue([
      {
        id: "removed-model",
        provider_id: "ollama",
        display_name: "Removed model",
        parameter_size: undefined,
        context_window: undefined,
        capabilities: undefined,
        available: false,
        stale: true,
        last_error: "model no longer installed",
      },
    ]);

    await useSettingsStore.getState().loadModels();

    expect(useSettingsStore.getState().selectionError).toBe(
      "Selected model 'removed-model' is unavailable. Refresh models or select an available model."
    );
  });
});
