import { create } from "zustand";
import * as api from "../lib/tauri";
import type { MemoryBackendStatus, QueueStatus, SemanticMemoryProfileStatus } from "../lib/types";

interface HealthStore {
  notebookId: string | null;
  providerId: string | null;
  backgroundProviderId: string | null;
  chatConnected: boolean | null;
  backgroundConnected: boolean | null;
  queueStatus: QueueStatus | null;
  memoryStatus: MemoryBackendStatus | null;
  profileStatus: SemanticMemoryProfileStatus | null;
  startPolling: (notebookId: string | null, providerId: string | null) => void;
  stopPolling: () => void;
  poll: () => Promise<void>;
}

let interval: ReturnType<typeof setInterval> | null = null;
let pollKey = "";
let epoch = 0;
let inFlight: { epoch: number; promise: Promise<void> } | null = null;

export const useHealthStore = create<HealthStore>((set, get) => ({
  notebookId: null, providerId: null, backgroundProviderId: null, chatConnected: null, backgroundConnected: null,
  queueStatus: null, memoryStatus: null, profileStatus: null,
  poll: () => {
    if (inFlight?.epoch === epoch) return inFlight.promise;
    const requestEpoch = epoch;
    const { notebookId, providerId } = get();
    const promise = (async () => {
      const providerCheck = providerId ? api.testProvider(providerId).catch(() => false) : Promise.resolve(null);
      const [queueStatus, memoryStatus, profileStatus, chatConnected] = await Promise.all([
        api.getQueueStatus().catch(() => null),
        api.memoryBackendStatus(notebookId).catch(() => null),
        notebookId ? api.getSemanticMemoryProfileStatus(notebookId, { kind: "all" }).catch(() => null) : Promise.resolve(null),
        providerCheck,
      ]);
      if (requestEpoch !== epoch) return;
      // The queue owns its configured summary provider. Never infer it from the
      // foreground model registry or retain health from an earlier queue identity.
      const backgroundProviderId = queueStatus?.summary_backend.provider_id ?? null;
      const backgroundConnected = backgroundProviderId
        ? backgroundProviderId === providerId
          ? chatConnected
          : await api.testProvider(backgroundProviderId).catch(() => false)
        : null;
      if (requestEpoch !== epoch) return;
      set({ queueStatus, memoryStatus, profileStatus, chatConnected, backgroundProviderId, backgroundConnected });
    })();
    inFlight = { epoch: requestEpoch, promise };
    return promise.finally(() => {
      if (inFlight?.promise === promise) inFlight = null;
    });
  },
  startPolling: (notebookId, providerId) => {
    const nextKey = JSON.stringify([notebookId, providerId]);
    if (pollKey === nextKey && interval !== null) return;
    pollKey = nextKey;
    ++epoch;
    if (interval !== null) clearInterval(interval);
    set({ notebookId, providerId, backgroundProviderId: null, chatConnected: null, backgroundConnected: null,
      queueStatus: null, memoryStatus: null, profileStatus: null });
    void get().poll();
    interval = setInterval(() => void get().poll(), 10000);
  },
  stopPolling: () => { if (interval !== null) clearInterval(interval); interval = null; pollKey = ""; ++epoch; inFlight = null; },
}));
