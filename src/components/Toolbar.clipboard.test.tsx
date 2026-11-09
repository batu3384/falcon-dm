import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import '../i18n';
import Toolbar from './Toolbar';

describe('Toolbar clipboard monitor', () => {
  const base = {
    onAddClick: vi.fn(),
    searchQuery: '',
    onSearchChange: vi.fn(),
    onPauseAll: vi.fn(),
    onResumeAll: vi.fn(),
    onOpenSettings: vi.fn(),
    onOpenScheduler: vi.fn(),
  };

  it('toggles via callback and exposes pressed state', async () => {
    const onToggle = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(
      <Toolbar
        {...base}
        clipboardMonitor={false}
        onToggleClipboard={onToggle}
      />,
    );
    const btn = screen.getByRole('button', { name: /Clipboard monitor/i });
    expect(btn).toHaveAttribute('aria-pressed', 'false');
    await user.click(btn);
    expect(onToggle).toHaveBeenCalledTimes(1);
    rerender(
      <Toolbar
        {...base}
        clipboardMonitor={true}
        onToggleClipboard={onToggle}
      />,
    );
    expect(screen.getByRole('button', { name: /Clipboard monitor/i })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  });
});
