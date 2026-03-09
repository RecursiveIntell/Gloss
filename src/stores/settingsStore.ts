import { create } from 'zustand';
import type { Provider, ModelRecord } from '../lib/types';
import * as api from '../lib/tauri';

interface SettingsStore {
  providers: Provider[];
  models: ModelRecord[];
  settings: Record<string, string>;
  activeModel: string;
  loading: boolean;
  externalTools: Record<string, boolean>;
  loadSettings: () => Promise<void>;
  loadProviders: () => Promise<void>;
  loadModels: () => Promise<void>;
  refreshModels: () => Promise<void>;
  updateSetting: (key: string, value: string) => Promise<void>;
  updateProvider: (id: string, enabled: boolean, baseUrl?: string, apiKey?: string) => Promise<void>;
  setActiveModel: (model: string) => void;
  testProvider: (providerId: string) => Promise<boolean>;
  loadExternalTools: () => Promise<void>;
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  providers: [],
  models: [],
  settings: {},
  activeModel: 'qwen3:8b',
  loading: false,
  externalTools: {},

  loadSettings: async () => {
    try {
      const settings = await api.getSettings();
      set({
        settings,
        activeModel: settings['default_model'] || 'qwen3:8b',
      });
    } catch (e) {
      console.error('Failed to load settings:', e);
    }
  },

  loadProviders: async () => {
    try {
      const providers = await api.getProviders();
      set({ providers });
    } catch (e) {
      console.error('Failed to load providers:', e);
    }
  },

  loadModels: async () => {
    try {
      const models = await api.getAllModels();
      set({ models });
    } catch (e) {
      console.error('Failed to load models:', e);
    }
  },

  refreshModels: async () => {
    set({ loading: true });
    try {
      await api.refreshModels();
      await get().loadModels();
    } catch (e) {
      console.error('Failed to refresh models:', e);
    } finally {
      set({ loading: false });
    }
  },

  updateSetting: async (key, value) => {
    await api.updateSetting(key, value);
    set((state) => ({
      settings: { ...state.settings, [key]: value },
    }));
  },

  updateProvider: async (id, enabled, baseUrl, apiKey) => {
    await api.updateProvider(id, enabled, baseUrl, apiKey);
    await get().loadProviders();
  },

  setActiveModel: (model) => set({ activeModel: model }),

  testProvider: async (providerId) => {
    try {
      return await api.testProvider(providerId);
    } catch {
      return false;
    }
  },

  loadExternalTools: async () => {
    try {
      const tools = await api.checkExternalTools();
      set({ externalTools: tools });
    } catch {
      // Non-critical — default to empty
    }
  },
}));
