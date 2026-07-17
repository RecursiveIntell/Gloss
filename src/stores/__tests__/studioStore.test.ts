import { beforeEach, describe, expect, it, vi } from "vitest";
import { useStudioStore } from "../studioStore";
import * as api from "../../lib/tauri";
import type { StudioOutput } from "../../lib/types";

vi.mock("../../lib/tauri", () => ({
  listStudioOutputs: vi.fn().mockResolvedValue([]),
  generateStudioOutput: vi.fn(),
  cancelStudioGeneration: vi.fn().mockResolvedValue(true),
  exportStudioOutput: vi.fn(),
}));

const output: StudioOutput = {
  id: "studio-output-1",
  output_type: "summary",
  title: "Summary",
  prompt_used: "source_grounded_refined_v1",
  raw_content: "{}",
  prose_content: "done",
  config: {
    schema: "StudioOutputConfigV1",
    attempt_id: "studio-attempt-test",
  },
  source_ids: ["src-1"],
  status: "ready",
  created_at: "2026-06-12T00:00:00Z",
};

describe("studioStore", () => {
  beforeEach(() => {
    useStudioStore.setState({
      outputs: [],
      activeOutputType: "report",
      activeOutputId: null,
      status: "idle",
      generationPhase: "idle",
      activeGeneration: null,
      error: null,
      lastExportReceipt: null,
    });
    vi.clearAllMocks();
  });

  it("reuses the active generation promise for repeated clicks", async () => {
    vi.mocked(api.generateStudioOutput).mockResolvedValue(output);

    const store = useStudioStore.getState();
    const requests = [
      store.generateOutput("nb-1", "summary", ["src-1"], 8),
      store.generateOutput("nb-1", "summary", ["src-1"], 8),
      store.generateOutput("nb-1", "summary", ["src-1"], 8),
      store.generateOutput("nb-1", "summary", ["src-1"], 8),
      store.generateOutput("nb-1", "summary", ["src-1"], 8),
    ];

    await Promise.all(requests);

    expect(api.generateStudioOutput).toHaveBeenCalledTimes(1);
    expect(useStudioStore.getState().outputs[0]?.id).toBe("studio-output-1");
  });

  it("cancels only the active Studio attempt", async () => {
    useStudioStore.setState({
      activeGeneration: {
        notebookId: "nb-1",
        attemptId: "studio-attempt-active",
        outputType: "summary",
      },
      status: "generating",
      generationPhase: "first_token_wait",
    });

    await expect(useStudioStore.getState().cancelGeneration("nb-1")).resolves.toBe(true);

    expect(api.cancelStudioGeneration).toHaveBeenCalledWith("nb-1", "studio-attempt-active");
    expect(useStudioStore.getState().generationPhase).toBe("cancelled");
  });

  it("keeps notebook generations independent and ignores a late A result after B activation", async () => {
    let resolveA!: (value: StudioOutput) => void;
    let resolveB!: (value: StudioOutput) => void;
    vi.mocked(api.generateStudioOutput)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveA = resolve; }))
      .mockImplementationOnce(() => new Promise((resolve) => { resolveB = resolve; }));
    useStudioStore.setState({ loadedNotebookId: "nb-a" });
    const a = useStudioStore.getState().generateOutput("nb-a", "summary", [], 8);
    useStudioStore.setState({ loadedNotebookId: "nb-b", outputs: [], activeOutputId: null });
    const b = useStudioStore.getState().generateOutput("nb-b", "summary", [], 8);
    expect(api.generateStudioOutput).toHaveBeenCalledTimes(2);
    resolveA({ ...output, id: "a-output" });
    await a;
    expect(useStudioStore.getState().outputs).toEqual([]);
    resolveB({ ...output, id: "b-output" });
    await b;
    expect(useStudioStore.getState().outputs.map((item) => item.id)).toEqual(["b-output"]);
  });

  it("resets output selection and export receipt on notebook activation", async () => {
    useStudioStore.setState({ activeOutputId: "old", lastExportReceipt: { schema: "StudioExportReceiptV1", receipt_id: "r", output_id: "old", output_type: "summary", notebook_id: "nb-a", format: "json", file_path: "x", file_path_redacted: "x", bytes_written: 1, sha256: "sha", recorded_utc: "now" } });
    vi.mocked(api.listStudioOutputs).mockResolvedValueOnce([]);
    await useStudioStore.getState().loadOutputs("nb-b");
    expect(useStudioStore.getState().activeOutputId).toBeNull();
    expect(useStudioStore.getState().lastExportReceipt).toBeNull();
  });
});
