import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Search } from 'lucide-react';
import { Sheet } from './Sheet';

export interface PaletteAction {
  id: string;
  label: string;
  icon: React.ComponentType<{ size?: number; 'aria-hidden'?: boolean }>;
  shortcut?: string;
  keywords?: string;
  run: () => void | Promise<void>;
}

export function CommandPalette({
  actions,
  onClose,
  onError,
}: {
  actions: PaletteAction[];
  onClose: () => void;
  onError?: (error: unknown) => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return actions;
    return actions.filter(
      (a) => a.label.toLowerCase().includes(q) || (a.keywords?.toLowerCase().includes(q) ?? false),
    );
  }, [actions, query]);

  const run = async (action: PaletteAction | undefined) => {
    if (!action) return;
    // Close first so the next sheet gets focus.
    onClose();
    try {
      await action.run();
    } catch (error) {
      onError?.(error);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!filtered.length) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive((i) => (i + 1) % filtered.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive((i) => (i - 1 + filtered.length) % filtered.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      void run(filtered[active]);
    }
  };

  return (
    <Sheet
      label={t('palette.title')}
      onClose={onClose}
      dismissOnBackdrop
      className="palette"
      overlayClassName="palette-overlay"
    >
      <div className="palette-input">
        <Search size={17} aria-hidden={true} />
        <input
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
          placeholder={t('palette.placeholder')}
          aria-label={t('palette.placeholder')}
          role="combobox"
          aria-expanded={true}
          aria-controls="palette-list"
          aria-activedescendant={filtered[active] ? `palette-${filtered[active].id}` : undefined}
          data-modal-initial-focus
          spellCheck={false}
        />
      </div>
      <div
        className="palette-list"
        id="palette-list"
        role="listbox"
        aria-label={t('palette.title')}
      >
        {filtered.length === 0 ? (
          <div className="palette-empty" role="status">
            {t('palette.empty')}
          </div>
        ) : (
          filtered.map((action, i) => {
            const Icon = action.icon;
            return (
              <button
                key={action.id}
                id={`palette-${action.id}`}
                type="button"
                role="option"
                aria-selected={i === active}
                tabIndex={-1}
                className={`palette-item${i === active ? ' active' : ''}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => void run(action)}
              >
                <Icon size={15} aria-hidden={true} />
                <span>{action.label}</span>
                {action.shortcut && <kbd className="kbd">{action.shortcut}</kbd>}
              </button>
            );
          })
        )}
      </div>
    </Sheet>
  );
}
