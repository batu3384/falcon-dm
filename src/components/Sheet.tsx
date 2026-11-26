import { useId, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useModalA11y } from '../hooks/useModalA11y';

interface SheetProps {
  title?: ReactNode;
  description?: ReactNode;
  /** Accessible name when there is no visible title. */
  label?: string;
  /** Id of an element inside the sheet that names it (alerts with their own heading). */
  labelledBy?: string;
  describedBy?: string;
  size?: 'small' | 'normal' | 'wide';
  role?: 'dialog' | 'alertdialog';
  onClose: () => void;
  /** Click on the dimmed backdrop closes the sheet. */
  dismissOnBackdrop?: boolean;
  className?: string;
  overlayClassName?: string;
  footer?: ReactNode;
  children?: ReactNode;
  as?: 'div' | 'form';
  onSubmit?: (e: React.FormEvent) => void;
}

/** Window-attached modal: focus trap, Escape, focus restore. */
export function Sheet({
  title,
  description,
  label,
  labelledBy,
  describedBy,
  size = 'normal',
  role = 'dialog',
  onClose,
  dismissOnBackdrop = false,
  className = '',
  overlayClassName = '',
  footer,
  children,
  as = 'div',
  onSubmit,
}: SheetProps) {
  const ref = useRef<HTMLElement>(null);
  const titleId = useId();
  const descId = useId();
  useModalA11y(ref, onClose);
  const Panel = as;
  const sizeClass = size === 'normal' ? '' : size;

  return createPortal(
    <div
      className={`sheet-overlay ${overlayClassName}`}
      role="presentation"
      onMouseDown={(e) => {
        if (dismissOnBackdrop && e.target === e.currentTarget) onClose();
      }}
    >
      <Panel
        ref={ref as never}
        className={`sheet ${sizeClass} ${className}`}
        role={role}
        aria-modal="true"
        aria-labelledby={title ? titleId : labelledBy}
        aria-label={title || labelledBy ? undefined : label}
        aria-describedby={description ? descId : describedBy}
        onSubmit={onSubmit}
      >
        {title && (
          <div className="sheet-head">
            <h2 id={titleId}>{title}</h2>
            {description && <p id={descId}>{description}</p>}
          </div>
        )}
        {children}
        {footer && <div className="sheet-foot">{footer}</div>}
      </Panel>
    </div>,
    document.body,
  );
}
