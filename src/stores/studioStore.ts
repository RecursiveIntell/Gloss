import { create } from "zustand";
import * as api from "../lib/tauri";
import type { StudioExportReceipt, StudioOutput } from "../lib/types";

type StudioStatus = "idle" | "loading" | "generating" | "exporting" | "error";
type StudioGenerationPhase =
  | "idle"
  | "source_readiness"
  | "provider_start"
  | "first_token_wait"
  | "streaming"
  | "fallback"
  | "cancelled"
  | "error";

interface ActiveStudioGeneration {
  notebookId: string;
  attemptId: string;
  outputType: string;
}

interface StudioStore {
  outputs: StudioOutput[];
  activeOutputType: string;
  activeOutputId: string | null;
  status: StudioStatus;
  generationPhase: StudioGenerationPhase;
  activeGeneration: ActiveStudioGeneration | null;
  loadedNotebookId: string | null;
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
  cancelGeneration: (notebookId: string) => Promise<boolean>;
  exportOutput: (notebookId: string, outputId: string) => Promise<StudioExportReceipt | null>;
}

type GenerationEntry = { notebookId: string; attemptId: string; promise: Promise<StudioOutput | null> };
const generationByNotebook = new Map<string, GenerationEntry>();

function newStudioAttemptId(): string {
  const random =
    globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  return `studio-attempt-${random}`;
}

export const useStudioStore = create<StudioStore>((set, get) => ({
  outputs: [],
  activeOutputType: "report",
  activeOutputId: null,
  status: "idle",
  generationPhase: "idle",
  activeGeneration: null,
  loadedNotebookId: null,
  error: null,
  lastExportReceipt: null,

  setActiveOutputType: (type) => set({ activeOutputType: type }),
  setActiveOutputId: (id) => set({ activeOutputId: id }),

  loadOutputs: async (notebookId) => {
    const existingGeneration = generationByNotebook.get(notebookId);
    set({
      loadedNotebookId: notebookId,
      outputs: [],
      activeOutputType: "report",
      activeOutputId: null,
      lastExportReceipt: null,
      activeGeneration: existingGeneration ? { notebookId, attemptId: existingGeneration.attemptId, outputType: "unknown" } : null,
      status: existingGeneration ? "generating" : "loading",
      error: null,
    });
    try {
      const outputs = await api.listStudioOutputs(notebookId);
      if (get().loadedNotebookId !== notebookId) return;
      set(() => ({
        outputs,
        activeOutputId: outputs[0]?.id ?? null,
        status: "idle",
      }));
    } catch (error) {
      if (get().loadedNotebookId !== notebookId) return;
      set({
        status: "error",
        error: error instanceof Error ? error.message : String(error),
      });
    }
  },

  generateOutput: async (notebookId, outputType, sourceIds, maxItems) => {
    const existing = generationByNotebook.get(notebookId);
    if (existing) {
      return existing.promise;
    }
    if (get().loadedNotebookId == null) set({ loadedNotebookId: notebookId });
    const attemptId = newStudioAttemptId();
    set({
      status: "generating",
      generationPhase: "source_readiness",
      activeGeneration: { notebookId, attemptId, outputType },
      error: null,
    });
    const promise = (async () => {
      let firstTokenTimer: ReturnType<typeof setTimeout> | null = null;
      try {
        set({ generationPhase: "provider_start" });
        firstTokenTimer = globalThis.setTimeout(() => {
          set((state) =>
            state.activeGeneration?.attemptId === attemptId
              ? { generationPhase: "first_token_wait" }
              : {}
          );
        }, 1200);
        const output = await api.generateStudioOutput(
          notebookId,
          outputType,
          sourceIds.length > 0 ? sourceIds : undefined,
          undefined,
          maxItems,
          attemptId
        );
        if (firstTokenTimer) {
          globalThis.clearTimeout(firstTokenTimer);
          firstTokenTimer = null;
        }
        if (get().loadedNotebookId !== notebookId || get().activeGeneration?.attemptId !== attemptId) return output;
        const fellBack = output.config?.fallback_receipt != null;
        set((state) => ({
          outputs: [output, ...state.outputs.filter((item) => item.id !== output.id)],
          activeOutputId: output.id,
          status: "idle",
          generationPhase: fellBack ? "fallback" : "idle",
          activeGeneration: null,
        }));
        return output;
      } catch (error) {
        if (firstTokenTimer) {
          globalThis.clearTimeout(firstTokenTimer);
        }
        const message = error instanceof Error ? error.message : String(error);
        const cancelled = message.toLowerCase().includes("cancelled");
        if (get().loadedNotebookId === notebookId && get().activeGeneration?.attemptId === attemptId) set({
          status: cancelled ? "idle" : "error",
          generationPhase: cancelled ? "cancelled" : "error",
          activeGeneration: null,
          error: cancelled ? null : message,
        });
        return null;
      } finally {
        const current = generationByNotebook.get(notebookId);
        if (current?.attemptId === attemptId) generationByNotebook.delete(notebookId);
      }
    })();
    generationByNotebook.set(notebookId, { notebookId, attemptId, promise });
    return promise;
  },

  cancelGeneration: async (notebookId) => {
    const active = get().activeGeneration?.notebookId === notebookId ? get().activeGeneration : generationByNotebook.get(notebookId);
    if (!active || active.notebookId !== notebookId) {
      return false;
    }
    if (get().loadedNotebookId === notebookId) set({ generationPhase: "cancelled" });
    return api.cancelStudioGeneration(notebookId, active.attemptId);
  },

  exportOutput: async (notebookId, outputId) => {
    if (get().loadedNotebookId !== notebookId) return null;
    set({ status: "exporting", error: null });
    try {
      const receipt = await api.exportStudioOutput(notebookId, outputId);
      const outputs = await api.listStudioOutputs(notebookId);
      if (get().loadedNotebookId !== notebookId) return receipt;
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
