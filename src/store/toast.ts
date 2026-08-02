import { create } from 'zustand';

// ponytail: toast store replaces the useState toast array + the showToast prop
// that was drilled into 5 components. Any component can now call useToast to
// surface a message without prop drilling.

export type ToastKind = 'success' | 'error' | 'info';

export interface Toast {
  id: number;
  kind: ToastKind;
  msg: string;
}

interface ToastState {
  toasts: Toast[];
  showToast: (kind: ToastKind, msg: string) => void;
  dismiss: (id: number) => void;
}

export const useToastStore = create<ToastState>((set) => ({
  toasts: [],
  showToast: (kind, msg) => {
    const id = Date.now() + Math.random();
    set((prev) => ({ toasts: [...prev.toasts, { id, kind, msg }] }));
    setTimeout(() => {
      set((prev) => ({ toasts: prev.toasts.filter((t) => t.id !== id) }));
    }, 3500);
  },
  dismiss: (id) => set((prev) => ({ toasts: prev.toasts.filter((t) => t.id !== id) })),
}));

// Convenience hook for components that only need the push function.
export const useToast = () => useToastStore((s) => s.showToast);
