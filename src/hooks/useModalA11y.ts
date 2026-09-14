import { useEffect, type RefObject } from 'react';

const FOCUSABLE =
  'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

export function isModalFocusable(el: HTMLElement): boolean {
  if (
    el.hasAttribute('disabled') ||
    el.hasAttribute('hidden') ||
    el.getAttribute('aria-hidden') === 'true'
  ) {
    return false;
  }
  if (el.closest('[hidden]')) return false;
  const style = getComputedStyle(el);
  return style.display !== 'none' && style.visibility !== 'hidden';
}

/** Onboarding or a modal is up — don't open another sheet (⌘K / drop / clipboard). */
export function isForegroundDialogOpen(opts?: { ignorePalette?: boolean }): boolean {
  if (document.querySelector('.wizard-overlay')) return true;
  const sel = opts?.ignorePalette ? '.modal-overlay:not(.palette-overlay)' : '.modal-overlay';
  return !!document.querySelector(sel);
}

/** Forms want the field; confirms want Cancel. Prefer explicit marker. */
export function pickModalInitialFocus(items: HTMLElement[]): HTMLElement | undefined {
  return (
    items.find((el) => el.hasAttribute('data-modal-initial-focus')) ||
    items.find((el) => el.hasAttribute('autofocus')) ||
    items.find((el) => el.hasAttribute('data-modal-cancel')) ||
    items[0]
  );
}

/** Escape closes; Tab cycles inside modal; restores focus on unmount. */
export function useModalA11y(
  containerRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  enabled = true,
) {
  useEffect(() => {
    if (!enabled) return;
    const root = containerRef.current;
    if (!root) return;

    const list = () =>
      Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(isModalFocusable);

    const prev = document.activeElement as HTMLElement | null;
    const first = pickModalInitialFocus(list());
    first?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== 'Tab') return;
      const items = list();
      if (!items.length) return;
      const head = items[0];
      const tail = items[items.length - 1];
      if (e.shiftKey && document.activeElement === head) {
        e.preventDefault();
        tail.focus();
      } else if (!e.shiftKey && document.activeElement === tail) {
        e.preventDefault();
        head.focus();
      }
    };

    root.addEventListener('keydown', onKey);
    return () => {
      root.removeEventListener('keydown', onKey);
      prev?.focus?.();
    };
  }, [containerRef, onClose, enabled]);
}
