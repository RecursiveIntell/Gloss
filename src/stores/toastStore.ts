import { create } from 'zustand';

export interface Toast {
  id: string;
  type: 'error' | 'success' | 'info' | 'warning';
  title: string;
  message: string;
  duration: number; // ms. 0 = sticky (manual dismiss only).
}

interface ToastStore {
  toasts: Toast[];
  addToast: (toast: Omit<Toast, 'id'>) => void;
  removeToast: (id: string) => void;
}

const MAX_TOASTS = 5;
let nextId = 0;

export const useToastStore = create<ToastStore>((set) => ({
  toasts: [],

  addToast: (toast) => {
    const id = `toast-${++nextId}`;
    const full: Toast = { ...toast, id };
    set((state) => ({
      toasts: [...state.toasts, full].slice(-MAX_TOASTS),
    }));

    if (toast.duration > 0) {
      setTimeout(() => {
        set((state) => ({
          toasts: state.toasts.filter((t) => t.id !== id),
        }));
      }, toast.duration);
    }
  },

  removeToast: (id) => {
    set((state) => ({
      toasts: state.toasts.filter((t) => t.id !== id),
    }));
  },
}));
