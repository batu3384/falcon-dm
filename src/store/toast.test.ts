import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useToastStore } from './toast';

// ponytail: toast store is the global notification channel — verify add, auto-
// dismiss, and manual dismiss so a regression here can't silently swallow
// user-facing error messages.

describe('useToastStore', () => {
  beforeEach(() => {
    useToastStore.setState({ toasts: [] });
    vi.useFakeTimers();
  });

  it('adds a toast', () => {
    useToastStore.getState().showToast('error', 'boom');
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0].msg).toBe('boom');
  });

  it('auto-dismisses after the timeout', () => {
    useToastStore.getState().showToast('info', 'hi');
    expect(useToastStore.getState().toasts).toHaveLength(1);
    vi.advanceTimersByTime(3500);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('manual dismiss removes a specific toast', () => {
    useToastStore.getState().showToast('success', 'ok');
    const id = useToastStore.getState().toasts[0].id;
    useToastStore.getState().dismiss(id);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });
});
