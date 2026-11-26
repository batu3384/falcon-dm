import { useState } from 'react';

/** Integer input that can be cleared while typing; clamps when focus leaves. */
export function NumberField({
  id,
  value,
  min,
  max,
  onChange,
  label,
  describedBy,
}: {
  id?: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
  label?: string;
  describedBy?: string;
}) {
  const [text, setText] = useState(String(value));
  const [synced, setSynced] = useState(value);
  // Follow outside changes (load, toggles) without fighting the user's typing.
  if (value !== synced) {
    setSynced(value);
    setText(String(value));
  }
  return (
    <input
      id={id}
      className="input narrow"
      type="number"
      inputMode="numeric"
      min={min}
      max={max}
      value={text}
      aria-label={label}
      aria-describedby={describedBy}
      onChange={(e) => {
        setText(e.target.value);
        const parsed = Number.parseInt(e.target.value, 10);
        if (Number.isFinite(parsed) && parsed >= min && parsed <= max) {
          setSynced(parsed);
          onChange(parsed);
        }
      }}
      onBlur={() => {
        const parsed = Number.parseInt(text, 10);
        const next = Number.isFinite(parsed) ? Math.min(max, Math.max(min, parsed)) : value;
        setText(String(next));
        if (next !== value) {
          setSynced(next);
          onChange(next);
        }
      }}
    />
  );
}
