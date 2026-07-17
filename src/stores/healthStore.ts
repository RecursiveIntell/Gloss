import { create } from "zustand";
import * as api from "../lib/tauri";
import type { MemoryBackendStatus, QueueStatus, SemanticMemoryProfileStatus } from "../lib/types";

interface HealthStore {
  notebookId: string | null;
  providerId: string | null;
  chatConnected: boolean | null;
  backgroundConnected: boolean;
  queueStatus: QueueStatus | null;
  memoryStatus: MemoryBackendStatus | null;
  profileStatus: SemanticMemoryProfileStatus | null;
  startPolling: (notebookId: string | null, providerId: string | null, backgroundProviderId?: string | null) => void;
  stopPolling: () => void;
  poll: () => Promise<void>;
}

let interval: ReturnType<typeof setInterval> | null = null;
let pollKey = "";
let epoch = 0;

export const useHealthStore = create<HealthStore>((set, get) => ({
  notebookId: null, providerId: null, chatConnected: null, backgroundConnected: false,
  queueStatus: null, memoryStatus: null, profileStatus: null,
  poll: async () => {
    const requestEpoch = ++epoch;
    const { notebookId, providerId } = get();
    const [queueStatus, memoryStatus, profileStatus, chatConnected] = await Promise.all([
      api.getQueueStatus().catch(() => null),
      api.memoryBackendStatus(notebookId).catch(() => null),
      notebookId ? api.getSemanticMemoryProfileStatus(notebookId, { kind: "all" }).catch(() => null) : Promise.resolve(null),
      providerId ? api.testProvider(providerId).catch(() => false) : Promise.resolve(false),
    ]);
    if (requestEpoch !== epoch) return;
    set({ queueStatus, memoryStatus, profileStatus, chatConnected });
  },
  startPolling: (notebookId, providerId, backgroundProviderId = null) => {
    const nextKey = `${notebookId ?? ""}:${providerId ?? ""}:${backgroundProviderId ?? ""}`;
    set({ notebookId, providerId });
    if (pollKey === nextKey && interval !== null) return;
    pollKey = nextKey;
    if (interval !== null) clearInterval(interval);
    void get().poll();
    interval = setInterval(() => void get().poll(), 10000);
  },
  stopPolling: () => { if (interval !== null) clearInterval(interval); interval = null; pollKey = ""; ++epoch; },
}));
