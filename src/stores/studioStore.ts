import { create } from "zustand";
import * as api from "../lib/tauri";
import type { StudioExportReceipt, StudioOutput } from "../lib/types";

type StudioStatus = "idle" | "loading" | "generating" | "exporting" | "error";

interface StudioStore {
  outputs: StudioOutput[];
  activeOutputType: string;
  activeOutputId: string | null;
  status: StudioStatus;
  error: string | null;
  lastExportReceipt: StudioExportReceipt | null;
  setActiveOutputType: (type: string) => void;
  setActiveOutputId: (id: string | null) => void;
  loadOutputs: (notebookId: string) => Promise<void>;
  generateOutput: (
    notebookId: string,
    outputType: string,
    sourceIds: string[],
    maxItems: number
  ) => Promise<StudioOutput | null>;
  exportOutput: (notebookId: string, outputId: string) => Promise<StudioExportReceipt | null>;
}

export const useStudioStore = create<StudioStore>((set) => ({
  outputs: [],
  activeOutputType: "report",
  activeOutputId: null,
  status: "idle",
  error: null,
  lastExportReceipt: null,

  setActiveOutputType: (type) => set({ activeOutputType: type }),
  setActiveOutputId: (id) => set({ activeOutputId: id }),

  loadOutputs: async (notebookId) => {
    set({ status: "loading", error: null });
    try {
      const outputs = await api.listStudioOutputs(notebookId);
      set((state) => ({
        outputs,
        activeOutputId: state.activeOutputId ?? outputs[0]?.id ?? null,
        status: "idle",
      }));
    } catch (error) {
      set({
        status: "error",
        error: error instanceof Error ? error.message : String(error),
      });
    }
  },

  generateOutput: async (notebookId, outputType, sourceIds, maxItems) => {
    set({ status: "generating", error: null });
    try {
      const output = await api.generateStudioOutput(
        notebookId,
        outputType,
        sourceIds.length > 0 ? sourceIds : undefined,
        undefined,
        maxItems
      );
      set((state) => ({
        outputs: [output, ...state.outputs.filter((item) => item.id !== output.id)],
        activeOutputId: output.id,
        status: "idle",
      }));
      return output;
    } catch (error) {
      set({
        status: "error",
        error: error instanceof Error ? error.message : String(error),
      });
      return null;
    }
  },

  exportOutput: async (notebookId, outputId) => {
    set({ status: "exporting", error: null });
    try {
      const receipt = await api.exportStudioOutput(notebookId, outputId);
      const outputs = await api.listStudioOutputs(notebookId);
      set({
        outputs,
        activeOutputId: outputId,
        status: "idle",
        lastExportReceipt: receipt,
      });
      return receipt;
    } catch (error) {
      set({
        status: "error",
        error: error instanceof Error ? error.message : String(error),
      });
      return null;
    }
  },
}));
