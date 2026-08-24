import { create } from 'zustand';
import type { Provider, ModelRecord, FeatureFlagStatus, ExternalToolAvailabilityReceipt } from '../lib/types';
import * as api from '../lib/tauri';
import { useToastStore } from './toastStore';

interface SettingsStore {
  providers: Provider[];
  models: ModelRecord[];
  settings: Record<string, string>;
  featureFlags: FeatureFlagStatus[];
  activeModel: string;
  selectionError: string | null;
  loading: boolean;
  externalTools: Record<string, ExternalToolAvailabilityReceipt>;
  loadSettings: () => Promise<void>;
  loadFeatureFlags: () => Promise<void>;
  loadProviders: () => Promise<void>;
  loadModels: () => Promise<void>;
  refreshModels: () => Promise<void>;
  updateSetting: (key: string, value: string) => Promise<void>;
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
  settings: {},
  featureFlags: [],
  activeModel: '',
  selectionError: null,
  loading: false,
  externalTools: {},

  loadSettings: async () => {
    try {
      const settings = await api.getSettings();
      const defaultModel = settings['default_model']?.trim() ?? '';
      set({
        settings,
        activeModel: defaultModel,
        selectionError: defaultModel
          ? null
          : 'No default model is configured. Select an available model in Settings.',
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
      const activeModel = get().activeModel;
      const selectedProviderId = get().settings['default_provider'];
      const selectedModel = activeModel
        ? models.find(
            (model) =>
              model.id === activeModel &&
              (!selectedProviderId || model.provider_id === selectedProviderId)
          ) ?? models.find((model) => model.id === activeModel)
        : undefined;
      const selectionError = activeModel && (!selectedModel || !selectedModel.available || selectedModel.stale)
        ? `Selected model '${activeModel}' is unavailable. Refresh models or select an available model.`
        : null;
      set({ models, selectionError });
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

  updateSetting: async (key, value) => {
    // Snapshot the prior value so we can roll back on failure.
    const prior = get().settings[key];
    const priorConfigured = get().settings[`${key}_configured`];
    // Optimistic local commit so the UI is responsive while the IPC is in
    // flight. On failure we restore the prior value below.
    set((state) => {
      const nextSettings = { ...state.settings };
      if (key === 'openai_api_key' || key === 'anthropic_api_key') {
        nextSettings[key] = '';
        nextSettings[`${key}_configured`] = value.trim() ? '1' : '0';
      } else {
        nextSettings[key] = value;
      }
      return { settings: nextSettings };
    });
    try {
      await api.updateSetting(key, value);
    } catch (err) {
      console.warn("Failed to update setting:", key, err);
      // Restore the prior value so the UI does not show a phantom setting
      // that was never persisted. Reload will overwrite with the authoritative
      // value on next load.
      set((state) => {
        const nextSettings = { ...state.settings };
        if (prior === undefined) {
          delete nextSettings[key];
        } else {
          nextSettings[key] = prior;
        }
        if (key === 'openai_api_key' || key === 'anthropic_api_key') {
          if (priorConfigured === undefined) {
            delete nextSettings[`${key}_configured`];
          } else {
            nextSettings[`${key}_configured`] = priorConfigured;
          }
        }
        return { settings: nextSettings };
      });
      useToastStore.getState().addToast({
        type: 'error',
        title: 'Setting not saved',
        message: `${key}: ${err instanceof Error ? err.message : String(err)}`,
        duration: 5000,
      });
      throw err;
    }
    if (key === 'memory_backend') {
      await get().loadFeatureFlags();
    }
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

  setActiveModel: (model) => set({ activeModel: model }),

  selectModel: async (providerId, modelId) => {
    const model = get().models.find((candidate) => candidate.provider_id === providerId && candidate.id === modelId);
    if (!model) {
      const message = `Model ${modelId} is not registered for provider ${providerId}`;
      set({ selectionError: message });
      useToastStore.getState().addToast({ type: 'error', title: 'Invalid model selection', message, duration: 5000 });
      throw new Error(message);
    }
    // Commit the pair together so no render can observe a provider/model mix.
    set((state) => ({ activeModel: modelId, selectionError: null, settings: { ...state.settings, default_model: modelId, default_provider: providerId } }));
    try {
      await Promise.all([api.updateSetting('default_model', modelId), api.updateSetting('default_provider', providerId)]);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      set({ selectionError: message });
      useToastStore.getState().addToast({ type: 'error', title: 'Model selection not saved', message, duration: 5000 });
      throw error;
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
