import { useState, useRef, useEffect, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Search } from 'lucide-react';
import { useModalA11y } from '../hooks/useModalA11y';

export interface PaletteAction {
  id: string;
  label: string;
  icon: React.ComponentType<{ size?: number; strokeWidth?: number; 'aria-hidden'?: boolean }>;
  run: () => void | Promise<void>;
  keywords?: string;
}

interface CommandPaletteProps {
  onClose: () => void;
  actions: PaletteAction[];
  onError?: (error: unknown) => void;
}

export function CommandPalette({ onClose, actions, onError }: CommandPaletteProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const [running, setRunning] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  useModalA11y(panelRef, onClose);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return actions;
    return actions.filter(
      (a) => a.label.toLowerCase().includes(q) || (a.keywords?.toLowerCase().includes(q) ?? false),
    );
  }, [query, actions]);

  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  const runAction = async (action: PaletteAction | undefined) => {
    if (!action || running) return;
    setRunning(true);
    // Close first so focus restore cannot steal the next dialog's initial focus.
    onClose();
    try {
      await action.run();
    } catch (error) {
      onError?.(error);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (filtered.length === 0) return;
      setActiveIndex((i) => Math.min(i + 1, filtered.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (filtered.length === 0) return;
      setActiveIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (filtered.length === 0) return;
      void runAction(filtered[activeIndex]);
    } else if (e.key === 'Home') {
      e.preventDefault();
      if (filtered.length === 0) return;
      setActiveIndex(0);
    } else if (e.key === 'End') {
      e.preventDefault();
      if (filtered.length === 0) return;
      setActiveIndex(Math.max(0, filtered.length - 1));
    }
  };

  return (
    <div className="modal-overlay palette-overlay" onClick={onClose} role="presentation">
      <div
        ref={panelRef}
        className="modal-panel modal-md palette-panel"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="palette-title"
      >
        <div className="modal-head">
          <div className="search-wrap palette-search">
            <Search size={16} aria-hidden={true} />
            <input
              ref={inputRef}
              className="search-input"
              placeholder={t('commandPalette.placeholder')}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onKeyDown}
              // ponytail: combobox pattern — input controls a listbox of options.
              role="combobox"
              aria-haspopup="listbox"
              aria-expanded={true}
              aria-controls="palette-listbox"
              aria-autocomplete="list"
              aria-activedescendant={
                filtered[activeIndex] ? `palette-opt-${filtered[activeIndex].id}` : undefined
              }
              aria-label={t('commandPalette.placeholder')}
              data-modal-initial-focus
            />
            <kbd className="kbd">Esc</kbd>
          </div>
        </div>
        <div
          className="modal-body palette-body"
          id="palette-listbox"
          role="listbox"
          aria-label={t('commandPalette.title')}
        >
          <span id="palette-title" className="sr-only">
            {t('commandPalette.title')}
          </span>
          {filtered.length === 0 ? (
            <div className="empty-state palette-empty" role="status">
              <span className="empty-title">{t('commandPalette.no_results')}</span>
            </div>
          ) : (
            filtered.map((action, i) => {
              const Icon = action.icon;
              return (
                <button
                  key={action.id}
                  id={`palette-opt-${action.id}`}
                  type="button"
                  className={`cmd-item ${i === activeIndex ? 'active' : ''}`}
                  onClick={() => void runAction(action)}
                  onMouseEnter={() => setActiveIndex(i)}
                  role="option"
                  aria-selected={i === activeIndex}
                  disabled={running}
                >
                  <Icon size={15} strokeWidth={1.75} aria-hidden={true} />
                  <span>{action.label}</span>
                </button>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
