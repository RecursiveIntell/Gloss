import { create } from 'zustand';
import type { Provider, ModelRecord, FeatureFlagStatus, ExternalToolAvailabilityReceipt } from '../lib/types';
import * as api from '../lib/tauri';
import { useToastStore } from './toastStore';

let settingsWriteTail: Promise<void> = Promise.resolve();

export function findSelectedModel(models: ModelRecord[], providerId: string | null | undefined, modelId: string): ModelRecord | undefined {
  if (!providerId || !modelId) return undefined;
  return models.find(model => model.id === modelId && model.provider_id === providerId);
}

function selectionReadiness(settings: Record<string, string>, models: ModelRecord[], modelsLoaded: boolean): string | null {
  const modelId = settings.default_model?.trim();
  if (!modelId) return 'No default model is configured. Select an available model in Settings.';
  if (!modelsLoaded) return null;
  const selected = findSelectedModel(models, settings.default_provider, modelId);
  return !selected || !selected.available || selected.stale
    ? `Selected model '${modelId}' is unavailable. Refresh models or select an available model.`
    : null;
}

interface SettingsStore {
  providers: Provider[];
  models: ModelRecord[];
  modelsLoaded: boolean;
  settings: Record<string, string>;
  featureFlags: FeatureFlagStatus[];
  activeModel: string;
  selectionError: string | null;
  selectionPending: boolean;
  loading: boolean;
  externalTools: Record<string, ExternalToolAvailabilityReceipt>;
  loadSettings: () => Promise<void>;
  loadFeatureFlags: () => Promise<void>;
  loadProviders: () => Promise<void>;
  loadModels: () => Promise<void>;
  refreshModels: () => Promise<void>;
  updateSetting: (key: string, value: string) => Promise<void>;
  applyEmbeddingSettings: (config: api.EmbeddingSettings) => Promise<string[]>;
  updateFeatureFlag: (id: string, enabled: boolean) => Promise<void>;
  updateProvider: (id: string, enabled: boolean, baseUrl?: string, apiKey?: string) => Promise<void>;
  setActiveModel: (model: string) => void;
  selectModel: (providerId: string, modelId: string) => Promise<void>;
  testProvider: (providerId: string) => Promise<boolean>;
  loadExternalTools: () => Promise<void>;
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  providers: [],
  models: [],
  modelsLoaded: false,
  settings: {},
  featureFlags: [],
  activeModel: '',
  selectionError: null,
  selectionPending: false,
  loading: false,
  externalTools: {},

  loadSettings: async () => {
    const priorSettings = get().settings;
    try {
      const settings = await api.getSettings();
      if (get().selectionPending || get().settings !== priorSettings) return;
      const defaultModel = settings['default_model']?.trim() ?? '';
      set({
        settings,
        activeModel: defaultModel,
        selectionError: selectionReadiness(settings, get().models, get().modelsLoaded),
      });
    } catch (e) {
      console.warn('Failed to load settings:', e);
      useToastStore.getState().addToast({ type: 'error', title: 'Load Failed', message: 'Failed to load settings', duration: 5000 });
    }
  },

  loadFeatureFlags: async () => {
    try {
      const featureFlags = await api.getFeatureFlags();
      set({ featureFlags });
    } catch (e) {
      console.warn('Failed to load feature flags:', e);
      useToastStore.getState().addToast({ type: 'error', title: 'Load Failed', message: 'Failed to load feature flags', duration: 5000 });
    }
  },

  loadProviders: async () => {
    try {
      const providers = await api.getProviders();
      set({ providers });
    } catch (e) {
      console.warn('Failed to load providers:', e);
      useToastStore.getState().addToast({ type: 'error', title: 'Load Failed', message: 'Failed to load providers', duration: 5000 });
    }
  },

  loadModels: async () => {
    try {
      const models = await api.getAllModels();
      const selectionError = selectionReadiness({ ...get().settings, default_model: get().activeModel }, models, true);
      set({ models, modelsLoaded: true, selectionError });
    } catch (e) {
      console.warn('Failed to load models:', e);
      useToastStore.getState().addToast({ type: 'error', title: 'Load Failed', message: 'Failed to load models', duration: 5000 });
    }
  },

  refreshModels: async () => {
    set({ loading: true });
    try {
      await api.refreshModels();
      await get().loadModels();
    } catch (e) {
      console.warn('Failed to refresh models:', e);
      useToastStore.getState().addToast({ type: 'error', title: 'Refresh Failed', message: 'Failed to refresh models', duration: 5000 });
    } finally {
      set({ loading: false });
    }
  },

  updateSetting: (key, value) => {
    // Keep canonical write order. Values enter the UI only after persistence;
    // an older rejection can never roll back a later acknowledged setting.
    const operation = settingsWriteTail.then(async () => {
      try {
        await api.updateSetting(key, value);
        set((state) => {
          const settings = { ...state.settings };
          if (key === 'openai_api_key' || key === 'anthropic_api_key') {
            settings[key] = '';
            settings[key + '_configured'] = value.trim() ? '1' : '0';
          } else { settings[key] = value; }
          return { settings };
        });
        if (key === 'memory_backend') await get().loadFeatureFlags();
      } catch (err) {
        useToastStore.getState().addToast({ type: 'error', title: 'Setting not saved', message: key + ': ' + String(err), duration: 5000 });
        throw err;
      }
    });
    settingsWriteTail = operation.catch(() => {});
    return operation;
  },
  updateFeatureFlag: async (id, enabled) => {
    try {
      const featureFlags = await api.updateFeatureFlag(id, enabled);
      set({ featureFlags });
      await get().loadSettings();
    } catch (err) {
      console.warn("Failed to update feature flag:", id, err);
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Feature flag not saved',
        message: `${id}: ${err instanceof Error ? err.message : String(err)}`,
        duration: 5000,
      });
      throw err;
    }
  },

  updateProvider: async (id, enabled, baseUrl, apiKey) => {
    try {
      await api.updateProvider(id, enabled, baseUrl, apiKey);
      await get().loadProviders();
    } catch (err) {
      console.warn("Failed to update provider:", id, err);
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Provider not saved',
        message: `${id}: ${err instanceof Error ? err.message : String(err)}`,
        duration: 5000,
      });
      throw err;
    }
  },

  applyEmbeddingSettings: (config) => {
    const operation = settingsWriteTail.then(async () => {
      const warnings = await api.updateEmbeddingSettings(config);
      set((state) => ({ settings: { ...state.settings,
        semantic_memory_embedding_provider: config.provider,
        semantic_memory_embedding_url: config.url,
        semantic_memory_embedding_model: config.model,
        semantic_memory_embedding_timeout_secs: String(config.timeout_secs),
        fastembed_download_consent: String(config.download_consent),
        semantic_memory_search_timeout_ms: String(config.search_timeout_ms),
        chunk_target_tokens: String(config.chunk_target_tokens),
      } }));
      return warnings;
    });
    settingsWriteTail = operation.then(() => {}, () => {});
    return operation;
  },

  setActiveModel: (model) => set({ activeModel: model }),

  selectModel: async (providerId, modelId) => {
    if (get().selectionPending) throw new Error('Model selection is already being saved.');
    const model = get().models.find((candidate) => candidate.provider_id === providerId && candidate.id === modelId);
    if (!model || !model.available || model.stale) {
      const message = `Model ${modelId} is not registered for provider ${providerId}`;
      set({ selectionError: message });
      useToastStore.getState().addToast({ type: 'error', title: 'Invalid model selection', message, duration: 5000 });
      throw new Error(message);
    }
    set({ selectionPending: true });
    try {
      await api.selectChatModel(providerId, modelId);
      // Project the pair only after the canonical DB transaction acknowledges it.
      set((state) => ({ activeModel: modelId, selectionError: null, settings: { ...state.settings, default_model: modelId, default_provider: providerId } }));
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      set({ selectionError: message });
      useToastStore.getState().addToast({ type: 'error', title: 'Model selection not saved', message, duration: 5000 });
      throw error;
    } finally {
      set({ selectionPending: false });
    }
  },

  testProvider: async (providerId) => {
    try {
      return await api.testProvider(providerId);
    } catch (err) {
      console.warn("testProvider failed:", err);
      return false;
    }
  },

  loadExternalTools: async () => {
    try {
      const tools = await api.checkExternalTools();
      set({ externalTools: tools });
    } catch (err) {
      console.warn("loadExternalTools failed:", err);
      // Non-critical — default to empty
    }
  },
}));
