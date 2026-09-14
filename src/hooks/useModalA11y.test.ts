import { describe, expect, it } from 'vitest';
import { pickModalInitialFocus, isModalFocusable, isForegroundDialogOpen } from './useModalA11y';

function el(tag: string, attrs: Record<string, string> = {}): HTMLElement {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  return node;
}

describe('pickModalInitialFocus', () => {
  it('prefers data-modal-initial-focus over cancel', () => {
    const cancel = el('button', { 'data-modal-cancel': '' });
    const field = el('input', { 'data-modal-initial-focus': '' });
    expect(pickModalInitialFocus([cancel, field])).toBe(field);
  });

  it('falls back to cancel then first', () => {
    const cancel = el('button', { 'data-modal-cancel': '' });
    const other = el('button');
    expect(pickModalInitialFocus([other, cancel])).toBe(cancel);
    expect(pickModalInitialFocus([other])).toBe(other);
  });
});

describe('isForegroundDialogOpen', () => {
  it('treats onboarding and modals as blocking', () => {
    expect(isForegroundDialogOpen()).toBe(false);
    const wizard = el('div', { class: 'wizard-overlay' });
    document.body.appendChild(wizard);
    expect(isForegroundDialogOpen()).toBe(true);
    expect(isForegroundDialogOpen({ ignorePalette: true })).toBe(true);
    wizard.remove();

    const palette = el('div', { class: 'modal-overlay palette-overlay' });
    document.body.appendChild(palette);
    expect(isForegroundDialogOpen()).toBe(true);
    expect(isForegroundDialogOpen({ ignorePalette: true })).toBe(false);
    palette.remove();

    const modal = el('div', { class: 'modal-overlay' });
    document.body.appendChild(modal);
    expect(isForegroundDialogOpen({ ignorePalette: true })).toBe(true);
    modal.remove();
  });
});

describe('isModalFocusable', () => {
  it('skips controls inside hidden ancestors', () => {
    const wrap = el('div', { hidden: '' });
    const field = el('input');
    wrap.appendChild(field);
    document.body.appendChild(wrap);
    expect(isModalFocusable(field)).toBe(false);
    wrap.remove();
  });

  it('keeps visible controls', () => {
    const field = el('input');
    document.body.appendChild(field);
    expect(isModalFocusable(field)).toBe(true);
    field.remove();
  });
});
