import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ConfirmDialog } from './ConfirmDialog';

describe('ConfirmDialog', () => {
  it('focuses cancel before destructive confirm', () => {
    render(
      <ConfirmDialog
        message="Delete?"
        cancelLabel="Cancel"
        confirmLabel="Delete"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  });

  it('restores focus to the trigger when unmounted', () => {
    const trigger = document.createElement('button');
    document.body.appendChild(trigger);
    trigger.focus();
    const { unmount } = render(
      <ConfirmDialog
        message="Delete?"
        cancelLabel="Cancel"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    unmount();
    expect(trigger).toHaveFocus();
    trigger.remove();
  });

  it('passes deleteFromDisk when delete checkbox is checked', async () => {
    const onConfirm = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmDialog
        message="Remove?"
        confirmLabel="Remove"
        deleteFileOption
        onConfirm={onConfirm}
        onCancel={vi.fn()}
      />,
    );

    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: 'Remove' }));
    expect(onConfirm).toHaveBeenCalledWith(true);
  });

  it('passes false when delete checkbox is unchecked', async () => {
    const onConfirm = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmDialog
        message="Remove?"
        confirmLabel="Remove"
        deleteFileOption
        onConfirm={onConfirm}
        onCancel={vi.fn()}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Remove' }));
    expect(onConfirm).toHaveBeenCalledWith(false);
  });

  it('gives stacked dialogs unique accessible names', () => {
    render(
      <>
        <ConfirmDialog message="Remove A?" onConfirm={vi.fn()} onCancel={vi.fn()} />
        <ConfirmDialog message="Remove B?" onConfirm={vi.fn()} onCancel={vi.fn()} />
      </>,
    );
    const dialogs = screen.getAllByRole('alertdialog');
    const ids = dialogs.map((dialog) => dialog.getAttribute('aria-labelledby'));
    expect(ids[0]).toBeTruthy();
    expect(ids[1]).toBeTruthy();
    expect(ids[0]).not.toBe(ids[1]);
    expect(ids[0] && document.getElementById(ids[0])?.textContent).toBe('Remove A?');
    expect(ids[1] && document.getElementById(ids[1])?.textContent).toBe('Remove B?');
  });
});
