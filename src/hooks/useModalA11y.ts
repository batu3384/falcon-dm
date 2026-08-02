import { useEffect, type RefObject } from 'react';

const FOCUSABLE =
  'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

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
      Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
        (el) => !el.hasAttribute('disabled') && el.offsetParent !== null,
      );

    const prev = document.activeElement as HTMLElement | null;
    const first = list()[0];
    first?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
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
