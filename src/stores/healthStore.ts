import { create } from "zustand";
import * as api from "../lib/tauri";
import type { MemoryBackendStatus, QueueStatus, SemanticMemoryProfileStatus } from "../lib/types";

export type MemoryHealthState = "checking" | "ready" | "unavailable";

export function memoryHealthError(error: unknown): string {
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "Memory status read failed";
  return message.trim().slice(0, 500) || "Memory status read failed";
}

interface HealthStore {
  notebookId: string | null;
  providerId: string | null;
  backgroundProviderId: string | null;
  chatConnected: boolean | null;
  backgroundConnected: boolean | null;
  queueStatus: QueueStatus | null;
  memoryStatus: MemoryBackendStatus | null;
  memoryState: MemoryHealthState;
  memoryError: string | null;
  profileStatus: SemanticMemoryProfileStatus | null;
  startPolling: (notebookId: string | null, providerId: string | null) => void;
  stopPolling: () => void;
  poll: () => Promise<void>;
}

let interval: ReturnType<typeof setInterval> | null = null;
let pollKey = "";
let epoch = 0;
// Coalesce each probe independently: a slow provider cannot hold local evidence
// or prevent the next local refresh. Pending requests are never superseded by a tick.
const probes = new Map<string, { epoch: number; promise: Promise<void> }>();
const providers = new Map<string, { epoch: number; promise: Promise<boolean> }>();

function probe(key: string, requestEpoch: number, read: () => Promise<void>): Promise<void> {
  const pending = probes.get(key);
  if (pending?.epoch === requestEpoch) return pending.promise;
  const promise = read().finally(() => {
    if (probes.get(key)?.promise === promise) probes.delete(key);
  });
  probes.set(key, { epoch: requestEpoch, promise });
  return promise;
}

function checkProvider(id: string, requestEpoch: number): Promise<boolean> {
  const pending = providers.get(id);
  if (pending?.epoch === requestEpoch) return pending.promise;
  const promise = api.testProvider(id).catch(() => false).finally(() => {
    if (providers.get(id)?.promise === promise) providers.delete(id);
  });
  providers.set(id, { epoch: requestEpoch, promise });
  return promise;
}

export const useHealthStore = create<HealthStore>((set, get) => ({
  notebookId: null, providerId: null, backgroundProviderId: null, chatConnected: null, backgroundConnected: null,
  queueStatus: null, memoryStatus: null, memoryState: "checking", memoryError: null, profileStatus: null,
  poll: async () => {
    const requestEpoch = epoch;
    const { notebookId, providerId } = get();
    const providerCheck = providerId ? checkProvider(providerId, requestEpoch) : Promise.resolve(null);
    const chatCheck = providerCheck.then(chatConnected => {
      if (requestEpoch === epoch) set({ chatConnected });
    });
    const memoryCheck = probe("memory", requestEpoch, async () => {
      set({ memoryStatus: null, memoryState: "checking", memoryError: null });
      try {
        const memoryStatus = await api.memoryBackendStatus(notebookId);
        if (requestEpoch === epoch) set({ memoryStatus, memoryState: "ready", memoryError: null });
      } catch (error) {
        if (requestEpoch === epoch) set({ memoryStatus: null, memoryState: "unavailable", memoryError: memoryHealthError(error) });
      }
    });
    const profileCheck = probe("profile", requestEpoch, async () => {
      const profileStatus = notebookId
        ? await api.getSemanticMemoryProfileStatus(notebookId, { kind: "all" }).catch(() => null)
        : null;
      if (requestEpoch === epoch) set({ profileStatus });
    });
    const queueCheck = probe("queue", requestEpoch, async () => {
      const queueStatus = await api.getQueueStatus().catch(() => null);
      if (requestEpoch !== epoch) return;
      // The queue owns its configured summary provider; foreground selection
      // cannot supply a missing background identity.
      const backgroundProviderId = queueStatus?.summary_backend.provider_id ?? null;
      set({ queueStatus, backgroundProviderId,
        ...(backgroundProviderId !== get().backgroundProviderId || !backgroundProviderId ? { backgroundConnected: null } : {}),
      });
    });
    const backgroundCheck = queueCheck.then(async () => {
      if (requestEpoch !== epoch) return;
      const { backgroundProviderId } = get();
      const backgroundConnected = backgroundProviderId
        ? await (backgroundProviderId === providerId ? providerCheck : checkProvider(backgroundProviderId, requestEpoch))
        : null;
      if (requestEpoch === epoch && backgroundProviderId === get().backgroundProviderId) set({ backgroundConnected });
    });
    await Promise.all([memoryCheck, profileCheck, queueCheck, chatCheck, backgroundCheck]);
  },
  startPolling: (notebookId, providerId) => {
    const nextKey = JSON.stringify([notebookId, providerId]);
    if (pollKey === nextKey && interval !== null) return;
    pollKey = nextKey;
    ++epoch;
    if (interval !== null) clearInterval(interval);
    set({ notebookId, providerId, backgroundProviderId: null, chatConnected: null, backgroundConnected: null,
      queueStatus: null, memoryStatus: null, memoryState: "checking", memoryError: null, profileStatus: null });
    void get().poll();
    interval = setInterval(() => void get().poll(), 10000);
  },
  stopPolling: () => {
    if (interval !== null) clearInterval(interval);
    interval = null; pollKey = ""; ++epoch; probes.clear(); providers.clear();
  },
}));
