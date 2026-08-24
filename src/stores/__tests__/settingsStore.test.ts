import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSettingsStore } from "../settingsStore";
import * as api from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({
  getSettings: vi.fn(),
  getProviders: vi.fn(),
  getAllModels: vi.fn(),
  refreshModels: vi.fn(),
  updateSetting: vi.fn(),
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
