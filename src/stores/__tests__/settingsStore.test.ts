import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSettingsStore } from "../settingsStore";
import * as api from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({ updateSetting: vi.fn().mockResolvedValue(undefined) }));

describe("settings provider/model selection", () => {
  beforeEach(() => {
    useSettingsStore.setState({ models: [{ id: "model-a", provider_id: "provider-a", display_name: "A", available: true, stale: false }], settings: {}, activeModel: "old", selectionError: null });
    vi.clearAllMocks();
  });

  it("commits a valid provider/model pair together", async () => {
    await useSettingsStore.getState().selectModel("provider-a", "model-a");
    expect(useSettingsStore.getState().settings).toMatchObject({ default_provider: "provider-a", default_model: "model-a" });
    expect(api.updateSetting).toHaveBeenCalledTimes(2);
  });

  it("rejects an invalid pair visibly without changing the selection", async () => {
    await expect(useSettingsStore.getState().selectModel("provider-b", "model-a")).rejects.toThrow();
    expect(useSettingsStore.getState().activeModel).toBe("old");
    expect(useSettingsStore.getState().selectionError).toMatch(/not registered/);
  });
});
