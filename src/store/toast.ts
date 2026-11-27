import { create } from 'zustand';

// ponytail: toast store replaces the useState toast array + the showToast prop
// that was drilled into 5 components. Any component can now call useToast to
// surface a message without prop drilling.

export type ToastKind = 'success' | 'error' | 'info';

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  kind: ToastKind;
  msg: string;
  action?: ToastAction;
  /** How many identical messages were folded into this toast (1 = first). */
  count: number;
}

interface ToastState {
  toasts: Toast[];
  showToast: (kind: ToastKind, msg: string, action?: ToastAction) => void;
  dismiss: (id: number) => void;
}

const TOAST_MS = 3500;

// Timer handles live outside the store so a duplicate can re-arm the
// auto-dismiss of the toast it folds into.
const timers = new Map<number, ReturnType<typeof setTimeout>>();

export const useToastStore = create<ToastState>((set, get) => {
  const remove = (id: number) => {
    clearTimeout(timers.get(id));
    timers.delete(id);
    set((prev) => ({ toasts: prev.toasts.filter((t) => t.id !== id) }));
  };
  const arm = (id: number) => {
    clearTimeout(timers.get(id));
    timers.set(
      id,
      setTimeout(() => remove(id), TOAST_MS),
    );
  };
  return {
    toasts: [],
    showToast: (kind, msg, action) => {
      // Burst dedupe: an identical visible toast absorbs the repeat instead of
      // stacking (8 identical errors = 1 toast); re-arming keeps it up while
      // the errors are still firing.
      const dupe = get().toasts.find((t) => t.kind === kind && t.msg === msg);
      if (dupe) {
        set((prev) => ({
          toasts: prev.toasts.map((t) => (t.id === dupe.id ? { ...t, count: t.count + 1 } : t)),
        }));
        arm(dupe.id);
        return;
      }
      const id = Date.now() + Math.random();
      set((prev) => ({
        toasts: [...prev.toasts, { id, kind, msg, count: 1, ...(action ? { action } : {}) }],
      }));
      arm(id);
    },
    dismiss: (id) => remove(id),
  };
});

// Convenience hook for components that only need the push function.
export const useToast = () => useToastStore((s) => s.showToast);
