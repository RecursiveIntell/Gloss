import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", () => ({
  listNotebooks: vi.fn(),
  createNotebook: vi.fn(),
  renameNotebook: vi.fn(),
  deleteNotebook: vi.fn(),
  setActiveNotebook: vi.fn(),
}));
vi.mock("../chatStore", () => ({
  useChatStore: { getState: () => ({ resetForNotebookSwitch: vi.fn() }) },
}));
vi.mock("../noteStore", () => ({
  useNoteStore: { getState: () => ({ resetForNotebookSwitch: vi.fn() }) },
}));
vi.mock("../sourceStore", () => ({
  useSourceStore: { getState: () => ({ resetForNotebookSwitch: vi.fn() }) },
}));
vi.mock("../notebookRefresh", () => ({ registerNotebookListRefresher: vi.fn() }));

describe("notebook startup authority", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.resetModules();
    const values = new Map([["gloss:activeNotebookId", "nb-persisted"]]);
    vi.stubGlobal("localStorage", {
      getItem: vi.fn((key: string) => values.get(key) ?? null),
      setItem: vi.fn((key: string, value: string) => values.set(key, value)),
      removeItem: vi.fn((key: string) => values.delete(key)),
    });
  });

  it("keeps a persisted notebook as a hint until backend activation succeeds", async () => {
    const { readActiveNotebookId, useNotebookStore } = await import("../notebookStore");

    expect(readActiveNotebookId()).toBe("nb-persisted");
    expect(useNotebookStore.getState()).toMatchObject({
      activeNotebookId: null,
      activationStatus: "idle",
      activationTargetId: null,
    });
  });
});
