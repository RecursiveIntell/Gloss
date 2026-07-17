import { create } from 'zustand';

export type ThemeMode = 'dark' | 'light';

interface UiStore {
  commandPaletteOpen: boolean;
  theme: ThemeMode;
  setCommandPaletteOpen: (open: boolean) => void;
  toggleCommandPaletteOpen: () => void;
  setTheme: (theme: ThemeMode) => void;
  toggleTheme: () => void;
}

const THEME_KEY = 'gloss:theme';

function readTheme(): ThemeMode {
  try {
    const saved = localStorage.getItem(THEME_KEY);
    return saved === 'light' ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

export const useUiStore = create<UiStore>((set, get) => ({
  commandPaletteOpen: false,
  theme: readTheme(),

  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),

  toggleCommandPaletteOpen: () => set({ commandPaletteOpen: !get().commandPaletteOpen }),

  setTheme: (theme) => {
    try {
      localStorage.setItem(THEME_KEY, theme);
    } catch {
      // localStorage can fail in restricted environments; UI still works with memory state.
    }
    set({ theme });
  },

  toggleTheme: () => {
    const next = get().theme === 'dark' ? 'light' : 'dark';
    get().setTheme(next);
  },
}));
