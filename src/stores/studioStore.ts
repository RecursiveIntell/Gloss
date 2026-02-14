import { create } from 'zustand';
import type { StudioOutput } from '../lib/types';

interface StudioStore {
  outputs: StudioOutput[];
  activeOutputType: string | null;
  activeOutputId: string | null;
  setActiveOutputType: (type: string | null) => void;
  setActiveOutputId: (id: string | null) => void;
}

// Studio is Phase 2+ — minimal store for now
export const useStudioStore = create<StudioStore>((set) => ({
  outputs: [],
  activeOutputType: null,
  activeOutputId: null,
  setActiveOutputType: (type) => set({ activeOutputType: type }),
  setActiveOutputId: (id) => set({ activeOutputId: id }),
}));
