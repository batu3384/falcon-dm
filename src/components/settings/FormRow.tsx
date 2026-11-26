import type { ReactNode } from 'react';

/** macOS settings row: right-aligned label, control column. */
export function FormRow({
  label,
  htmlFor,
  plain = false,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  /** Label sits beside a checkbox or text, not a 28px field. */
  plain?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="form-row">
      {htmlFor ? (
        <label className={`form-label${plain ? ' plain' : ''}`} htmlFor={htmlFor}>
          {label}
        </label>
      ) : (
        <span className={`form-label${plain ? ' plain' : ''}`}>{label}</span>
      )}
      <div className="form-group">{children}</div>
    </div>
  );
}

export function Check({
  id,
  checked,
  onChange,
  label,
  hint,
}: {
  id: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: ReactNode;
  hint?: ReactNode;
}) {
  return (
    <div className="check">
      <input
        id={id}
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        aria-describedby={hint ? `${id}-hint` : undefined}
      />
      <label htmlFor={id}>
        {label}
        {hint && (
          <span id={`${id}-hint`} className="hint">
            {hint}
          </span>
        )}
      </label>
    </div>
  );
}
