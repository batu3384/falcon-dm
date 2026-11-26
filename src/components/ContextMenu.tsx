import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { contextMenuNavIndex } from '../lib/menuNav';

export type MenuEntry =
  | {
      id: string;
      label: string;
      icon?: ReactNode;
      danger?: boolean;
      disabled?: boolean;
      run: () => void;
    }
  | { id: string; separator: true };

interface ContextMenuProps {
  x: number;
  y: number;
  label: string;
  entries: MenuEntry[];
  onClose: () => void;
}

export function ContextMenu({ x, y, label, entries, onClose }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const pad = 8;
    const rect = el.getBoundingClientRect();
    setPos({
      left: Math.max(pad, Math.min(x, window.innerWidth - rect.width - pad)),
      top: Math.max(pad, Math.min(y, window.innerHeight - rect.height - pad)),
    });
  }, [x, y]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const restore = document.activeElement as HTMLElement | null;
    const items = () => Array.from(el.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
    items()[0]?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || e.key === 'Tab') {
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      }
      const list = items();
      const next = contextMenuNavIndex(
        list.length,
        list.findIndex((b) => b === document.activeElement),
        e.key,
      );
      if (next === null) return;
      e.preventDefault();
      list[next]?.focus();
    };
    const onPointer = (e: MouseEvent) => {
      if (!el.contains(e.target as Node)) onClose();
    };
    el.addEventListener('keydown', onKey);
    // The opening right-click may still be dispatching; listen from the next turn.
    const timer = window.setTimeout(() => {
      window.addEventListener('mousedown', onPointer);
      window.addEventListener('blur', onClose);
      window.addEventListener('resize', onClose);
    }, 0);
    return () => {
      window.clearTimeout(timer);
      el.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onPointer);
      window.removeEventListener('blur', onClose);
      window.removeEventListener('resize', onClose);
      if (restore?.isConnected) restore.focus();
    };
  }, [onClose]);

  return createPortal(
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      tabIndex={-1}
      style={{ left: pos.left, top: pos.top }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {entries.map((entry) =>
        'separator' in entry ? (
          <div key={entry.id} className="menu-sep" role="separator" />
        ) : (
          <button
            key={entry.id}
            type="button"
            role="menuitem"
            className={`menu-item${entry.danger ? ' danger' : ''}`}
            disabled={entry.disabled}
            onClick={() => {
              onClose();
              entry.run();
            }}
          >
            {entry.icon}
            {entry.label}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
